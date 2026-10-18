use marginal_theme::Color;
use parley::{Layout, PositionedLayoutItem};
use vello::Scene;
use vello::kurbo::{Affine, Line, Rect, RoundedRect, Stroke};
use vello::peniko::{Color as PColor, Fill};

use crate::view::{BlockView, Brush, DocView};

pub fn color(c: Color) -> PColor {
    PColor::from_rgba8(c.r, c.g, c.b, c.a)
}

/// Draws a parley layout with its top-left at `transform`'s origin.
pub fn draw_layout(scene: &mut Scene, layout: &Layout<Brush>, transform: Affine) {
    for line in layout.lines() {
        for item in line.items() {
            let PositionedLayoutItem::GlyphRun(glyph_run) = item else { continue };
            let style = glyph_run.style();
            let brush = color(style.brush.color());
            let run = glyph_run.run();
            let font = run.font();
            let font_size = run.font_size();
            let synthesis = run.synthesis();
            let glyph_xform = synthesis.skew().map(|angle| Affine::skew(angle.to_radians().tan() as f64, 0.0));
            let y = glyph_run.baseline();
            let coords: Vec<i16> = run.normalized_coords().iter().map(|c| c.to_bits()).collect();
            scene
                .draw_glyphs(font)
                .brush(brush)
                .hint(false)
                .transform(transform)
                .glyph_transform(glyph_xform)
                .font_size(font_size)
                .normalized_coords(&coords)
                .draw(Fill::NonZero, glyph_run.positioned_glyphs().map(|g| vello::Glyph { id: g.id, x: g.x, y: g.y }));

            let metrics = run.font_metrics();
            let mut decoration = |offset: f32, size: f32, brush: Option<Brush>| {
                let w = glyph_run.advance() as f64;
                let x0 = glyph_run.offset() as f64;
                let y0 = (y - offset) as f64;
                let rect = Rect::new(x0, y0, x0 + w, y0 + size.max(1.0) as f64);
                let c = brush.map_or(brush_of(style), |b| color(b.color()));
                scene.fill(Fill::NonZero, transform, c, None, &rect);
            };
            if let Some(u) = &style.underline {
                decoration(
                    u.offset.unwrap_or(metrics.underline_offset),
                    u.size.unwrap_or(metrics.underline_size),
                    Some(u.brush),
                );
            }
            if let Some(st) = &style.strikethrough {
                decoration(
                    st.offset.unwrap_or(metrics.strikethrough_offset),
                    st.size.unwrap_or(metrics.strikethrough_size),
                    Some(st.brush),
                );
            }
        }
    }
}

fn brush_of(style: &parley::Style<Brush>) -> PColor {
    color(style.brush.color())
}

fn draw_ornament(scene: &mut Scene, origin: Affine, o: &crate::view::Ornament) {
    use crate::view::Ornament;
    match o {
        Ornament::Text { layout, x, y, background, radius, padding } => {
            if let Some(bg) = background {
                let rect = Rect::new(
                    (*x - padding[3]) as f64,
                    (*y - padding[0]) as f64,
                    (*x + layout.width() + padding[1]) as f64,
                    (*y + layout.height() + padding[2]) as f64,
                );
                scene.fill(Fill::NonZero, origin, color(*bg), None, &RoundedRect::from_rect(rect, *radius as f64));
            }
            draw_layout(scene, layout, origin * Affine::translate((*x as f64, *y as f64)));
        }
        Ornament::Shape { rect, color: fill, border, radius } => {
            let r = RoundedRect::new(rect.0 as f64, rect.1 as f64, rect.2 as f64, rect.3 as f64, *radius as f64);
            if let Some(c) = fill {
                scene.fill(Fill::NonZero, origin, color(*c), None, &r);
            }
            if let Some(b) = border {
                let band = Rect::new(rect.0 as f64, (rect.3 - b.width) as f64, rect.2 as f64, rect.3 as f64);
                scene.fill(Fill::NonZero, origin, color(b.color), None, &band);
            }
        }
    }
}

/// A block's borders: a rounded outline when the corners are rounded and the sides match,
/// otherwise each side as a band along the box's edge.
fn draw_borders(scene: &mut Scene, origin: Affine, s: &marginal_theme::Style, outer: Rect) {
    let sides = [s.border_top, s.border_right, s.border_bottom, s.border_left];
    if s.border_radius > 0.0
        && let [Some(first), ..] = sides
        && sides.iter().all(|b| *b == Some(first))
    {
        let w = first.width as f64;
        let rect = RoundedRect::from_rect(outer.inset(-w / 2.0), s.border_radius as f64);
        scene.stroke(&Stroke::new(w), origin, color(first.color), None, &rect);
        return;
    }
    let (x0, y0, x1, y1) = (outer.x0, outer.y0, outer.x1, outer.y1);
    let bands = [
        sides[0].map(|b| (Rect::new(x0, y0, x1, y0 + b.width as f64), b.color)),
        sides[1].map(|b| (Rect::new(x1 - b.width as f64, y0, x1, y1), b.color)),
        sides[2].map(|b| (Rect::new(x0, y1 - b.width as f64, x1, y1), b.color)),
        sides[3].map(|b| (Rect::new(x0, y0, x0 + b.width as f64, y1), b.color)),
    ];
    for (rect, c) in bands.into_iter().flatten() {
        scene.fill(Fill::NonZero, origin, color(c), None, &rect);
    }
}

