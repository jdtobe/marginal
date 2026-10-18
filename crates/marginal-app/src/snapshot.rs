//! Offscreen rendering to PNG, for visual checks and snapshot tests.

use std::path::Path;
use std::time::Instant;

use vello::util::RenderContext;
use vello::wgpu;
use vello::{AaConfig, RenderParams, Renderer, Scene};

use crate::editor::Editor;
use crate::findbar::FindBar;
use crate::gallery::panel::Panel;
use crate::paint::color;
use crate::renderer_options;
use crate::themes::ThemeEntry;
use crate::view::Fonts;
use vello::kurbo::Affine;

/// Renders the editor (and, if given, the find strip above it and the theme gallery beside it)
/// to a PNG.
pub fn snapshot(
    editor: &mut Editor,
    fonts: &mut Fonts,
    mut findbar: Option<&mut FindBar>,
    gallery: Option<(&mut Panel, &[ThemeEntry])>,
    out: &Path,
    scale: f64,
) -> anyhow::Result<()> {
    let mut context = RenderContext::new();
    let dev_id = pollster::block_on(context.device(None)).ok_or_else(|| anyhow::anyhow!("no GPU device"))?;
    let handle = &context.devices[dev_id];
    let (device, queue) = (&handle.device, &handle.queue);
    let mut renderer = Renderer::new(device, renderer_options())?;
    let side = if gallery.is_some() { crate::gallery::panel::WIDTH } else { 0.0 };
    let (width, height) = (((editor.width + side) as f64 * scale) as u32, (editor.height as f64 * scale) as u32);
    let mut scene = Scene::new();
    // Render with the caret in its visible phase.
    editor.activity = Instant::now();
    let inset = findbar.as_ref().map_or(0.0, |b| b.height(editor.width));
    if let Some(bar) = findbar.as_mut() {
        let caret = editor.selection.range().start;
        bar.refresh(editor.buffer.doc.text(), editor.buffer.history.version(), caret);
        editor.highlights = bar.matches.clone();
        editor.highlight_current = bar.current;
        if let Some(m) = bar.current_match() {
            editor.set_selection(marginal_doc::Selection::new(m.start, m.end));
        }
    }
    let screen = Affine::scale(scale);
    editor.paint(&mut scene, fonts, screen * Affine::translate((0.0, inset as f64)), editor.activity);
    if let Some((panel, installed)) = gallery {
        let appearance = editor.appearance;
        panel.paint(
            &mut scene,
            fonts,
            appearance,
            screen,
            editor.width,
            inset,
            editor.height,
            &panel.original.clone(),
            installed,
        );
    }
    if let Some(bar) = findbar {
        bar.paint(&mut scene, fonts, &editor.theme, editor.appearance, screen, editor.width, true);
    }
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("snapshot"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    renderer.render_to_texture(
        device,
        queue,
        &scene,
        &view,
        &RenderParams { base_color: color(editor.background()), width, height, antialiasing_method: AaConfig::Area },
    )?;
    let padded = (width * 4).div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: padded as u64 * height as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("copy") });
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: None },
        },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
    queue.submit([encoder.finish()]);
    let slice = buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
    device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None })?;
    rx.recv()??;
    let data = slice.get_mapped_range()?;
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for row in 0..height {
        let start = (row * padded) as usize;
        pixels.extend_from_slice(&data[start..start + (width * 4) as usize]);
    }
    let file = std::io::BufWriter::new(std::fs::File::create(out)?);
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&pixels)?;
    Ok(())
}
