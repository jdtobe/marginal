//! Lays out a [`Document`] with a [`Theme`]: one parley layout per leaf block, syntax glyphs in
//! the margin, and a mapping between source offsets and displayed text so the caret can live in
//! source coordinates while syntax is hidden or revealed around it.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use marginal_doc::{Align, Block, BlockKind, ContainerKind, Document, InlineStyle, Segment, SegmentKind, TableShape};
use marginal_highlight::Highlighter;
use marginal_theme::{Appearance, Color, Node, Style, TextAlign, TextTransform, Theme, is_icon_glyph, restates_syntax};
use parley::editing::Cursor;
use parley::{
    Affinity, Alignment, AlignmentOptions, FontContext, FontFamily, FontStyle, FontWeight, Layout, LayoutContext,
    LineHeight, StyleProperty,
};
use vello::kurbo::Rect;

/// Text color carried through parley layouts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Brush(pub Color);

impl Default for Brush {
    fn default() -> Self {
        Self(Color::BLACK)
    }
}

impl Brush {
    pub fn color(self) -> Color {
        self.0
    }
}

pub struct Fonts {
    pub font_cx: FontContext,
    pub layout_cx: LayoutContext<Brush>,
    /// Theme font files already registered.
    loaded: HashSet<std::path::PathBuf>,
}

impl Fonts {
    pub fn new() -> Self {
        let mut font_cx = FontContext::new();
        for &data in marginal_theme::BUNDLED_FONTS {
            font_cx.collection.register_fonts(parley::fontique::Blob::new(std::sync::Arc::new(data)), None);
        }
        Self { font_cx, layout_cx: LayoutContext::new(), loaded: HashSet::new() }
    }

    /// Registers the fonts a theme ships with (`@font-face` with local files), under the family
    /// names the theme uses. WOFF and WOFF2 are decoded; the first readable source of each
    /// face wins. Already-registered files are skipped.
    pub fn load_theme_fonts(&mut self, theme: &Theme) {
        let Some(base) = &theme.base_dir else { return };
        for face in &theme.font_faces {
            for source in &face.sources {
                let path = base.join(source);
                if self.loaded.contains(&path) {
                    break;
                }
                let Ok(bytes) = std::fs::read(&path) else { continue };
                let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
                let data = match ext.as_str() {
                    "woff2" => wuff::decompress_woff2(&bytes).ok(),
                    "woff" => wuff::decompress_woff1(&bytes).ok(),
                    _ => Some(bytes),
                };
                let Some(data) = data else { continue };
                let rename =
                    parley::fontique::FontInfoOverride { family_name: Some(&face.family), ..Default::default() };
                let registered = self
                    .font_cx
                    .collection
                    .register_fonts(parley::fontique::Blob::new(std::sync::Arc::new(data)), Some(rename));
                if !registered.is_empty() {
                    self.loaded.insert(path);
                    break;
                }
            }
        }
    }
}

/// A theme's `::before`/`::after` box, placed in document coordinates.
pub enum Ornament {
    Text { layout: Box<Layout<Brush>>, x: f32, y: f32, background: Option<Color>, radius: f32, padding: [f32; 4] },
    Shape { rect: (f32, f32, f32, f32), color: Option<Color>, border: Option<marginal_theme::Border>, radius: f32 },
}

/// One stretch of displayed text and the source bytes it came from.
#[derive(Clone, Debug)]
struct MapPiece {
    display: Range<usize>,
    source: Range<usize>,
    /// Display text equals source text byte for byte, so offsets inside map 1:1.
    exact: bool,
}

/// A block's marker in the margin, e.g. `##`, `>`, `- [ ]`, ```` ``` ````.
pub struct Glyph {
    pub layout: Layout<Brush>,
    pub x: f32,
    pub y: f32,
    /// A background behind the glyph (a pill), when the theme gives `.glyph` one.
    pub pill: Option<Color>,
}

pub struct Decoration {
    pub layout: Layout<Brush>,
    pub x: f32,
    pub y: f32,
}

impl Decoration {
    /// The outline drawn around a chip, which is also its click target.
    pub fn chip_bounds(&self) -> Rect {
        let pad = 6.0;
        Rect::new(
            (self.x - pad) as f64,
            (self.y - 2.0) as f64,
            (self.x + self.layout.width() + pad) as f64,
            (self.y + self.layout.height() + 2.0) as f64,
        )
    }
}

pub struct BlockView {
    pub layout: Layout<Brush>,
    map: Vec<MapPiece>,
    display_len: usize,
    /// Top-left of the text.
    pub text_x: f32,
    pub text_y: f32,
    /// The block's box, including padding, for backgrounds and hit testing.
    pub top: f32,
    pub bottom: f32,
    pub box_x: f32,
    pub box_width: f32,
    pub style: Style,
    pub glyph: Option<Glyph>,
    pub bullet: Option<Decoration>,
    pub chip: Option<Decoration>,
    pub quote_bars: Vec<(f32, f32, Color)>,
    pub rule: bool,
    /// Set for table cells, which are laid out as blocks of their own.
    pub cell: Option<CellFrame>,
    /// The theme's decorations around this block.
    pub ornaments: Vec<Ornament>,
}

/// A table cell's position and border colors.
#[derive(Clone, Copy, Debug)]
pub struct CellFrame {
    /// The table's block index in the document.
    pub table: usize,
    pub row: usize,
    pub col: usize,
    pub border: Color,
    /// The line under the header row.
    pub header_rule: Color,
}

impl BlockView {
    /// Display index for a source offset. Offsets inside hidden syntax snap to its edges.
    pub fn display_index(&self, offset: usize) -> usize {
        display_index_in(&self.map, offset, self.display_len)
    }

    /// Source offset for a display index.
    pub fn source_offset(&self, index: usize, affinity: Affinity) -> usize {
        let candidates: Vec<&MapPiece> = self
            .map
            .iter()
            .filter(|p| !p.display.is_empty() && p.display.start <= index && index <= p.display.end)
            .collect();
        // At a boundary, upstream affinity keeps the caret with the text before it.
        let piece = match affinity {
            Affinity::Upstream => candidates.iter().find(|p| p.display.end == index).or(candidates.first()),
            Affinity::Downstream => candidates.iter().find(|p| p.display.start == index).or(candidates.last()),
        };
        match piece {
            Some(p) if p.exact => p.source.start + (index - p.display.start),
            Some(p) if index == p.display.start => p.source.start,
            Some(p) => p.source.end,
            None => self.map.last().map_or(0, |p| p.source.end),
        }
    }

    pub fn source_range(&self) -> Range<usize> {
        let start = self.map.first().map_or(0, |p| p.source.start);
        let end = self.map.last().map_or(start, |p| p.source.end);
        start..end
    }
}

/// Which syntax is currently shown inline.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reveal {
    pub caret: Option<usize>,
    /// Holding ⌥: show every inline mark in the document.
    pub xray: bool,
    /// Uncommitted IME composition, shown inline at the caret.
    pub preedit: Option<String>,
}

pub struct CaretBox {
    pub x: f32,
    pub y: f32,
    pub height: f32,
    pub advance: f32,
}

pub struct DocView {
    pub blocks: Vec<BlockView>,
    pub height: f32,
}