pub fn draw_document(scene: &mut Scene, view: &DocView, origin: Affine) {
    for b in &view.blocks {
        draw_block(scene, b, origin);
    }
}

fn draw_block(scene: &mut Scene, b: &BlockView, origin: Affine) {
    let s = &b.style;
    // The box including a bottom border, which sits just below `bottom`.
    let below = if b.rule { 0.0 } else { s.border_bottom.map_or(0.0, |bd| bd.width) };
    let outer = Rect::new(b.box_x as f64, b.top as f64, (b.box_x + b.box_width) as f64, (b.bottom + below) as f64);
    if let Some(shadow) = s.box_shadow.filter(|_| b.cell.is_none()) {
        let r = outer.inflate(shadow.spread as f64, shadow.spread as f64)
            + vello::kurbo::Vec2::new(shadow.dx as f64, shadow.dy as f64);
        scene.draw_blurred_rounded_rect(
            origin,
            r,
            color(shadow.color),
            s.border_radius as f64,
            (shadow.blur / 2.0).max(0.5) as f64,
        );
    }
    if let Some(bg) = s.background {
        let rect = RoundedRect::new(
            b.box_x as f64,
            b.top as f64,
            (b.box_x + b.box_width) as f64,
            b.bottom as f64,
            s.border_radius as f64,
        );
        scene.fill(Fill::NonZero, origin, color(bg), None, &rect);
    }
    if let Some(cell) = &b.cell {
        let rect = Rect::new(b.box_x as f64, b.top as f64, (b.box_x + b.box_width) as f64, b.bottom as f64);
        scene.stroke(&Stroke::new(1.0), origin, color(cell.border), None, &rect);
        if cell.row == 0 {
            let y = b.bottom as f64;
            let line = Line::new((b.box_x as f64, y), ((b.box_x + b.box_width) as f64, y));
            scene.stroke(&Stroke::new(2.0), origin, color(cell.header_rule), None, &line);
        }
    }
    for &(x, top, c) in &b.quote_bars {
        let width = 2.0;
        let rect = Rect::new(x as f64, top as f64, (x + width) as f64, b.bottom as f64);
        scene.fill(Fill::NonZero, origin, color(c), None, &rect);
    }
    for o in &b.ornaments {
        draw_ornament(scene, origin, o);
    }
    if b.rule {
        let c = s.border_bottom.map_or(s.color.with_alpha(0.3), |bd| bd.color);
        let y = (b.top + b.bottom) as f64 / 2.0;
        let line = Line::new((b.box_x as f64, y), ((b.box_x + b.box_width) as f64, y));
        scene.stroke(&Stroke::new(s.border_bottom.map_or(1.0, |bd| bd.width) as f64), origin, color(c), None, &line);
    } else {
        draw_layout(scene, &b.layout, origin * Affine::translate((b.text_x as f64, b.text_y as f64)));
    }
    if b.cell.is_none() && !b.rule {
        draw_borders(scene, origin, s, outer);
    }
    if let Some(g) = &b.glyph
        && let Some(bg) = g.pill
    {
        let (pad_x, pad_y) = (g.layout.height() as f64 * 0.45, 1.0);
        let rect = RoundedRect::new(
            g.x as f64 - pad_x,
            g.y as f64 - pad_y,
            (g.x + g.layout.width()) as f64 + pad_x,
            (g.y + g.layout.height()) as f64 + pad_y,
            g.layout.height() as f64,
        );
        scene.fill(Fill::NonZero, origin, color(bg), None, &rect);
    }
    for deco in [b.glyph.as_ref().map(|g| (&g.layout, g.x, g.y)), b.bullet.as_ref().map(|d| (&d.layout, d.x, d.y))]
        .into_iter()
        .flatten()
    {
        draw_layout(scene, deco.0, origin * Affine::translate((deco.1 as f64, deco.2 as f64)));
    }
    if let Some(chip) = &b.chip {
        let rect = RoundedRect::from_rect(chip.chip_bounds(), 4.0);
        let fg = chip.layout.lines().next().and_then(|l| {
            l.items().find_map(|i| match i {
                PositionedLayoutItem::GlyphRun(r) => Some(r.style().brush.color()),
                _ => None,
            })
        });
        if let Some(fg) = fg {
            scene.stroke(&Stroke::new(1.0), origin, color(fg.with_alpha(0.6)), None, &rect);
        }
        draw_layout(scene, &chip.layout, origin * Affine::translate((chip.x as f64, chip.y as f64)));
    }
}
