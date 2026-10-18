//! The gallery panel on the right of the window: search, filters, and a list of built-in,
//! installed and Typora-gallery themes. Selecting a row previews it on the document; the app
//! runs the [`PanelAction`]s the panel returns.

use std::collections::HashMap;
use std::path::PathBuf;

use marginal_theme::{Appearance, BUILT_IN, Color, Fit, Style, Theme};
use vello::Scene;
use vello::kurbo::{Affine, Line, Rect, RoundedRect, Stroke};
use vello::peniko::{Fill, ImageData};
use winit::keyboard::{Key, NamedKey};

use super::{Entry, Variant};
use crate::paint::{color, draw_layout};
use crate::themes::ThemeEntry;
use crate::view::{Fonts, simple_layout};

/// The panel's width in window points.
pub const WIDTH: f32 = 340.0;
const ROW: f64 = 56.0;
const PAD: f64 = 12.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    All,
    Light,
    Dark,
    Installed,
    GoodFit,
}

const FILTERS: [(Filter, &str); 5] = [
    (Filter::All, "All"),
    (Filter::Light, "Light"),
    (Filter::Dark, "Dark"),
    (Filter::Installed, "Installed"),
    (Filter::GoodFit, "Good fit"),
];

#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    BuiltIn,
    Installed(PathBuf),
    /// An index into the gallery entries.
    Gallery(usize),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub name: String,
    pub source: Source,
    pub author: String,
    pub tags: Vec<String>,
}

#[derive(Debug)]
pub enum Download {
    Pending,
    Ready(Vec<(Variant, Fit)>),
    Failed(String),
}

#[derive(Debug, PartialEq)]
pub enum Status {
    Loading,
    Ready { fetched_at: u64, stale: bool },
    Failed(String),
}

/// Something for the app to do.
#[derive(Clone, Debug, PartialEq)]
pub enum PanelAction {
    /// The selection moved: preview this row.
    Preview(Row),
    /// Use (or install, then use) this row's theme and close.
    Commit(Row),
    OpenUrl(String),
    Reveal(PathBuf),
    Remove(PathBuf),
    /// Download this gallery entry again.
    Retry(usize),
    Refresh,
    /// Close and restore the theme in use before opening.
    Close,
}

#[derive(Clone, Debug, PartialEq)]
enum Hit {
    Row(usize),
    Filter(Filter),
    Action(PanelAction),
}

pub struct Panel {
    pub query: String,
    pub filter: Filter,
    pub selected: usize,
    scroll: f64,
    /// Scroll the selected row into view on the next paint (after the selection moves, not
    /// while the reader scrolls with the mouse).
    follow: bool,
    /// The theme in use when the panel opened, restored on Esc.
    pub original: String,
    pub status: Status,
    pub entries: Vec<Entry>,
    pub downloads: HashMap<String, Download>,
    /// Decoded thumbnails by slug; `None` once a thumbnail is known to be missing.
    pub thumbs: HashMap<String, Option<ImageData>>,
    /// Fit scores of built-in and installed themes, by name.
    pub fits: HashMap<String, Fit>,
    /// Whether a downloaded gallery theme turned out dark, by slug (most posts have no tags).
    pub dark: HashMap<String, bool>,
    /// Thumbnails the last paint wanted but didn't have.
    pub wanted_thumbs: Vec<(String, String)>,
    /// The theme styling the panel itself, fixed while previews change the document.
    chrome: Theme,
    hits: Vec<(Rect, Hit)>,
}

impl Panel {
    pub fn new(original: String, chrome: Theme) -> Self {
        Self {
            query: String::new(),
            filter: Filter::All,
            selected: 0,
            scroll: 0.0,
            follow: true,
            original,
            status: Status::Loading,
            entries: Vec::new(),
            downloads: HashMap::new(),
            thumbs: HashMap::new(),
            fits: HashMap::new(),
            dark: HashMap::new(),
            wanted_thumbs: Vec::new(),
            chrome,
            hits: Vec::new(),
        }
    }

    fn fit_of(&self, row: &Row) -> Option<&Fit> {
        match &row.source {
            Source::Gallery(i) => match self.downloads.get(&self.entries[*i].slug) {
                Some(Download::Ready(v)) => v.first().map(|(_, f)| f),
                _ => None,
            },
            _ => self.fits.get(&row.name),
        }
    }