struct Ctx<'a> {
    theme: &'a Theme,
    appearance: Appearance,
    body: Style,
}

impl Ctx<'_> {
    fn style(&self, element: &str, classes: &[&str], parent: &Style) -> Style {
        self.theme.style(self.appearance, element, classes, Some(parent))
    }

    /// The style of the last node of `path`, with its ancestors known (for `blockquote p`,
    /// `li > p`, `tr:nth-child(2n) td`…).
    fn style_at(&self, path: &[Node], parent: &Style) -> Style {
        self.theme.style_at(self.appearance, path, Some(parent))
    }
}

/// Where blocks, quotes, lists and list items sit among their siblings, as they would in HTML:
/// a quote's children are its paragraphs, lists and nested quotes; a list's are its items.
#[derive(Default)]
struct Positions {
    /// For each block: its (index, count) among its parent's children.
    leaf: Vec<(usize, usize)>,
    /// For quotes and footnotes (by container id), and for lists (by their first item's id).
    container: HashMap<usize, (usize, usize)>,
    /// For list items: (index, count) within their list, and the list's key.
    item: HashMap<usize, (usize, usize, usize)>,
}

fn positions(doc: &Document) -> Positions {
    #[derive(Clone, Copy, PartialEq)]
    enum Unit {
        Leaf(usize),
        Container(usize),
        /// A list, keyed by its first item's id, and whether it is ordered.
        List(usize, bool),
    }
    let mut children: HashMap<Option<usize>, Vec<Unit>> = HashMap::new();
    let mut items: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut list_of: HashMap<usize, usize> = HashMap::new();
    for (i, block) in doc.blocks().iter().enumerate() {
        for depth in 0..=block.containers.len() {
            let parent = depth.checked_sub(1).map(|d| block.containers[d].id);
            let siblings = children.entry(parent).or_default();
            let unit = match block.containers.get(depth) {
                None => Unit::Leaf(i),
                Some(c) => match &c.kind {
                    ContainerKind::ListItem { ordered, .. } => {
                        let key = match (list_of.get(&c.id), siblings.last()) {
                            (Some(&k), _) => k,
                            (None, Some(&Unit::List(k, o))) if o == ordered.is_some() => k,
                            _ => c.id,
                        };
                        list_of.insert(c.id, key);
                        let list = items.entry(key).or_default();
                        if list.last() != Some(&c.id) {
                            list.push(c.id);
                        }
                        Unit::List(key, ordered.is_some())
                    }
                    _ => Unit::Container(c.id),
                },
            };
            if siblings.last() != Some(&unit) {
                siblings.push(unit);
            }
        }
    }
    let mut out = Positions { leaf: vec![(0, 1); doc.blocks().len()], ..Default::default() };
    for units in children.values() {
        let count = units.len();
        for (index, unit) in units.iter().enumerate() {
            match *unit {
                Unit::Leaf(i) => out.leaf[i] = (index, count),
                Unit::Container(id) | Unit::List(id, _) => {
                    out.container.insert(id, (index, count));
                }
            }
        }
    }
    for (key, list) in &items {
        for (index, id) in list.iter().enumerate() {
            out.item.insert(*id, (index, list.len(), *key));
        }
    }
    out
}

