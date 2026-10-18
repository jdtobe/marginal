//! The theme gallery: search the Typora theme gallery (plus built-in and installed themes),
//! preview a theme on the open document, and install it. Network and disk work happens on a
//! worker thread; results come back to the event loop as [`GalleryEvent`]s.

pub mod download;
pub mod index;
pub mod panel;
#[cfg(test)]
mod survey;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Sender, channel};

use vello::peniko::{Blob, ImageAlphaType, ImageData, ImageFormat};

pub use download::Variant;
pub use index::Entry;

#[derive(Debug)]
pub enum GalleryEvent {
    /// The listing, and when it was fetched (seconds since the epoch). `stale` means the
    /// network failed and this is the cache.
    Index {
        entries: Vec<Entry>,
        fetched_at: u64,
        stale: bool,
    },
    IndexFailed(String),
    Thumbnail {
        slug: String,
        image: Option<ImageData>,
    },
    Downloaded {
        slug: String,
        result: Result<Vec<Variant>, String>,
    },
}

enum Job {
    Index { force: bool },
    Thumbnail { slug: String, url: String },
    Download { slug: String, url: Option<String>, homepage: Option<String> },
}

/// Where the gallery keeps its cache, under the app's data folder.
#[derive(Clone)]
pub struct Paths {
    pub root: PathBuf,
}

impl Paths {
    pub fn index(&self) -> PathBuf {
        self.root.join("index.json")
    }
    fn thumb(&self, slug: &str) -> PathBuf {
        self.root.join("thumbs").join(format!("{slug}.png"))
    }
    pub fn download(&self, slug: &str) -> PathBuf {
        self.root.join("cache").join(slug)
    }
}

/// Runs gallery jobs one at a time on a background thread.
pub struct Worker {
    tx: Sender<Job>,
}

impl Worker {
    pub fn start(paths: Paths, notify: impl Fn(GalleryEvent) + Send + 'static) -> Self {
        let (tx, rx) = channel::<Job>();
        let p = paths;
        std::thread::Builder::new()
            .name("marginal-gallery".into())
            .spawn(move || {
                for job in rx {
                    match job {
                        Job::Index { force } => notify(load_index(&p, force)),
                        Job::Thumbnail { slug, url } => {
                            let image = thumbnail(&p.thumb(&slug), &url);
                            notify(GalleryEvent::Thumbnail { slug, image });
                        }
                        Job::Download { slug, url, homepage } => {
                            let result = download::download(url.as_deref(), homepage.as_deref(), &p.download(&slug));
                            notify(GalleryEvent::Downloaded { slug, result });
                        }
                    }
                }
            })
            .expect("spawn gallery thread");
        Self { tx }
    }

    pub fn load_index(&self, force: bool) {
        let _ = self.tx.send(Job::Index { force });
    }

    pub fn thumbnail(&self, slug: &str, url: &str) {
        let _ = self.tx.send(Job::Thumbnail { slug: slug.into(), url: url.into() });
    }

    pub fn download(&self, slug: &str, url: Option<&str>, homepage: Option<&str>) {
        let job = Job::Download { slug: slug.into(), url: url.map(Into::into), homepage: homepage.map(Into::into) };
        let _ = self.tx.send(job);
    }
}

fn load_index(paths: &Paths, force: bool) -> GalleryEvent {
    let cached = index::Cached::load(&paths.index());
    if let Some(c) = &cached
        && c.is_fresh()
        && !force
    {
        return GalleryEvent::Index { entries: c.entries.clone(), fetched_at: c.fetched_at, stale: false };
    }
    match index::fetch() {
        Ok(entries) => {
            let fresh = index::Cached { fetched_at: index::now_secs(), entries };
            if let Err(e) = fresh.save(&paths.index()) {
                eprintln!("could not cache the theme index: {e}");
            }
            GalleryEvent::Index { entries: fresh.entries, fetched_at: fresh.fetched_at, stale: false }
        }
        Err(e) => match cached {
            Some(c) => GalleryEvent::Index { entries: c.entries, fetched_at: c.fetched_at, stale: true },
            None => GalleryEvent::IndexFailed(e),
        },
    }
}

/// A thumbnail from the cache or the network, decoded for drawing. Only PNGs are supported.
fn thumbnail(cache: &std::path::Path, url: &str) -> Option<ImageData> {
    let bytes = match std::fs::read(cache) {
        Ok(b) => b,
        Err(_) => {
            let b = index::get(url, 4 << 20).ok()?;
            if let Some(dir) = cache.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(cache, &b);
            b
        }
    };
    decode_png(&bytes)
}

pub fn decode_png(bytes: &[u8]) -> Option<ImageData> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let pixels = &buf[..info.buffer_size()];
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => pixels.to_vec(),
        png::ColorType::Rgb => pixels.chunks(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => pixels.chunks(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        png::ColorType::Grayscale => pixels.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return None,
    };
    Some(ImageData {
        data: Blob::new(Arc::new(rgba)),
        format: ImageFormat::Rgba8,
        alpha_type: ImageAlphaType::Alpha,
        width: info.width,
        height: info.height,
    })
}