    /// Whether a row's theme is dark: measured once downloaded, else from tags or its name.
    fn is_dark(&self, row: &Row) -> bool {
        if let Source::Gallery(i) = row.source
            && let Some(&dark) = self.dark.get(&self.entries[i].slug)
        {
            return dark;
        }
        if row.tags.iter().any(|t| t == "dark") {
            return true;
        }
        if row.tags.iter().any(|t| t == "light") {
            return false;
        }
        let name = row.name.to_lowercase();
        ["dark", "night", "black", "dracula", "monokai", "mocha", "midnight", "nord", "darcula", "dim"]
            .iter()
            .any(|w| name.contains(w))
    }

    /// The rows shown for the current query and filter, in display order.
    pub fn rows(&self, installed: &[ThemeEntry]) -> Vec<Row> {
        let words: Vec<String> = self.query.split_whitespace().map(str::to_lowercase).collect();
        let matches = |row: &Row| {
            let hay = format!("{} {} {}", row.name, row.author, row.tags.join(" ")).to_lowercase();
            words.iter().all(|w| hay.contains(w))
        };
        let mut rows: Vec<Row> = Vec::new();
        for (name, _) in BUILT_IN {
            rows.push(Row { name: (*name).into(), source: Source::BuiltIn, author: "Marginal".into(), tags: vec![] });
        }
        for e in installed.iter().filter(|e| e.path.is_some()) {
            rows.push(Row {
                name: e.name.clone(),
                source: Source::Installed(e.path.clone().unwrap()),
                author: "Installed".into(),
                tags: vec![],
            });
        }
        let installed_names: Vec<String> = rows.iter().map(|r| r.name.to_lowercase()).collect();
        for (i, e) in self.entries.iter().enumerate() {
            if installed_names.contains(&e.title.to_lowercase()) {
                continue;
            }
            rows.push(Row {
                name: e.title.clone(),
                source: Source::Gallery(i),
                author: e.author.clone(),
                tags: e.tags.clone(),
            });
        }
        rows.into_iter()
            .filter(|r| matches(r))
            .filter(|r| match self.filter {
                Filter::All => true,
                // Built-in themes follow the system appearance, so they count as both.
                Filter::Light => r.source == Source::BuiltIn || !self.is_dark(r),
                Filter::Dark => r.source == Source::BuiltIn || self.is_dark(r),
                Filter::Installed => matches!(r.source, Source::Installed(_)),
                Filter::GoodFit => self.fit_of(r).is_some_and(|f| f.ratio() >= 0.8),
            })
            .collect()
    }

    fn select(&mut self, index: usize, rows: &[Row]) -> Option<PanelAction> {
        if rows.is_empty() {
            return None;
        }
        self.selected = index.min(rows.len() - 1);
        self.follow = true;
        Some(PanelAction::Preview(rows[self.selected].clone()))
    }

    /// A key while the panel is open. `None` means it isn't the panel's.
    pub fn key(
        &mut self,
        key: &Key,
        text: Option<&str>,
        cmd: bool,
        installed: &[ThemeEntry],
    ) -> Option<Option<PanelAction>> {
        let rows = self.rows(installed);
        let action = match key {
            Key::Named(NamedKey::Escape) => Some(PanelAction::Close),
            Key::Named(NamedKey::ArrowDown) => self.select(self.selected + 1, &rows),
            Key::Named(NamedKey::ArrowUp) => self.select(self.selected.saturating_sub(1), &rows),
            Key::Named(NamedKey::Enter) => rows.get(self.selected).cloned().map(PanelAction::Commit),
            Key::Named(NamedKey::Backspace) => {
                self.query.pop();
                self.scroll = 0.0;
                self.select(0, &self.rows(installed))
            }
            _ if cmd => return None,
            _ => {
                let t = text.filter(|t| !t.is_empty() && !t.chars().any(char::is_control))?;
                self.query.push_str(t);
                self.scroll = 0.0;
                self.select(0, &self.rows(installed))
            }
        };
        Some(action)
    }