pub fn layout_document(
    doc: &Document,
    theme: &Theme,
    appearance: Appearance,
    fonts: &mut Fonts,
    width: f32,
    reveal: &Reveal,
    highlighter: &mut Highlighter,
) -> DocView {
    let body = theme.style(appearance, "body", &[], None);
    let cx = Ctx { theme, appearance, body: body.clone() };
    let px = |name: &str, default: f32| {
        theme.var(appearance, name).and_then(|v| v.trim_end_matches("px").trim().parse().ok()).unwrap_or(default)
    };
    let gutter = 24.0;
    let margin = px("--marginal-margin", 96.0);
    let glyph_right = theme.var(appearance, "--marginal-glyph-side").is_some_and(|s| s.trim() == "right");
    // The theme's text column: Marginal's own setting, else the page's max-width (Typora
    // themes set `#write { max-width }`), else 640.
    let measure =
        px("--marginal-measure", body.max_width.unwrap_or(640.0)).min((width - margin - gutter * 2.0).max(200.0));
    let left = ((width - margin - measure) / 2.0).max(gutter);
    let content_x = if glyph_right { left } else { left + margin };

    let src = doc.text();
    let mut views: Vec<BlockView> = Vec::with_capacity(doc.blocks().len());
    let mut y = 48.0_f32;
    let mut prev_margin = 0.0_f32;
    let mut seen_containers = HashSet::new();
    let mut prev_quote_ids: Vec<usize> = Vec::new();

    // A caret between blocks (e.g. after pressing Enter twice) gets an empty line of its own.
    let gap = reveal
        .caret
        .filter(|&c| !doc.blocks().iter().any(|b| b.range.start <= c && c <= b.range.end))
        .map(|c| (c, doc.blocks().partition_point(|b| b.range.start <= c)));
    let place_gap = |at: usize, views: &mut Vec<BlockView>, y: &mut f32, prev_margin: &mut f32, fonts: &mut Fonts| {
        let Some((c, gi)) = gap else { return };
        if gi != at {
            return;
        }
        let style = cx.style("p", &[], &body);
        if !views.is_empty() {
            *y += prev_margin.max(style.margin_top);
        }
        let layout = simple_layout(fonts, "", &style, Some(measure));
        let height = layout.height().max(style.font_size * style.line_height);
        views.push(BlockView {
            layout,
            map: vec![MapPiece { display: 0..0, source: c..c, exact: false }],
            display_len: 0,
            text_x: content_x,
            text_y: *y,
            top: *y,
            bottom: *y + height,
            box_x: content_x,
            box_width: measure,
            style: style.clone(),
            glyph: None,
            bullet: None,
            chip: None,
            quote_bars: Vec::new(),
            rule: false,
            cell: None,
            ornaments: Vec::new(),
        });
        *y += height;
        *prev_margin = style.margin_bottom;
    };

    let positions = positions(doc);
    // Heading numbers for themes that number headings with `counter(h2)` and friends.
    let mut heading_numbers = [0u64; 6];
    for (index, block) in doc.blocks().iter().enumerate() {
        if let BlockKind::Heading { level, .. } = block.kind {
            let l = (level as usize).clamp(1, 6);
            heading_numbers[l - 1] += 1;
            heading_numbers[l..].iter_mut().for_each(|n| *n = 0);
        }
        // Theme decorations of quotes this block opens or closes, laid out as their own lines.
        // Each with the left edge of the quote's box, which absolute offsets are measured from.
        let mut container_decorations: Vec<(marginal_theme::Decoration, f32)> = Vec::new();
        let last_in = |id: usize| doc.blocks().get(index + 1).is_none_or(|n| !n.containers.iter().any(|c| c.id == id));
        place_gap(index, &mut views, &mut y, &mut prev_margin, fonts);
        // Container chain: indentation, inherited style, glyphs, bullets.
        let mut style = body.clone();
        let mut path = vec![Node::new("body")];
        let at = |pos: Option<&(usize, usize)>, node: Node| match pos {
            Some(&(i, n)) => node.at(i, n),
            None => node,
        };
        let mut indent = 0.0_f32;
        let mut glyph_parts: Vec<String> = Vec::new();
        let mut bullet: Option<(String, Style)> = None;
        let mut quote_bars_x: Vec<(f32, Color, usize)> = Vec::new();
        for c in &block.containers {
            let first = seen_containers.insert(c.id);
            let marker = src[c.marker.clone()].trim().to_owned();
            match &c.kind {
                ContainerKind::Quote => {
                    path.push(at(positions.container.get(&c.id), Node::new("blockquote")));
                    style = cx.style_at(&path, &style);
                    for mut d in theme.decorations(appearance, &path, &style) {
                        let keep = match d.pseudo {
                            marginal_theme::PseudoElement::Before => first,
                            marginal_theme::PseudoElement::After => last_in(c.id),
                        };
                        if keep {
                            d.block |= !d.absolute;
                            container_decorations.push((d, content_x + indent));
                        }
                    }
                    if let Some(b) = style.border_left {
                        quote_bars_x.push((indent, b.color, c.id));
                    }
                    indent += style.padding[3] + style.border_left.map_or(0.0, |b| b.width);
                    glyph_parts.push(">".into());
                }
                ContainerKind::ListItem { ordered, task } => {
                    let classes: &[&str] = match task {
                        Some(true) => &["task", "task-done", "md-task-list-item", "task-list-item"],
                        Some(false) => &["task", "md-task-list-item", "task-list-item"],
                        None => &[],
                    };
                    let (item_index, item_count, list) = positions.item.get(&c.id).copied().unwrap_or((0, 1, c.id));
                    let list_element = if ordered.is_some() { "ol" } else { "ul" };
                    path.push(at(positions.container.get(&list), Node::new(list_element)));
                    style = cx.style_at(&path, &style);
                    path.push(Node::new("li").with_classes(classes).at(item_index, item_count));
                    style = cx.style_at(&path, &style);
                    let mark = match (task, ordered, style.list_style.as_deref()) {
                        (Some(true), _, _) => "[x]".to_owned(),
                        (Some(false), _, _) => "[ ]".to_owned(),
                        // A theme's `list-style-type` wins over Marginal's own marker settings.
                        (None, _, Some(kind)) => list_marker(kind, ordered.unwrap_or(item_index as u64 + 1)),
                        (None, Some(n), None) => {
                            match theme.var(appearance, "--marginal-ol-style").as_deref().map(str::trim) {
                                Some("circled") => circled(*n),
                                Some("padded") => format!("{n:02}"),
                                _ => format!("{n}."),
                            }
                        }
                        (None, None, None) => theme
                            .var(appearance, "--marginal-bullet")
                            .map(|b| b.trim().trim_matches(['"', '\'']).to_owned())
                            .filter(|b| !b.is_empty())
                            .unwrap_or_else(|| "•".to_owned()),
                    };
                    // A theme's `li::before` text replaces the bullet.
                    let custom = theme
                        .decorations(appearance, &path, &style)
                        .into_iter()
                        .find(|d| d.pseudo == marginal_theme::PseudoElement::Before && !d.absolute)
                        .map(|d| (d.text(|_, _| String::new()), d.style))
                        .filter(|(t, _)| !t.trim().is_empty() && !is_icon_glyph(t) && !restates_syntax(t));
                    if first && let Some((text, deco_style)) = custom {
                        bullet = Some((text.trim().to_owned(), deco_style));
                        if reveal.xray {
                            glyph_parts.push(marker);
                        }
                    } else if first {
                        let mut bullet_style = cx.style("span", &["bullet"], &style);
                        bullet_style.strikethrough = false;
                        if task.is_some() {
                            bullet_style.color = cx.style("code", &[], &style).color;
                        } else if ordered.is_some() {
                            // Numbers sit back in the glyph gray so the text stays loudest.
                            bullet_style.color = cx.style("span", &["glyph"], &style).color;
                        }
                        bullet = Some((mark, bullet_style));
                        // List markers render inline; the raw syntax only shows under x-ray.
                        if reveal.xray {
                            glyph_parts.push(marker);
                        }
                    }
                    indent += style.font_size * 2.6;
                }
                ContainerKind::Footnote { label } => {
                    path.push(at(positions.container.get(&c.id), Node::new("footnote")));
                    style = cx.style_at(&path, &style);
                    if first {
                        glyph_parts.push(format!("[^{label}]:"));
                    }
                }
            }
        }

        let (element, block_glyph): (String, Option<String>) = match &block.kind {
            BlockKind::Paragraph => ("p".into(), None),
            BlockKind::Heading { level, setext } => (
                format!("h{level}"),
                Some(if *setext {
                    if *level == 1 { "===".into() } else { "---".into() }
                } else {
                    "#".repeat(*level as usize)
                }),
            ),
            BlockKind::CodeBlock { fenced, .. } => {
                ("pre".into(), Some(if *fenced { "```".into() } else { "⇥".into() }))
            }
            BlockKind::Rule => ("hr".into(), Some(src[block.range.clone()].trim().to_owned())),
            BlockKind::Html => ("pre".into(), Some("<>".into())),
            BlockKind::Table => ("table".into(), Some("|".into())),
            BlockKind::FrontMatter => ("pre".into(), Some("---".into())),
        };
        path.push(at(positions.leaf.get(index), Node::new(&element)));
        style = cx.style_at(&path, &style);
        if let Some(g) = block_glyph {
            glyph_parts.push(g);
        }

        let collapsed = prev_margin.max(style.margin_top);
        if index > 0 {
            y += collapsed;
        }
        let top = y;
        let box_x = content_x + indent + style.margin_left;
        let box_width = (measure - indent - style.margin_left - style.margin_right).max(40.0);
        let border = |b: Option<marginal_theme::Border>| b.map_or(0.0, |b| b.width);
        let (border_top, border_left, border_right) =
            (border(style.border_top), border(style.border_left), border(style.border_right));
        let text_width = (box_width - style.padding[1] - style.padding[3] - border_left - border_right).max(20.0);
        let text_x = box_x + style.padding[3] + border_left;
        let text_y = top + style.padding[0] + border_top;

        let table = block.table.as_deref().filter(|_| block.kind == BlockKind::Table);
        if let Some(shape) = table.filter(|_| !reveal.xray) {
            let mut cells =
                layout_table(fonts, &cx, &path, src, block, index, shape, reveal, &style, box_x, box_width, top);
            if let Some(first) = cells.first_mut() {
                let baseline = first.layout.lines().next().map_or(style.font_size, |l| l.metrics().baseline);
                first.glyph = make_glyph(fonts, &cx, &glyph_parts, first.text_y + baseline, content_x, measure);
            }
            for cell in cells.iter_mut().filter(|c| c.cell.is_some_and(|f| f.col == 0)) {
                cell.quote_bars = quote_bars_x.iter().map(|&(x, color, _)| (content_x + x, cell.top, color)).collect();
            }
            prev_quote_ids = quote_bars_x.iter().map(|q| q.2).collect();
            y = cells.iter().map(|c| c.bottom).fold(top, f32::max);
            views.extend(cells);
            prev_margin = style.margin_bottom;
            continue;
        }
        // Under x-ray a table shows its source, pipes and all.
        let raw;
        let shown = if table.is_some() {
            raw = Block {
                segments: vec![Segment {
                    range: block.range.clone(),
                    kind: SegmentKind::Text { style: InlineStyle::RAW },
                }],
                ..block.clone()
            };
            &raw
        } else {
            block
        };
        let (display, map, mut spans) = build_display(src, shown, reveal);
        if let BlockKind::CodeBlock { info, .. } = &block.kind
            && let Some(lang) = info.split_whitespace().next()
        {
            spans.extend(
                highlighter.highlight(lang, &display).into_iter().map(|s| (s.range, Span::Highlight(s.capture))),
            );
        }
        let display = transform_text(display, style.text_transform);
        // The theme's ::before/::after boxes: block ones take lines of their own, inline ones
        // sit at the start of the first line or the end of the last.
        let counter = |name: &str, kind: Option<&str>| {
            let level = name.chars().rev().find(char::is_ascii_digit).and_then(|d| d.to_digit(10)).unwrap_or(1);
            let n = heading_numbers[(level as usize).clamp(1, 6) - 1];
            list_marker(kind.unwrap_or("decimal"), n).trim_end_matches('.').to_owned()
        };
        let mut decorations: Vec<(marginal_theme::Decoration, f32)> =
            theme.decorations(appearance, &path, &style).into_iter().map(|d| (d, box_x)).collect();
        decorations.extend(container_decorations);
        let mut ornaments = Vec::new();
        let (mut before_h, mut after) = (0.0_f32, Vec::new());
        let (mut inline_before, mut inline_after) = (Vec::new(), Vec::new());
        let mut indent_before = 0.0_f32;
        for (d, origin_x) in &decorations {
            let origin_x = *origin_x;
            let text = d.text(counter);
            if restates_syntax(&text) || is_icon_glyph(&text) {
                continue;
            }
            let s = &d.style;
            let border = s.border_bottom.or(s.border_top).or(s.border_left);
            let shape = d.is_shape();
            if shape && s.background.is_none() && border.is_none() {
                continue; // an invisible clearfix
            }
            let (w, h, layout) = if shape {
                let w = d.width.unwrap_or(if d.block { text_width } else { s.font_size * 0.5 });
                let h =
                    d.height.unwrap_or(if d.block { 0.0 } else { s.font_size * 0.5 }) + border.map_or(0.0, |b| b.width);
                (w, h, None)
            } else {
                let l = simple_layout(fonts, &text, s, None);
                (l.width() + s.padding[1] + s.padding[3], l.height() + s.padding[0] + s.padding[2], Some(l))
            };
            let make = |x: f32, y: f32, layout: Option<Layout<Brush>>| match layout {
                Some(layout) => Ornament::Text {
                    layout: Box::new(layout),
                    x,
                    y,
                    background: s.background,
                    radius: s.border_radius,
                    padding: s.padding,
                },
                None => {
                    Ornament::Shape { rect: (x, y, x + w, y + h), color: s.background, border, radius: s.border_radius }
                }
            };
            if d.absolute {
                let x = match (d.left, d.right) {
                    (Some(l), _) => origin_x + l,
                    (None, Some(r)) => box_x + box_width - r - w,
                    _ => text_x - w - s.margin_right,
                };
                ornaments.push(make(x, top + d.top.unwrap_or(0.0), layout));
            } else if d.block {
                let x = match s.text_align {
                    TextAlign::Center => text_x + (text_width - w) / 2.0,
                    TextAlign::End => text_x + text_width - w,
                    _ => text_x + s.margin_left,
                };
                if d.pseudo == marginal_theme::PseudoElement::Before {
                    ornaments.push(make(x, text_y + before_h + s.margin_top, layout));
                    before_h += s.margin_top + h + s.margin_bottom;
                } else {
                    after.push((x, s.margin_top, h + s.margin_top + s.margin_bottom, layout, w, h, s.clone(), border));
                }
            } else if d.pseudo == marginal_theme::PseudoElement::Before {
                inline_before.push((indent_before + s.margin_left, layout, w, h, s.clone(), border));
                indent_before += s.margin_left + w + s.margin_right;
            } else {
                inline_after.push((s.margin_left, layout, w, h, s.clone(), border));
            }
        }
        let text_y = text_y + before_h;
        let layout_style = if indent_before > 0.0 {
            Style { text_indent: style.text_indent + indent_before, ..style.clone() }
        } else {
            style.clone()
        };
        let layout = build_layout(fonts, &cx, &path, &display, &spans, &layout_style, text_width);
        let text_height = if block.kind == BlockKind::Rule { style.font_size * 0.5 } else { layout.height() };
        let mut bottom = text_y + text_height + style.padding[2];
        {
            let first = layout.lines().next().map(|l| *l.metrics());
            let last = layout.lines().last().map(|l| *l.metrics());
            let place_inline = |x: f32,
                                line: Option<parley::LineMetrics>,
                                layout: Option<Layout<Brush>>,
                                w: f32,
                                h: f32,
                                s: &Style,
                                border| {
                let (base, line_h) = line
                    .map_or((style.font_size, style.font_size * style.line_height), |m| (m.baseline, m.line_height));
                match layout {
                    Some(l) => {
                        let lb = l.lines().next().map_or(0.0, |m| m.metrics().baseline);
                        Ornament::Text {
                            layout: Box::new(l),
                            x,
                            y: text_y + base - lb,
                            background: s.background,
                            radius: s.border_radius,
                            padding: s.padding,
                        }
                    }
                    None => {
                        let y = text_y + line.map_or(0.0, |m| m.block_min_coord) + (line_h - h) / 2.0;
                        Ornament::Shape {
                            rect: (x, y, x + w, y + h),
                            color: s.background,
                            border,
                            radius: s.border_radius,
                        }
                    }
                }
            };
            for (dx, l, w, h, s, border) in inline_before {
                ornaments.push(place_inline(text_x + dx, first, l, w, h, &s, border));
            }
            let end_x = text_x + last.map_or(0.0, |m| m.advance + m.offset);
            let mut x = end_x;
            for (dx, l, w, h, s, border) in inline_after {
                ornaments.push(place_inline(x + dx, last, l, w, h, &s, border));
                x += dx + w;
            }
            for (x, margin_top, total, l, w, h, s, border) in after {
                let y = bottom + margin_top;
                ornaments.push(match l {
                    Some(layout) => Ornament::Text {
                        layout: Box::new(layout),
                        x,
                        y,
                        background: s.background,
                        radius: s.border_radius,
                        padding: s.padding,
                    },
                    None => Ornament::Shape {
                        rect: (x, y, x + w, y + h),
                        color: s.background,
                        border,
                        radius: s.border_radius,
                    },
                });
                bottom += total;
            }
        }
        let border_bottom = if block.kind == BlockKind::Rule { 0.0 } else { border(style.border_bottom) };

        let first_baseline = layout.lines().next().map_or(style.font_size, |l| l.metrics().baseline);
        let glyph = make_glyph(fonts, &cx, &glyph_parts, text_y + first_baseline, content_x, measure);
        let bullet = bullet.map(|(mark, bstyle)| {
            let bl = simple_layout(fonts, &mark, &bstyle, None);
            let baseline = bl.lines().next().map_or(0.0, |l| l.metrics().baseline);
            // Right-aligned against the text so wide markers like `[x]` keep a gap.
            Decoration {
                x: box_x - bl.width() - bstyle.font_size * 0.6,
                y: text_y + first_baseline - baseline,
                layout: bl,
            }
        });
        let caret_inside = reveal.caret.is_some_and(|c| block.range.start <= c && c <= block.range.end);
        let chip = match &block.kind {
            BlockKind::CodeBlock { info, fenced: true } if caret_inside => {
                let label = if info.is_empty() {
                    "plain ▾".to_owned()
                } else {
                    format!("{} ▾", info.split_whitespace().next().unwrap_or(""))
                };
                let chip_style = cx.style("span", &["chip"], &cx.style("span", &["glyph"], &style));
                let cl = simple_layout(fonts, &label, &chip_style, None);
                Some(Decoration { x: box_x + box_width - cl.width() - 10.0, y: top + 6.0, layout: cl })
            }
            _ => None,
        };

        // Quote bars run continuously through consecutive blocks of the same quote.
        let quote_bars = quote_bars_x
            .iter()
            .map(|&(x, color, id)| {
                let joined = prev_quote_ids.contains(&id);
                (content_x + x, if joined { top - collapsed } else { top }, color)
            })
            .collect::<Vec<_>>();
        prev_quote_ids = quote_bars_x.iter().map(|q| q.2).collect();

        views.push(BlockView {
            display_len: display.len(),
            layout,
            map,
            text_x,
            text_y,
            top,
            bottom,
            box_x,
            box_width,
            style: style.clone(),
            glyph,
            bullet,
            chip,
            quote_bars,
            rule: block.kind == BlockKind::Rule,
            cell: None,
            ornaments,
        });
        y = bottom + border_bottom;
        prev_margin = style.margin_bottom;
    }

    place_gap(doc.blocks().len(), &mut views, &mut y, &mut prev_margin, fonts);

    DocView { blocks: views, height: y + prev_margin + 240.0 }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Span {
    Content(InlineStyle),
    Syntax,
    Preedit,
    /// A tree-sitter capture in a code block, like `keyword` or `function.method`.
    Highlight(&'static str),
}

/// A block's display text, its map back to the source, and its styled spans.
type Display = (String, Vec<MapPiece>, Vec<(Range<usize>, Span)>);

fn build_display(src: &str, block: &Block, reveal: &Reveal) -> Display {
    let revealed: HashSet<usize> = match reveal.caret {
        _ if reveal.xray => (0..block.inlines.len()).collect(),
        Some(c) => block.inlines_at(c).into_iter().collect(),
        None => HashSet::new(),
    };
    let mut display = String::new();
    let mut map = Vec::new();
    let mut spans = Vec::new();
    let mut push = |display: &mut String, text: &str, source: Range<usize>, exact: bool, span: Option<Span>| {
        let start = display.len();
        display.push_str(text);
        map.push(MapPiece { display: start..display.len(), source, exact });
        if let Some(span) = span
            && !text.is_empty()
        {
            spans.push((start..display.len(), span));
        }
    };

    // Trailing newlines of the last visible text (code blocks) are hidden.
    let last_visible = block.segments.iter().rposition(|s| !matches!(s.kind, SegmentKind::Syntax { .. }));
    for (i, seg) in block.segments.iter().enumerate() {
        let text = &src[seg.range.clone()];
        match &seg.kind {
            SegmentKind::Text { style } => {
                let trailing = Some(i) == last_visible && text.ends_with('\n');
                if trailing || style.contains(InlineStyle::RAW) {
                    let trimmed = text.trim_end_matches(['\n', '\r']);
                    let cut = seg.range.start + trimmed.len();
                    push(&mut display, trimmed, seg.range.start..cut, true, Some(Span::Content(*style)));
                    push(&mut display, "", cut..seg.range.end, false, None);
                } else {
                    push(&mut display, text, seg.range.clone(), true, Some(Span::Content(*style)));
                }
            }
            SegmentKind::Replaced { text, style } => {
                push(&mut display, text, seg.range.clone(), false, Some(Span::Content(*style)))
            }
            SegmentKind::Break { hard } => {
                push(&mut display, if *hard { "\n" } else { " " }, seg.range.clone(), false, None)
            }
            SegmentKind::Syntax { owner: Some(owner) } if revealed.contains(owner) => {
                push(&mut display, text, seg.range.clone(), true, Some(Span::Syntax))
            }
            SegmentKind::Syntax { .. } => push(&mut display, "", seg.range.clone(), false, None),
        }
    }
    if let (Some(preedit), Some(c)) = (reveal.preedit.as_deref(), reveal.caret)
        && block.range.start <= c
        && c <= block.range.end
        && !preedit.is_empty()
    {
        insert_preedit(&mut display, &mut map, &mut spans, c, preedit);
    }
    (display, map, spans)
}

fn display_index_in(map: &[MapPiece], offset: usize, len: usize) -> usize {
    let mut best = 0;
    for p in map {
        if offset < p.source.start {
            break;
        }
        if offset <= p.source.end {
            best = if p.exact {
                p.display.start + (offset - p.source.start)
            } else if offset == p.source.start {
                p.display.start
            } else {
                p.display.end
            };
            if offset < p.source.end {
                break;
            }
        } else {
            best = p.display.end;
        }
    }
    best.min(len)
}

/// Splices IME composition text into the display at the caret, shifting everything after it.
fn insert_preedit(
    display: &mut String,
    map: &mut Vec<MapPiece>,
    spans: &mut Vec<(Range<usize>, Span)>,
    caret: usize,
    preedit: &str,
) {
    let at = display_index_in(map, caret, display.len());
    let len = preedit.len();
    display.insert_str(at, preedit);
    let mut pieces = Vec::with_capacity(map.len() + 2);
    for p in map.drain(..) {
        if p.display.start >= at && !(p.display.is_empty() && p.source.end <= caret) {
            pieces.push(MapPiece { display: p.display.start + len..p.display.end + len, ..p });
        } else if p.display.end > at {
            // An exact piece straddling the caret splits around the composition.
            let cut = p.source.start + (at - p.display.start);
            pieces.push(MapPiece { display: p.display.start..at, source: p.source.start..cut, exact: true });
            pieces.push(MapPiece { display: at + len..p.display.end + len, source: cut..p.source.end, exact: true });
        } else {
            pieces.push(p);
        }
    }
    let pos =
        pieces.iter().position(|p| p.display.start >= at + len && p.source.start >= caret).unwrap_or(pieces.len());
    pieces.insert(pos, MapPiece { display: at..at + len, source: caret..caret, exact: false });
    *map = pieces;
    let mut out = Vec::with_capacity(spans.len() + 2);
    for (r, span) in spans.drain(..) {
        if r.start >= at {
            out.push((r.start + len..r.end + len, span));
        } else if r.end > at {
            out.push((r.start..at, span));
            out.push((at + len..r.end + len, span));
        } else {
            out.push((r, span));
        }
    }
    out.push((at..at + len, Span::Preedit));
    *spans = out;
}

/// Inline marker for ordered items: ① … ⑳, then plain numbers.
/// A list marker for a CSS `list-style-type`; empty for `none`.
fn list_marker(kind: &str, n: u64) -> String {
    let alpha = |upper: bool| {
        let mut s = String::new();
        let mut k = n;
        while k > 0 {
            k -= 1;
            s.insert(0, (if upper { b'A' } else { b'a' } + (k % 26) as u8) as char);
            k /= 26;
        }
        s
    };
    let roman = |upper: bool| {
        let mut k = n;
        let mut s = String::new();
        for (v, r) in [
            (1000, "m"),
            (900, "cm"),
            (500, "d"),
            (400, "cd"),
            (100, "c"),
            (90, "xc"),
            (50, "l"),
            (40, "xl"),
            (10, "x"),
            (9, "ix"),
            (5, "v"),
            (4, "iv"),
            (1, "i"),
        ] {
            while k >= v {
                s.push_str(r);
                k -= v;
            }
        }
        if upper { s.to_uppercase() } else { s }
    };
    match kind {
        "none" => String::new(),
        "disc" => "•".into(),
        "circle" => "◦".into(),
        "square" => "▪".into(),
        "decimal-leading-zero" => format!("{n:02}."),
        "lower-alpha" | "lower-latin" => format!("{}.", alpha(false)),
        "upper-alpha" | "upper-latin" => format!("{}.", alpha(true)),
        "lower-roman" => format!("{}.", roman(false)),
        "upper-roman" => format!("{}.", roman(true)),
        _ => format!("{n}."),
    }
}

fn circled(n: u64) -> String {
    match n {
        1..=20 => char::from_u32(0x2460 + n as u32 - 1).map_or(format!("{n}."), String::from),
        _ => format!("{n}."),
    }
}

fn transform_text(text: String, transform: TextTransform) -> String {
    let changed = match transform {
        TextTransform::None => return text,
        TextTransform::Uppercase => text.to_uppercase(),
        TextTransform::Lowercase => text.to_lowercase(),
    };
    // Only when byte offsets are preserved; otherwise the source map would break.
    if changed.len() == text.len() && changed.char_indices().map(|(i, _)| i).eq(text.char_indices().map(|(i, _)| i)) {
        changed
    } else {
        text
    }
}

fn inline_style(cx: &Ctx, path: &[Node], base: &Style, flags: InlineStyle) -> Style {
    let mut style = base.clone();
    let mut path = path.to_vec();
    for (flag, element) in [
        (InlineStyle::STRONG, "strong"),
        (InlineStyle::EMPHASIS, "em"),
        (InlineStyle::STRIKE, "del"),
        (InlineStyle::LINK, "a"),
        (InlineStyle::CODE, "code"),
        (InlineStyle::MATH, "code"),
        (InlineStyle::HTML, "code"),
        (InlineStyle::SUPERSCRIPT, "sup"),
        (InlineStyle::IMAGE_ALT, "em"),
    ] {
        if flags.contains(flag) {
            path.push(Node::new(element));
            let next = cx.style_at(&path, &style);
            style = Style {
                // Inline elements don't take block decorations.
                background: next.background,
                margin_top: 0.0,
                margin_bottom: 0.0,
                padding: [0.0; 4],
                border_left: None,
                border_bottom: None,
                ..next
            };
        }
    }
    style
}

/// The margin glyph for a block, its baseline at `baseline_y`. Themes choose the side
/// (`--marginal-glyph-side`) and whether it shows raw syntax or a word (`--marginal-glyph-style:
/// label`); a `.glyph` background draws it as a pill.
fn make_glyph(
    fonts: &mut Fonts,
    cx: &Ctx,
    parts: &[String],
    baseline_y: f32,
    content_x: f32,
    measure: f32,
) -> Option<Glyph> {
    let var = |name: &str| cx.theme.var(cx.appearance, name).map(|v| v.trim().to_owned());
    let text = if var("--marginal-glyph-style").as_deref() == Some("label") {
        glyph_label(parts.last()?)
    } else {
        (!parts.is_empty()).then(|| parts.join(" "))?
    };
    let style = cx.style("span", &["glyph"], &cx.body);
    let layout = simple_layout(fonts, &text, &style, None);
    let baseline = layout.lines().next().map_or(0.0, |l| l.metrics().baseline);
    let pad = if style.background.is_some() { style.font_size * 0.7 } else { 0.0 };
    let x = if var("--marginal-glyph-side").as_deref() == Some("right") {
        content_x + measure + 24.0 + pad
    } else {
        content_x - 22.0 - pad - layout.width()
    };
    Some(Glyph { x, y: baseline_y - baseline, layout, pill: style.background })
}

/// A word for a block's syntax, for themes that label blocks instead of showing raw Markdown.
fn glyph_label(syntax: &str) -> String {
    let label = match syntax {
        ">" => "Quote",
        "```" | "⇥" => "Code",
        "|" => "Table",
        "<>" => "HTML",
        "===" => "H1",
        "---" | "***" | "___" => "Rule",
        s if s.starts_with("[^") => "Note",
        s if !s.is_empty() && s.chars().all(|c| c == '#') => return format!("H{}", s.len()),
        s if s.starts_with('-') || s.starts_with('*') || s.starts_with('+') || s.ends_with('.') || s.ends_with(')') => {
            "List"
        }
        s => return s.to_owned(),
    };
    label.to_owned()
}

/// Lays out a table as a grid of cell views, one per cell, in row-major order. Each cell's
/// source map covers just its text, so the caret moves cell to cell and never into the pipes.
#[allow(clippy::too_many_arguments)]
fn layout_table(
    fonts: &mut Fonts,
    cx: &Ctx,
    table_path: &[Node],
    src: &str,
    block: &Block,
    index: usize,
    shape: &TableShape,
    reveal: &Reveal,
    table_style: &Style,
    box_x: f32,
    box_width: f32,
    top: f32,
) -> Vec<BlockView> {
    let cols = shape.aligns.len();
    let body_rows = shape.rows.len().saturating_sub(1);
    // The HTML a table becomes: table > thead|tbody > tr > th|td, so themes' row striping
    // (`tbody tr:nth-child(2n)`) and first/last-cell rules apply. A row's background shows
    // through cells that don't set their own.
    let cell_path = |r: usize, c: usize| {
        let mut path = table_path.to_vec();
        let (section, row, rows) = if r == 0 { ("thead", 0, 1) } else { ("tbody", r - 1, body_rows) };
        path.push(Node::new(section).at(usize::from(r > 0), if body_rows > 0 { 2 } else { 1 }));
        path.push(Node::new("tr").at(row, rows));
        let tr = path.clone();
        path.push(Node::new(if r == 0 { "th" } else { "td" }).at(c, cols));
        (tr, path)
    };
    let styles: Vec<Vec<(Style, Vec<Node>)>> = (0..shape.rows.len())
        .map(|r| {
            (0..cols)
                .map(|c| {
                    let (tr_path, path) = cell_path(r, c);
                    let tr = cx.style_at(&tr_path, table_style);
                    let mut s = cx.style_at(&path, &tr);
                    if s.padding == [0.0; 4] {
                        s.padding = [4.0, 10.0, 4.0, 10.0];
                    }
                    s.background = s.background.or(tr.background);
                    (s, path)
                })
                .collect()
        })
        .collect();
    let td = styles.get(1).unwrap_or(&styles[0])[0].0.clone();
    let th = styles[0][0].0.clone();
    let border = td.border_bottom.map_or(td.color.with_alpha(0.25), |b| b.color);
    let header_rule = th.border_bottom.map_or(border, |b| b.color);

    struct Prepared {
        display: String,
        map: Vec<MapPiece>,
        spans: Vec<(Range<usize>, Span)>,
        layout: Layout<Brush>,
    }
    let mut grid: Vec<Vec<Option<Prepared>>> = Vec::new();
    for (row, row_styles) in shape.rows.iter().zip(&styles) {
        let mut cells = Vec::new();
        for (c, (style, path)) in row_styles.iter().enumerate() {
            let Some(range) = row.get(c).map(|cr| marginal_doc::cell_content(src, cr.clone())) else {
                cells.push(None);
                continue;
            };
            let (display, mut map, spans) = build_display(src, &block.sub_block(range.clone()), reveal);
            if map.is_empty() {
                map.push(MapPiece { display: 0..0, source: range.clone(), exact: false });
            }
            let layout = build_layout(fonts, cx, path, &display, &spans, style, 1e6);
            cells.push(Some(Prepared { display, map, spans, layout }));
        }
        grid.push(cells);
    }

    // Columns take their natural width, shrinking proportionally (and wrapping) when the table
    // is wider than the page.
    let pad_x = |s: &Style| s.padding[1] + s.padding[3];
    let min_width = td.font_size * 2.0 + pad_x(&td);
    let mut widths: Vec<f32> = (0..cols)
        .map(|c| {
            grid.iter()
                .enumerate()
                .filter_map(|(r, row)| {
                    let p = row[c].as_ref()?;
                    Some(p.layout.width() + pad_x(&styles[r][c].0))
                })
                .fold(min_width, f32::max)
        })
        .collect();
    let total: f32 = widths.iter().sum();
    if total > box_width {
        let scale = box_width / total;
        for w in &mut widths {
            *w = (*w * scale).max(min_width);
        }
    }

    let mut views = Vec::new();
    let mut y = top;
    for (r, row) in grid.into_iter().enumerate() {
        let mut prepared = Vec::new();
        for (c, cell) in row.into_iter().enumerate() {
            let Some(mut p) = cell else { continue };
            let (style, path) = &styles[r][c];
            let inner = widths[c] - pad_x(style);
            // The Markdown column alignment wins; otherwise the theme's text-align applies.
            let alignment = match (shape.aligns[c], style.text_align) {
                (Align::Center, _) | (Align::None, TextAlign::Center) => Alignment::Center,
                (Align::Right, _) | (Align::None, TextAlign::End) => Alignment::End,
                _ => Alignment::Start,
            };
            if p.layout.width() > inner || alignment != Alignment::Start || style.text_align != TextAlign::Start {
                p.layout = build_layout(fonts, cx, path, &p.display, &p.spans, style, inner);
                p.layout.align(alignment, AlignmentOptions::default());
            }
            prepared.push((c, p));
        }
        let height = prepared
            .iter()
            .map(|(c, p)| {
                let s = &styles[r][*c].0;
                p.layout.height().max(s.font_size * s.line_height) + s.padding[0] + s.padding[2]
            })
            .fold(0.0_f32, f32::max);
        for (c, p) in prepared {
            let style = &styles[r][c].0;
            let x = box_x + widths[..c].iter().sum::<f32>();
            views.push(BlockView {
                display_len: p.display.len(),
                layout: p.layout,
                map: p.map,
                text_x: x + style.padding[3],
                text_y: y + style.padding[0],
                top: y,
                bottom: y + height,
                box_x: x,
                box_width: widths[c],
                style: Style { border_bottom: None, ..style.clone() },
                glyph: None,
                bullet: None,
                chip: None,
                quote_bars: Vec::new(),
                rule: false,
                cell: Some(CellFrame { table: index, row: r, col: c, border, header_rule }),
                ornaments: Vec::new(),
            });
        }
        y += height;
    }
    views
}

/// The theme's style for a highlight capture. `function.method` matches `.hl-function` and,
/// more specifically, `.hl-function-method`.
fn capture_style(cx: &Ctx, base: &Style, capture: &str) -> Style {
    let parts: Vec<&str> = capture.split('.').collect();
    let classes: Vec<String> = (1..=parts.len()).map(|n| format!("hl-{}", parts[..n].join("-"))).collect();
    let classes: Vec<&str> = classes.iter().map(String::as_str).collect();
    let next = cx.style("span", &classes, base);
    Style { color: next.color, italic: next.italic, font_weight: next.font_weight, ..base.clone() }
}

fn push_style<'a>(builder: &mut parley::RangedBuilder<'a, Brush>, style: &'a Style, range: Option<Range<usize>>) {
    let props: [StyleProperty<'a, Brush>; 8] = [
        StyleProperty::FontFamily(FontFamily::Source(style.font_family.as_str().into())),
        StyleProperty::FontSize(style.font_size),
        StyleProperty::FontWeight(FontWeight::new(style.font_weight)),
        StyleProperty::FontStyle(if style.italic { FontStyle::Italic } else { FontStyle::Normal }),
        StyleProperty::Brush(Brush(style.color.with_alpha(style.opacity))),
        StyleProperty::Underline(style.underline),
        StyleProperty::Strikethrough(style.strikethrough),
        StyleProperty::LetterSpacing(style.letter_spacing),
    ];
    for prop in props {
        match &range {
            Some(r) => builder.push(prop, r.clone()),
            None => builder.push_default(prop),
        }
    }
    let lh = StyleProperty::LineHeight(LineHeight::FontSizeRelative(style.line_height));
    match &range {
        Some(r) => builder.push(lh, r.clone()),
        None => builder.push_default(lh),
    }
}

fn build_layout(
    fonts: &mut Fonts,
    cx: &Ctx,
    path: &[Node],
    text: &str,
    spans: &[(Range<usize>, Span)],
    base: &Style,
    width: f32,
) -> Layout<Brush> {
    let syntax_style = {
        let mut s = cx.style("span", &["syntax"], base);
        s.font_size = base.font_size * 0.85;
        s
    };
    let mut captures: HashMap<&str, Style> = HashMap::new();
    let styles: Vec<(Range<usize>, Style)> = spans
        .iter()
        .filter_map(|(range, span)| match span {
            Span::Content(flags) if *flags == InlineStyle::empty() => None,
            Span::Content(flags) => Some((range.clone(), inline_style(cx, path, base, *flags))),
            Span::Syntax => Some((range.clone(), syntax_style.clone())),
            Span::Preedit => Some((range.clone(), Style { underline: true, ..base.clone() })),
            Span::Highlight(capture) => Some((
                range.clone(),
                captures.entry(capture).or_insert_with(|| capture_style(cx, base, capture)).clone(),
            )),
        })
        .collect();
    let mut builder = fonts.layout_cx.ranged_builder(&mut fonts.font_cx, text, 1.0, true);
    push_style(&mut builder, base, None);
    for (range, style) in &styles {
        push_style(&mut builder, style, Some(range.clone()));
    }
    let mut layout = builder.build(text);
    if base.text_indent != 0.0 {
        layout.set_text_indent(base.text_indent, parley::IndentOptions::default());
    }
    layout.break_all_lines(Some(width));
    // Measuring natural width (an effectively unbounded line) must not align, or centered text
    // lands half a million pixels away.
    let alignment = match base.text_align {
        _ if width > 1e5 => Alignment::Start,
        TextAlign::Start => Alignment::Start,
        TextAlign::Center => Alignment::Center,
        TextAlign::End => Alignment::End,
        TextAlign::Justify => Alignment::Justify,
    };
    layout.align(alignment, AlignmentOptions::default());
    layout
}

pub fn simple_layout(fonts: &mut Fonts, text: &str, style: &Style, width: Option<f32>) -> Layout<Brush> {
    let mut builder = fonts.layout_cx.ranged_builder(&mut fonts.font_cx, text, 1.0, true);
    push_style(&mut builder, style, None);
    let mut layout = builder.build(text);
    layout.break_all_lines(width);
    layout.align(Alignment::Start, AlignmentOptions::default());
    layout
}

impl DocView {
    /// The block view whose source range contains `offset`, or the nearest preceding one.
    pub fn block_for_offset(&self, offset: usize) -> Option<&BlockView> {
        if let Some(b) = self.blocks.iter().find(|b| {
            let r = b.source_range();
            r.start <= offset && offset <= r.end
        }) {
            return Some(b);
        }
        let i = self.blocks.partition_point(|b| b.source_range().start <= offset);
        self.blocks.get(i.saturating_sub(1)).or(self.blocks.first())
    }

    /// Caret position plus the width of the character it sits before (for block carets).
    pub fn caret_box(&self, offset: usize) -> Option<CaretBox> {
        let b = self.block_for_offset(offset)?;
        let idx = b.display_index(offset.clamp(b.source_range().start, b.source_range().end));
        let cursor = Cursor::from_byte_index(&b.layout, idx, Affinity::Downstream);
        let geom = cursor.geometry(&b.layout, 2.0);
        let advance = cursor
            .downstream_cluster(&b.layout)
            .filter(|c| !c.is_hard_line_break() && !c.is_soft_line_break())
            .map(|c| c.advance())
            .filter(|a| *a > 0.0)
            .unwrap_or(b.style.font_size * 0.6);
        Some(CaretBox {
            x: b.text_x + geom.x0 as f32,
            y: b.text_y + geom.y0 as f32,
            height: (geom.y1 - geom.y0) as f32,
            advance,
        })
    }

    /// Rectangles covering the source range `sel`, in document coordinates.
    pub fn selection_rects(&self, sel: Range<usize>) -> Vec<(f32, f32, f32, f32)> {
        let mut rects = Vec::new();
        if sel.is_empty() {
            return rects;
        }
        for b in &self.blocks {
            let r = b.source_range();
            if r.end < sel.start || r.start > sel.end {
                continue;
            }
            let a = b.display_index(sel.start.max(r.start));
            let z = b.display_index(sel.end.min(r.end));
            let selection = parley::editing::Selection::new(
                Cursor::from_byte_index(&b.layout, a, Affinity::Downstream),
                Cursor::from_byte_index(&b.layout, z, Affinity::Upstream),
            );
            for (bb, _) in selection.geometry(&b.layout) {
                rects.push((
                    b.text_x + bb.x0 as f32,
                    b.text_y + bb.y0 as f32,
                    b.text_x + bb.x1 as f32,
                    b.text_y + bb.y1 as f32,
                ));
            }
            // Selection continuing past this block: mark the line end so the break is visible.
            if sel.end > r.end {
                let geom = Cursor::from_byte_index(&b.layout, z, Affinity::Upstream).geometry(&b.layout, 0.0);
                let x = b.text_x + geom.x0 as f32;
                rects.push((x, b.text_y + geom.y0 as f32, x + b.style.font_size * 0.5, b.text_y + geom.y1 as f32));
            }
        }
        rects
    }

    /// Among the blocks sharing block `i`'s row (table cells), the one nearest `x`.
    fn nearest_in_row(&self, i: usize, x: f32) -> &BlockView {
        let top = self.blocks[i].top;
        let distance = |b: &BlockView| (b.box_x - x).max(x - (b.box_x + b.box_width)).max(0.0);
        let row_start = self.blocks[..i].iter().rposition(|b| b.top != top).map_or(0, |j| j + 1);
        self.blocks[row_start..]
            .iter()
            .take_while(|b| b.top == top)
            .min_by(|a, b| distance(a).total_cmp(&distance(b)))
            .unwrap_or(&self.blocks[i])
    }

    /// The offset one visual line above or below `offset`, nearest to `x`. Walks parley lines
    /// rather than probing pixels, so block margins and padding never trap the caret.
    pub fn vertical_neighbor(&self, offset: usize, x: f32, down: bool) -> Option<usize> {
        let i = self.blocks.iter().position(|b| {
            let r = b.source_range();
            r.start <= offset && offset <= r.end
        })?;
        let b = &self.blocks[i];
        let idx = b.display_index(offset);
        let geom = Cursor::from_byte_index(&b.layout, idx, Affinity::Downstream).geometry(&b.layout, 1.0);
        let mid = ((geom.y0 + geom.y1) / 2.0) as f32;
        let lines: Vec<_> = b.layout.lines().map(|l| *l.metrics()).collect();
        let line = lines.iter().position(|m| mid < m.block_max_coord).unwrap_or(lines.len().saturating_sub(1));
        // Leaving a block goes to the next row of blocks: the neighbor paragraph, or the cell
        // in the same column of a table.
        let row_above = || self.blocks[..i].iter().rposition(|o| o.top < b.top).map(|j| self.nearest_in_row(j, x));
        let row_below = || self.blocks[i..].iter().position(|o| o.top > b.top).map(|j| self.nearest_in_row(i + j, x));
        let (block, line) = match (down, line) {
            (false, 0) => {
                let prev = row_above()?;
                (prev, prev.layout.lines().count().saturating_sub(1))
            }
            (false, l) => (b, l - 1),
            (true, l) if l + 1 < lines.len() => (b, l + 1),
            (true, _) => (row_below()?, 0),
        };
        let m = block.layout.lines().nth(line).map(|l| *l.metrics()).unwrap_or_default();
        let y = (m.block_min_coord + m.block_max_coord) / 2.0;
        let cursor = Cursor::from_point(&block.layout, x - block.text_x, y);
        Some(block.source_offset(cursor.index(), cursor.affinity()))
    }

    /// Whether a point in document coordinates is on a code block's language chip.
    pub fn chip_at(&self, x: f32, y: f32) -> bool {
        self.blocks.iter().filter_map(|b| b.chip.as_ref()).any(|c| c.chip_bounds().contains((x as f64, y as f64)))
    }

    /// Source offset under a point in document coordinates.
    pub fn hit_test(&self, x: f32, y: f32) -> Option<usize> {
        let first = self.blocks.iter().position(|b| y < b.bottom).unwrap_or(self.blocks.len().checked_sub(1)?);
        // Table cells share a row band; pick the one under (or nearest to) the pointer.
        let b = self.nearest_in_row(first, x);
        let cursor = Cursor::from_point(&b.layout, x - b.text_x, (y - b.text_y).max(0.0));
        Some(b.source_offset(cursor.index(), cursor.affinity()))
    }
}