    /// A click inside the panel (window coordinates).
    pub fn click(&mut self, x: f32, y: f32, installed: &[ThemeEntry]) -> Option<PanelAction> {
        let hit = self.hits.iter().find(|(r, _)| r.contains((x as f64, y as f64))).map(|(_, h)| h.clone())?;
        match hit {
            Hit::Row(i) => {
                let rows = self.rows(installed);
                self.select(i, &rows)
            }
            Hit::Filter(f) => {
                self.filter = f;
                self.scroll = 0.0;
                let rows = self.rows(installed);
                self.select(0, &rows)
            }
            Hit::Action(a) => Some(a),
        }
    }

    pub fn scroll_by(&mut self, dy: f32) {
        self.scroll = (self.scroll + dy as f64).max(0.0);
        self.follow = false;
    }

    /// Draws the panel in the window's right `WIDTH` points, from `top` down to `bottom`.
    #[allow(clippy::too_many_arguments)]
    pub fn paint(
        &mut self,
        scene: &mut Scene,
        fonts: &mut Fonts,
        appearance: Appearance,
        origin: Affine,
        x0: f32,
        top: f32,
        bottom: f32,
        current: &str,
        installed: &[ThemeEntry],
    ) {
        let theme = &self.chrome;
        let body = theme.style(appearance, "body", &[], None);
        let paper = body.background.unwrap_or(Color::WHITE);
        let glyph = theme.style(appearance, "span", &["glyph"], Some(&body)).color;
        let accent = theme.style(appearance, "a", &[], Some(&body)).color;
        let text = Style { font_size: 13.0, line_height: 1.3, background: None, ..body.clone() };
        let muted = Style { color: glyph, font_size: 11.5, ..text.clone() };
        let bold = Style { font_weight: 700.0, ..text.clone() };
        let (x0, top, bottom) = (x0 as f64, top as f64, bottom as f64);
        let w = WIDTH as f64;
        let mut hits = Vec::new();

        let frame = Rect::new(x0, top, x0 + w, bottom);
        scene.fill(Fill::NonZero, origin, color(paper), None, &frame);
        scene.stroke(
            &Stroke::new(1.0),
            origin,
            color(glyph.with_alpha(0.4)),
            None,
            &Line::new((x0, top), (x0, bottom)),
        );

        // Header: title, refresh, close.
        let mut y = top + 10.0;
        let title = simple_layout(fonts, "Themes", &bold, None);
        draw_layout(scene, &title, origin * Affine::translate((x0 + PAD, y)));
        for (i, (label, action)) in [("×", PanelAction::Close), ("↻", PanelAction::Refresh)].into_iter().enumerate()
        {
            let l = simple_layout(fonts, label, &text, None);
            let bx = x0 + w - PAD - 22.0 * (i as f64 + 1.0);
            draw_layout(scene, &l, origin * Affine::translate((bx + 5.0, y)));
            hits.push((Rect::new(bx, y - 4.0, bx + 20.0, y + 20.0), Hit::Action(action)));
        }
        y += 28.0;

        // Search field.
        let field = Rect::new(x0 + PAD, y, x0 + w - PAD, y + 28.0);
        scene.fill(Fill::NonZero, origin, color(paper), None, &RoundedRect::from_rect(field, 5.0));
        scene.stroke(&Stroke::new(1.5), origin, color(accent), None, &RoundedRect::from_rect(field, 5.0));
        let shown = if self.query.is_empty() {
            simple_layout(fonts, "Search themes", &Style { color: glyph, ..text.clone() }, None)
        } else {
            simple_layout(fonts, &self.query, &text, None)
        };
        let ty = y + (28.0 - shown.height() as f64) / 2.0;
        draw_layout(scene, &shown, origin * Affine::translate((x0 + PAD + 8.0, ty)));
        let caret_x = x0 + PAD + 8.0 + if self.query.is_empty() { 0.0 } else { shown.full_width() as f64 };
        scene.fill(
            Fill::NonZero,
            origin,
            color(accent),
            None,
            &Rect::new(caret_x, ty, caret_x + 1.5, ty + shown.height() as f64),
        );
        y += 36.0;

        // Filter chips.
        let mut cx = x0 + PAD;
        for (f, label) in FILTERS {
            let on = self.filter == f;
            let l = simple_layout(
                fonts,
                label,
                &Style { color: if on { paper } else { glyph }, font_size: 11.5, ..text.clone() },
                None,
            );
            let r = Rect::new(cx, y, cx + l.width() as f64 + 16.0, y + 20.0);
            if on {
                scene.fill(Fill::NonZero, origin, color(accent), None, &RoundedRect::from_rect(r, 10.0));
            } else {
                scene.stroke(
                    &Stroke::new(1.0),
                    origin,
                    color(glyph.with_alpha(0.5)),
                    None,
                    &RoundedRect::from_rect(r, 10.0),
                );
            }
            draw_layout(scene, &l, origin * Affine::translate((cx + 8.0, y + (20.0 - l.height() as f64) / 2.0)));
            hits.push((r, Hit::Filter(f)));
            cx = r.x1 + 6.0;
        }
        y += 30.0;

        // Footer: keys and index status.
        let footer_h = 42.0;
        let list_bottom = bottom - footer_h;
        scene.stroke(
            &Stroke::new(1.0),
            origin,
            color(glyph.with_alpha(0.3)),
            None,
            &Line::new((x0, list_bottom), (x0 + w, list_bottom)),
        );
        let status = match &self.status {
            Status::Loading => "Loading the Typora gallery…".to_owned(),
            Status::Ready { fetched_at, stale: false } => format!("Gallery updated {}", ago(*fetched_at)),
            Status::Ready { fetched_at, stale: true } => format!("Offline · themes from {}", ago(*fetched_at)),
            Status::Failed(e) => format!("Gallery unavailable: {e}"),
        };
        let hint = simple_layout(fonts, "↑↓ preview · ⏎ use · esc cancel", &muted, None);
        draw_layout(scene, &hint, origin * Affine::translate((x0 + PAD, list_bottom + 5.0)));
        let status = simple_layout(fonts, &status, &muted, Some((w - PAD * 2.0) as f32));
        draw_layout(scene, &status, origin * Affine::translate((x0 + PAD, list_bottom + 5.0 + hint.height() as f64)));

        // The list, clipped and scrolled.
        let list = Rect::new(x0, y, x0 + w, list_bottom);
        scene.push_clip_layer(Fill::NonZero, origin, &list);
        let rows = self.rows(installed);
        self.selected = self.selected.min(rows.len().saturating_sub(1));
        // Keep the selection in view.
        let detail_h = 64.0;
        let mut offsets = Vec::with_capacity(rows.len());
        let mut cursor = 0.0;
        let mut last_section = None;
        for (i, row) in rows.iter().enumerate() {
            let section = section_of(&row.source);
            if last_section != Some(section) {
                cursor += 22.0;
                last_section = Some(section);
            }
            offsets.push(cursor);
            cursor += ROW + if i == self.selected { detail_h } else { 0.0 };
        }
        let view_h = list_bottom - y;
        if let Some(&sel_top) = offsets.get(self.selected).filter(|_| std::mem::take(&mut self.follow)) {
            if sel_top < self.scroll {
                self.scroll = (sel_top - 22.0).max(0.0);
            } else if sel_top + ROW + detail_h > self.scroll + view_h {
                self.scroll = sel_top + ROW + detail_h - view_h;
            }
        }
        self.scroll = self.scroll.min((cursor - view_h).max(0.0));

        if rows.is_empty() {
            let msg = if self.query.is_empty() {
                "No themes here yet."
            } else {
                "No themes match. Clear the search or pick All."
            };
            let l = simple_layout(fonts, msg, &muted, Some((w - PAD * 2.0) as f32));
            draw_layout(scene, &l, origin * Affine::translate((x0 + PAD, y + 12.0)));
        }
        let gallery_count = rows.iter().filter(|r| matches!(r.source, Source::Gallery(_))).count();
        let mut wanted = Vec::new();
        let mut last_section = None;
        for (i, row) in rows.iter().enumerate() {
            let row_top = y + offsets[i] - self.scroll;
            let section = section_of(&row.source);
            if last_section != Some(section) {
                last_section = Some(section);
                let label = match section {
                    0 => "BUILT-IN".to_owned(),
                    1 => "INSTALLED".to_owned(),
                    _ => format!("TYPORA GALLERY · {gallery_count}"),
                };
                let l = simple_layout(
                    fonts,
                    &label,
                    &Style { font_size: 10.0, letter_spacing: 1.2, ..muted.clone() },
                    None,
                );
                draw_layout(scene, &l, origin * Affine::translate((x0 + PAD, row_top - 18.0)));
            }
            let height = ROW + if i == self.selected { detail_h } else { 0.0 };
            if row_top + height < y || row_top > list_bottom {
                continue;
            }
            let r = Rect::new(x0, row_top, x0 + w, row_top + height);
            if i == self.selected {
                scene.fill(Fill::NonZero, origin, color(accent.with_alpha(0.14)), None, &r);
                scene.fill(
                    Fill::NonZero,
                    origin,
                    color(accent),
                    None,
                    &Rect::new(x0, row_top, x0 + 2.0, row_top + height),
                );
            }
            hits.push((Rect::new(x0, row_top, x0 + w, row_top + ROW), Hit::Row(i)));

            // Thumbnail.
            let thumb = Rect::new(x0 + PAD, row_top + 6.0, x0 + PAD + 64.0, row_top + 6.0 + 44.0);
            self.paint_thumb(scene, fonts, origin, thumb, row, appearance, &mut wanted);
            scene.stroke(
                &Stroke::new(1.0),
                origin,
                color(glyph.with_alpha(0.4)),
                None,
                &RoundedRect::from_rect(thumb, 4.0),
            );

            // Name, badge, meta.
            let tx = thumb.x1 + 10.0;
            let name = simple_layout(fonts, &row.name, &bold, Some((w - (tx - x0) - PAD) as f32));
            draw_layout(scene, &name, origin * Affine::translate((tx, row_top + 8.0)));
            let mut badge_x = tx + name.width() as f64 + 6.0;
            let badge = if row.name == current {
                Some(("in use".to_owned(), accent))
            } else {
                self.fit_of(row).map(|f| {
                    let c = match f.label() {
                        "Good fit" => Color::rgb(0x3f, 0xb9, 0x50),
                        "Partial" => Color::rgb(0xd2, 0x99, 0x22),
                        _ => Color::rgb(0xf8, 0x51, 0x49),
                    };
                    (f.label().to_owned(), c)
                })
            };
            if let Some((label, c)) = badge {
                let l = simple_layout(fonts, &label, &Style { color: c, font_size: 10.0, ..muted.clone() }, None);
                if badge_x + l.width() as f64 + 10.0 > x0 + w - PAD {
                    badge_x = tx;
                }
                let br = Rect::new(
                    badge_x,
                    row_top + 10.0,
                    badge_x + l.width() as f64 + 10.0,
                    row_top + 10.0 + l.height() as f64 + 2.0,
                );
                scene.stroke(&Stroke::new(1.0), origin, color(c), None, &RoundedRect::from_rect(br, 3.0));
                draw_layout(scene, &l, origin * Affine::translate((badge_x + 5.0, row_top + 11.0)));
            }
            let meta = if row.tags.is_empty() {
                row.author.clone()
            } else {
                format!("{} · {}", row.author, row.tags.iter().take(3).cloned().collect::<Vec<_>>().join(", "))
            };
            let m = simple_layout(fonts, &meta, &muted, Some((w - (tx - x0) - PAD) as f32));
            draw_layout(scene, &m, origin * Affine::translate((tx, row_top + 30.0)));

            if i == self.selected {
                self.paint_detail(
                    scene,
                    fonts,
                    origin,
                    x0 + PAD + 74.0,
                    row_top + ROW,
                    w - PAD * 2.0 - 74.0,
                    row,
                    &text,
                    &muted,
                    accent,
                    paper,
                    &mut hits,
                );
            }
        }
        scene.pop_layer();
        self.wanted_thumbs = wanted;
        self.hits = hits;
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_thumb(
        &self,
        scene: &mut Scene,
        fonts: &mut Fonts,
        origin: Affine,
        r: Rect,
        row: &Row,
        appearance: Appearance,
        wanted: &mut Vec<(String, String)>,
    ) {
        scene.push_clip_layer(Fill::NonZero, origin, &RoundedRect::from_rect(r, 4.0));
        match &row.source {
            Source::Gallery(i) => {
                let e = &self.entries[*i];
                match self.thumbs.get(&e.slug) {
                    Some(Some(img)) => {
                        // Cover the box, keeping the aspect ratio.
                        let s = (r.width() / img.width as f64).max(r.height() / img.height as f64);
                        scene.draw_image(img, origin * Affine::translate((r.x0, r.y0)) * Affine::scale(s));
                    }
                    found => {
                        scene.fill(
                            Fill::NonZero,
                            origin,
                            color(Color::rgb(0x88, 0x88, 0x88).with_alpha(0.25)),
                            None,
                            &r,
                        );
                        if found.is_none()
                            && let Some(url) = &e.thumbnail
                        {
                            wanted.push((e.slug.clone(), url.clone()));
                        }
                    }
                }
            }
            source => {
                // A swatch drawn from the theme's own colors.
                let swatch = match source {
                    Source::BuiltIn => Theme::built_in(&row.name),
                    _ => None,
                };
                let (bg, ink, accent) = match &swatch {
                    Some(t) => {
                        let b = t.style(appearance, "body", &[], None);
                        (b.background.unwrap_or(Color::WHITE), b.color, t.style(appearance, "h1", &[], Some(&b)).color)
                    }
                    None => (Color::rgb(0xee, 0xee, 0xee), Color::rgb(0x33, 0x33, 0x33), Color::rgb(0x55, 0x55, 0x99)),
                };
                scene.fill(Fill::NonZero, origin, color(bg), None, &r);
                scene.fill(
                    Fill::NonZero,
                    origin,
                    color(accent),
                    None,
                    &Rect::new(r.x0 + 8.0, r.y0 + 9.0, r.x0 + 34.0, r.y0 + 14.0),
                );
                for k in 0..3 {
                    let ly = r.y0 + 20.0 + k as f64 * 7.0;
                    scene.fill(
                        Fill::NonZero,
                        origin,
                        color(ink.with_alpha(0.6)),
                        None,
                        &Rect::new(r.x0 + 8.0, ly, r.x1 - 10.0 - k as f64 * 6.0, ly + 3.0),
                    );
                }
                let _ = fonts;
            }
        }
        scene.pop_layer();
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_detail(
        &self,
        scene: &mut Scene,
        fonts: &mut Fonts,
        origin: Affine,
        x: f64,
        y: f64,
        width: f64,
        row: &Row,
        text: &Style,
        muted: &Style,
        accent: Color,
        paper: Color,
        hits: &mut Vec<(Rect, Hit)>,
    ) {
        let fit_line = |f: &Fit| {
            let pct = (f.ratio() * 100.0).round();
            if f.missing.is_empty() {
                format!("Marginal shows {pct}% of this theme.")
            } else {
                format!("Marginal shows {pct}%. Missing: {}.", f.summary(3))
            }
        };
        let (line, buttons): (String, Vec<(&str, PanelAction, bool)>) = match &row.source {
            Source::BuiltIn => ("Built into Marginal.".into(), vec![("Use ⏎", PanelAction::Commit(row.clone()), true)]),
            Source::Installed(path) => (
                self.fits.get(&row.name).map_or("Installed theme.".into(), fit_line),
                vec![
                    ("Use ⏎", PanelAction::Commit(row.clone()), true),
                    ("Reveal", PanelAction::Reveal(path.clone()), false),
                    ("Remove", PanelAction::Remove(path.clone()), false),
                ],
            ),
            Source::Gallery(i) => {
                let e = &self.entries[*i];
                let home =
                    e.homepage.clone().or(e.download.clone()).map(|u| ("Homepage", PanelAction::OpenUrl(u), false));
                match self.downloads.get(&e.slug) {
                    Some(Download::Ready(v)) => {
                        let mut line = v.first().map_or(String::new(), |(_, f)| fit_line(f));
                        if v.len() > 1 {
                            let names: Vec<&str> = v.iter().map(|(var, _)| var.name.as_str()).collect();
                            line.push_str(&format!(" Includes {}.", names.join(", ")));
                        }
                        let mut b = vec![("Install ⏎", PanelAction::Commit(row.clone()), true)];
                        b.extend(home);
                        (line, b)
                    }
                    Some(Download::Failed(err)) => {
                        let mut b = vec![("Retry", PanelAction::Retry(*i), false)];
                        b.extend(home);
                        (format!("Couldn't download: {err}"), b)
                    }
                    _ => ("Downloading to preview…".into(), home.into_iter().collect()),
                }
            }
        };
        let l = simple_layout(fonts, &line, muted, Some(width as f32));
        draw_layout(scene, &l, origin * Affine::translate((x, y)));
        let mut bx = x;
        let by = y + (l.height() as f64).min(30.0) + 6.0;
        for (label, action, primary) in buttons {
            let style = Style { color: if primary { paper } else { text.color }, font_size: 12.0, ..text.clone() };
            let bl = simple_layout(fonts, label, &style, None);
            let r = Rect::new(bx, by, bx + bl.width() as f64 + 16.0, by + 20.0);
            if primary {
                scene.fill(Fill::NonZero, origin, color(accent), None, &RoundedRect::from_rect(r, 4.0));
            } else {
                scene.stroke(
                    &Stroke::new(1.0),
                    origin,
                    color(muted.color.with_alpha(0.6)),
                    None,
                    &RoundedRect::from_rect(r, 4.0),
                );
            }
            draw_layout(scene, &bl, origin * Affine::translate((bx + 8.0, by + (20.0 - bl.height() as f64) / 2.0)));
            hits.push((r, Hit::Action(action)));
            bx = r.x1 + 6.0;
        }
    }
}

fn section_of(source: &Source) -> u8 {
    match source {
        Source::BuiltIn => 0,
        Source::Installed(_) => 1,
        Source::Gallery(_) => 2,
    }
}

/// "today", "yesterday", "3 days ago".
fn ago(secs: u64) -> String {
    let days = super::index::now_secs().saturating_sub(secs) / 86_400;
    match days {
        0 => "today".into(),
        1 => "yesterday".into(),
        d => format!("{d} days ago"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel() -> Panel {
        let mut p = Panel::new("Terminal".into(), Theme::terminal());
        p.entries = vec![
            Entry {
                slug: "night".into(),
                title: "Night".into(),
                author: "typora".into(),
                tags: vec!["dark".into()],
                ..Default::default()
            },
            Entry {
                slug: "github".into(),
                title: "Github".into(),
                author: "typora".into(),
                tags: vec!["light".into()],
                ..Default::default()
            },
        ];
        p
    }

    #[test]
    fn rows_search_and_filter() {
        let mut p = panel();
        assert_eq!(p.rows(&[]).len(), BUILT_IN.len() + 2);
        p.query = "night".into();
        assert_eq!(p.rows(&[]).iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["Night"]);
        p.query.clear();
        p.filter = Filter::Dark;
        assert!(p.rows(&[]).iter().all(|r| r.name != "Github"));
        p.filter = Filter::Installed;
        assert!(p.rows(&[]).is_empty());
    }

    #[test]
    fn wheel_scrolling_is_not_undone_by_the_selection() {
        let mut p = panel();
        p.entries = (0..40)
            .map(|i| Entry {
                slug: format!("t{i}"),
                title: format!("Theme {i}"),
                author: "x".into(),
                ..Default::default()
            })
            .collect();
        let mut fonts = Fonts::new();
        let mut scene = Scene::new();
        let mut paint = |p: &mut Panel| {
            p.paint(&mut scene, &mut fonts, Appearance::Dark, Affine::IDENTITY, 0.0, 0.0, 600.0, "Terminal", &[]);
        };
        paint(&mut p);
        assert_eq!(p.scroll, 0.0, "the first row is selected and visible");
        p.scroll_by(500.0);
        paint(&mut p);
        assert_eq!(p.scroll, 500.0, "wheel scrolling sticks even though the selection is off screen");
        p.key(&Key::Named(NamedKey::ArrowDown), None, false, &[]);
        paint(&mut p);
        assert!(p.scroll < 500.0, "moving the selection brings it back into view");
    }

    #[test]
    fn keys_move_and_commit() {
        let mut p = panel();
        p.query = "git".into();
        let a = p.key(&Key::Named(NamedKey::ArrowDown), None, false, &[]).unwrap();
        assert!(matches!(a, Some(PanelAction::Preview(r)) if r.name == "Github"));
        let a = p.key(&Key::Named(NamedKey::Enter), None, false, &[]).unwrap();
        assert!(matches!(a, Some(PanelAction::Commit(r)) if r.name == "Github"));
        assert_eq!(p.key(&Key::Named(NamedKey::Escape), None, false, &[]), Some(Some(PanelAction::Close)));
        assert_eq!(p.key(&Key::Character("t".into()), Some("t"), true, &[]), None, "⌘ keys pass through");
    }
}
