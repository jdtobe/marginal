//! Turns Markdown source into a flat list of leaf blocks whose bytes are partitioned into
//! *content* (rendered) and *syntax* (hidden, shown as marginalia or revealed at the caret).
//!
//! The partition is derived generically: any byte inside a leaf block that the parser did not
//! report as text is syntax. That covers `#`, `**`, `> ` prefixes, fences, escapes and link
//! destinations without special-casing each one.

use std::ops::Range;

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

pub fn parser_options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_MATH
        | Options::ENABLE_GFM
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
}

/// Inline formatting flags active on a run of content.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct InlineStyle(u16);

impl InlineStyle {
    pub const EMPHASIS: Self = Self(1 << 0);
    pub const STRONG: Self = Self(1 << 1);
    pub const STRIKE: Self = Self(1 << 2);
    pub const CODE: Self = Self(1 << 3);
    pub const LINK: Self = Self(1 << 4);
    pub const MATH: Self = Self(1 << 5);
    pub const HTML: Self = Self(1 << 6);
    pub const SUPERSCRIPT: Self = Self(1 << 7);
    pub const SUBSCRIPT: Self = Self(1 << 8);
    pub const IMAGE_ALT: Self = Self(1 << 9);
    /// Unparsed source shown verbatim (tables and front matter until they get real editors).
    pub const RAW: Self = Self(1 << 10);

    pub const fn empty() -> Self {
        Self(0)
    }
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

impl std::ops::BitOr for InlineStyle {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        self.union(rhs)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockKind {
    Paragraph,
    Heading { level: u8, setext: bool },
    CodeBlock { info: String, fenced: bool },
    Rule,
    Html,
    Table,
    FrontMatter,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContainerKind {
    Quote,
    ListItem { ordered: Option<u64>, task: Option<bool> },
    Footnote { label: String },
}

/// A container (quote, list item, footnote) enclosing a leaf block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Container {
    pub kind: ContainerKind,
    /// Source range of the whole container.
    pub range: Range<usize>,
    /// Source range of the container's own marker (`- `, `1. `, `> `, `[^n]: `), if any.
    pub marker: Range<usize>,
    /// Stable id so the UI can tell "first leaf of this container" apart from later ones.
    pub id: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InlineKind {
    Emphasis,
    Strong,
    Strike,
    Superscript,
    Subscript,
    Code,
    Math,
    Html,
    Link { dest: String },
    Image { dest: String },
    FootnoteRef { label: String },
}

/// An inline element. Its range covers its delimiters, e.g. the whole of `**bold**`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inline {
    pub kind: InlineKind,
    pub range: Range<usize>,
    pub parent: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SegmentKind {
    /// Source bytes displayed as-is.
    Text { style: InlineStyle },
    /// Source bytes displayed as different text (entities, escapes, footnote labels).
    Replaced { text: String, style: InlineStyle },
    /// A line break inside a block. Soft breaks display as a space.
    Break { hard: bool },
    /// Markup. Hidden unless revealed; `owner` is the inline element it delimits.
    Syntax { owner: Option<usize> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    pub range: Range<usize>,
    pub kind: SegmentKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    None,
    Left,
    Center,
    Right,
}

/// A GFM table's cells as the parser reports them. Row 0 is the header; the delimiter row isn't
/// included. Each cell range covers the text between its pipes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableShape {
    pub aligns: Vec<Align>,
    pub rows: Vec<Vec<Range<usize>>>,
}

/// A leaf block: the unit of layout. Its segments tile `range` exactly, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub kind: BlockKind,
    pub range: Range<usize>,
    /// Enclosing containers, outermost first.
    pub containers: Vec<Container>,
    pub segments: Vec<Segment>,
    pub inlines: Vec<Inline>,
    /// Cell layout, for tables.
    pub table: Option<Box<TableShape>>,
}

impl Block {
    /// The part of this block within `range` as a block of its own, for laying out one table
    /// cell. Segments are clipped to the range; inline indices stay valid.
    pub fn sub_block(&self, range: Range<usize>) -> Block {
        let owner_of = |r: &Range<usize>| {
            self.inlines
                .iter()
                .enumerate()
                .filter(|(_, i)| i.range.start <= r.start && r.end <= i.range.end)
                .min_by_key(|(_, i)| i.range.len())
                .map(|(idx, _)| idx)
        };
        let segments = self
            .segments
            .iter()
            .filter(|s| s.range.start < range.end && range.start < s.range.end)
            .filter_map(|s| {
                let clipped = s.range.start.max(range.start)..s.range.end.min(range.end);
                let kind = match &s.kind {
                    SegmentKind::Syntax { .. } => SegmentKind::Syntax { owner: owner_of(&clipped) },
                    SegmentKind::Replaced { .. } if clipped != s.range => return None,
                    k => k.clone(),
                };
                Some(Segment { range: clipped, kind })
            })
            .collect();
        Block {
            kind: BlockKind::Paragraph,
            range,
            containers: self.containers.clone(),
            segments,
            inlines: self.inlines.clone(),
            table: None,
        }
    }

    /// The innermost inline elements containing `offset`, innermost last.
    pub fn inlines_at(&self, offset: usize) -> Vec<usize> {
        let mut hits: Vec<usize> = (0..self.inlines.len())
            .filter(|&i| {
                let r = &self.inlines[i].range;
                r.start <= offset && offset <= r.end
            })
            .collect();
        hits.sort_by_key(|&i| std::cmp::Reverse(self.inlines[i].range.len()));
        hits
    }
}

struct OpenLeaf {
    kind: BlockKind,
    start: usize,
    /// Pulldown-reported end, or `None` for an implicit paragraph in a tight list item.
    end: Option<usize>,
    content: Vec<Segment>,
    inlines: Vec<Inline>,
    inline_stack: Vec<usize>,
    style_stack: Vec<InlineStyle>,
    last_content_end: usize,
    table: Option<Box<TableShape>>,
}

impl OpenLeaf {
    fn new(kind: BlockKind, start: usize, end: Option<usize>) -> Self {
        Self {
            kind,
            start,
            end,
            content: Vec::new(),
            inlines: Vec::new(),
            inline_stack: Vec::new(),
            style_stack: Vec::new(),
            last_content_end: start,
            table: None,
        }
    }

    fn style(&self) -> InlineStyle {
        self.style_stack.iter().fold(InlineStyle::empty(), |a, &b| a | b)
    }

    fn push_content(&mut self, range: Range<usize>, kind: SegmentKind) {
        self.last_content_end = self.last_content_end.max(range.end);
        self.content.push(Segment { range, kind });
    }

    fn open_inline(&mut self, kind: InlineKind, range: Range<usize>, style: InlineStyle) {
        let parent = self.inline_stack.last().copied();
        self.inlines.push(Inline { kind, range, parent });
        self.inline_stack.push(self.inlines.len() - 1);
        self.style_stack.push(style);
    }

    fn close_inline(&mut self) {
        self.inline_stack.pop();
        self.style_stack.pop();
    }

    /// A leaf atom like inline code: content between delimiters, delimiters as syntax.
    fn push_atom(
        &mut self,
        src: &str,
        kind: InlineKind,
        range: Range<usize>,
        delim: usize,
        text: &str,
        style: InlineStyle,
    ) {
        let parent = self.inline_stack.last().copied();
        self.inlines.push(Inline { kind, range: range.clone(), parent });
        let style = self.style() | style;
        let delim = delim.min(range.len() / 2);
        let mut inner = range.start + delim..range.end - delim;
        let trimmed = inner.start + 1..inner.end.saturating_sub(1);
        if src.get(inner.clone()) != Some(text)
            && trimmed.start <= trimmed.end
            && src.get(trimmed.clone()) == Some(text)
        {
            inner = trimmed;
        }
        let kind = if src.get(inner.clone()) == Some(text) {
            SegmentKind::Text { style }
        } else {
            SegmentKind::Replaced { text: text.to_owned(), style }
        };
        self.push_content(inner, kind);
    }

    fn finish(self, src: &str, containers: Vec<Container>) -> Block {
        let end = self.end.unwrap_or(self.last_content_end).max(self.last_content_end);
        let range = self.start..end;
        let mut content = self.content;
        content.sort_by_key(|s| s.range.start);

        let mut segments = Vec::with_capacity(content.len() * 2 + 1);
        let mut pos = range.start;
        let owner_of = |gap: &Range<usize>| -> Option<usize> {
            self.inlines
                .iter()
                .enumerate()
                .filter(|(_, i)| i.range.start <= gap.start && gap.end <= i.range.end)
                .min_by_key(|(_, i)| i.range.len())
                .map(|(idx, _)| idx)
        };
        for seg in content {
            if seg.range.start < pos {
                continue; // overlapping report from the parser; keep the first
            }
            if seg.range.start > pos {
                let gap = pos..seg.range.start;
                segments.push(Segment { kind: SegmentKind::Syntax { owner: owner_of(&gap) }, range: gap });
            }
            pos = seg.range.end;
            segments.push(seg);
        }
        if pos < range.end {
            let gap = pos..range.end;
            segments.push(Segment { kind: SegmentKind::Syntax { owner: owner_of(&gap) }, range: gap });
        }
        debug_assert!(segments.iter().all(|s| src.is_char_boundary(s.range.start)));
        Block { kind: self.kind, range, containers, segments, inlines: self.inlines, table: self.table }
    }
}

/// Parses `src` into leaf blocks. Never modifies or normalizes the source.
pub fn parse_blocks(src: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut containers: Vec<Container> = Vec::new();
    let mut leaf: Option<OpenLeaf> = None;
    let mut next_container_id = 0;
    // Nesting depth of a raw leaf (table, front matter) whose inner events we ignore.
    let mut raw_depth = 0usize;

    let close_leaf = |leaf: &mut Option<OpenLeaf>, blocks: &mut Vec<Block>, containers: &[Container]| {
        if let Some(l) = leaf.take() {
            blocks.push(l.finish(src, containers.to_vec()));
        }
    };
    // A container that ends without any leaf (an empty `- ` or `>`) still needs a block to hold
    // the caret: give it an empty paragraph right after its marker.
    let ensure_leaf = |blocks: &mut Vec<Block>, containers: &[Container]| {
        let Some(c) = containers.last() else { return };
        if blocks.last().is_some_and(|b| b.range.start >= c.range.start) {
            return;
        }
        let at = c.marker.end.min(c.range.end);
        blocks.push(Block {
            kind: BlockKind::Paragraph,
            range: at..at,
            containers: containers.to_vec(),
            segments: Vec::new(),
            inlines: Vec::new(),
            table: None,
        });
    };

    for (event, range) in Parser::new_ext(src, parser_options()).into_offset_iter() {
        if raw_depth > 0 {
            match event {
                Event::Start(_) => raw_depth += 1,
                Event::End(_) => {
                    raw_depth -= 1;
                    if raw_depth == 0 {
                        close_leaf(&mut leaf, &mut blocks, &containers);
                    }
                }
                _ => {}
            }
            continue;
        }

        // Content arriving outside any leaf (tight list items) opens an implicit paragraph.
        let is_inline = matches!(
            event,
            Event::Text(_)
                | Event::Code(_)
                | Event::InlineMath(_)
                | Event::DisplayMath(_)
                | Event::InlineHtml(_)
                | Event::FootnoteReference(_)
                | Event::SoftBreak
                | Event::HardBreak
                | Event::Start(
                    Tag::Emphasis
                        | Tag::Strong
                        | Tag::Strikethrough
                        | Tag::Superscript
                        | Tag::Subscript
                        | Tag::Link { .. }
                        | Tag::Image { .. }
                )
        );
        if is_inline && leaf.is_none() {
            leaf = Some(OpenLeaf::new(BlockKind::Paragraph, range.start, None));
        }

        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => {
                    close_leaf(&mut leaf, &mut blocks, &containers);
                    leaf = Some(OpenLeaf::new(BlockKind::Paragraph, range.start, Some(range.end)));
                }
                Tag::Heading { level, .. } => {
                    close_leaf(&mut leaf, &mut blocks, &containers);
                    let setext = !src[range.clone()].starts_with('#');
                    leaf = Some(OpenLeaf::new(
                        BlockKind::Heading { level: heading_level(level), setext },
                        range.start,
                        Some(range.end),
                    ));
                }
                Tag::CodeBlock(kind) => {
                    close_leaf(&mut leaf, &mut blocks, &containers);
                    let (info, fenced) = match kind {
                        CodeBlockKind::Fenced(info) => (info.to_string(), true),
                        CodeBlockKind::Indented => (String::new(), false),
                    };
                    leaf = Some(OpenLeaf::new(BlockKind::CodeBlock { info, fenced }, range.start, Some(range.end)));
                }
                Tag::HtmlBlock => {
                    close_leaf(&mut leaf, &mut blocks, &containers);
                    leaf = Some(OpenLeaf::new(BlockKind::Html, range.start, Some(range.end)));
                }
                Tag::Table(aligns) => {
                    close_leaf(&mut leaf, &mut blocks, &containers);
                    let mut l = OpenLeaf::new(BlockKind::Table, range.start, Some(range.end));
                    let aligns = aligns
                        .iter()
                        .map(|a| match a {
                            pulldown_cmark::Alignment::None => Align::None,
                            pulldown_cmark::Alignment::Left => Align::Left,
                            pulldown_cmark::Alignment::Center => Align::Center,
                            pulldown_cmark::Alignment::Right => Align::Right,
                        })
                        .collect();
                    l.table = Some(Box::new(TableShape { aligns, rows: Vec::new() }));
                    leaf = Some(l);
                }
                Tag::MetadataBlock(_) => {
                    close_leaf(&mut leaf, &mut blocks, &containers);
                    let mut l = OpenLeaf::new(BlockKind::FrontMatter, range.start, Some(range.end));
                    l.push_content(range.clone(), SegmentKind::Text { style: InlineStyle::RAW });
                    leaf = Some(l);
                    raw_depth = 1;
                }
                Tag::BlockQuote(_) => {
                    close_leaf(&mut leaf, &mut blocks, &containers);
                    containers.push(Container {
                        kind: ContainerKind::Quote,
                        marker: quote_marker(src, range.start),
                        range,
                        id: next_container_id,
                    });
                    next_container_id += 1;
                }
                Tag::Item => {
                    close_leaf(&mut leaf, &mut blocks, &containers);
                    containers.push(Container {
                        kind: ContainerKind::ListItem { ordered: list_number(src, range.start), task: None },
                        marker: list_marker(src, range.start),
                        range,
                        id: next_container_id,
                    });
                    next_container_id += 1;
                }
                Tag::FootnoteDefinition(label) => {
                    close_leaf(&mut leaf, &mut blocks, &containers);
                    let marker_end = src[range.clone()].find("]:").map_or(range.start, |i| range.start + i + 2);
                    containers.push(Container {
                        kind: ContainerKind::Footnote { label: label.to_string() },
                        marker: range.start..marker_end,
                        range,
                        id: next_container_id,
                    });
                    next_container_id += 1;
                }
                Tag::List(_) | Tag::DefinitionList | Tag::DefinitionListTitle | Tag::DefinitionListDefinition => {
                    close_leaf(&mut leaf, &mut blocks, &containers);
                }
                Tag::TableHead | Tag::TableRow => {
                    if let Some(t) = leaf.as_mut().and_then(|l| l.table.as_mut()) {
                        t.rows.push(Vec::new());
                    }
                }
                Tag::TableCell => {
                    if let Some(row) = leaf.as_mut().and_then(|l| l.table.as_mut()).and_then(|t| t.rows.last_mut()) {
                        row.push(range);
                    }
                }
                Tag::Emphasis | Tag::Strong | Tag::Strikethrough | Tag::Superscript | Tag::Subscript => {
                    let (kind, style) = match tag {
                        Tag::Emphasis => (InlineKind::Emphasis, InlineStyle::EMPHASIS),
                        Tag::Strong => (InlineKind::Strong, InlineStyle::STRONG),
                        Tag::Strikethrough => (InlineKind::Strike, InlineStyle::STRIKE),
                        Tag::Superscript => (InlineKind::Superscript, InlineStyle::SUPERSCRIPT),
                        _ => (InlineKind::Subscript, InlineStyle::SUBSCRIPT),
                    };
                    leaf.as_mut().unwrap().open_inline(kind, range, style);
                }
                Tag::Link { dest_url, .. } => {
                    leaf.as_mut().unwrap().open_inline(
                        InlineKind::Link { dest: dest_url.to_string() },
                        range,
                        InlineStyle::LINK,
                    );
                }
                Tag::Image { dest_url, .. } => {
                    leaf.as_mut().unwrap().open_inline(
                        InlineKind::Image { dest: dest_url.to_string() },
                        range,
                        InlineStyle::IMAGE_ALT,
                    );
                }
            },
            Event::End(end) => match end {
                TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::CodeBlock | TagEnd::HtmlBlock | TagEnd::Table => {
                    close_leaf(&mut leaf, &mut blocks, &containers);
                }
                TagEnd::BlockQuote(_) | TagEnd::Item | TagEnd::FootnoteDefinition => {
                    close_leaf(&mut leaf, &mut blocks, &containers);
                    ensure_leaf(&mut blocks, &containers);
                    containers.pop();
                }
                TagEnd::List(_) => close_leaf(&mut leaf, &mut blocks, &containers),
                TagEnd::Emphasis
                | TagEnd::Strong
                | TagEnd::Strikethrough
                | TagEnd::Superscript
                | TagEnd::Subscript
                | TagEnd::Link
                | TagEnd::Image => {
                    if let Some(l) = leaf.as_mut() {
                        l.close_inline();
                    }
                }
                _ => {}
            },
            Event::Text(text) => {
                let l = leaf.as_mut().unwrap();
                let style = l.style();
                let kind = if src.get(range.clone()) == Some(&*text) {
                    SegmentKind::Text { style }
                } else {
                    SegmentKind::Replaced { text: text.to_string(), style }
                };
                l.push_content(range, kind);
            }
            Event::Code(text) => {
                let delim = src[range.clone()].bytes().take_while(|&b| b == b'`').count();
                leaf.as_mut().unwrap().push_atom(src, InlineKind::Code, range, delim, &text, InlineStyle::CODE);
            }
            Event::InlineMath(text) => {
                leaf.as_mut().unwrap().push_atom(src, InlineKind::Math, range, 1, &text, InlineStyle::MATH);
            }
            Event::DisplayMath(text) => {
                leaf.as_mut().unwrap().push_atom(src, InlineKind::Math, range, 2, &text, InlineStyle::MATH);
            }
            Event::InlineHtml(_) => {
                let l = leaf.as_mut().unwrap();
                let style = l.style() | InlineStyle::HTML;
                l.push_content(range, SegmentKind::Text { style });
            }
            Event::Html(_) => {
                if let Some(l) = leaf.as_mut() {
                    l.push_content(range, SegmentKind::Text { style: InlineStyle::HTML });
                }
            }
            Event::FootnoteReference(label) => {
                let l = leaf.as_mut().unwrap();
                let parent = l.inline_stack.last().copied();
                l.inlines.push(Inline {
                    kind: InlineKind::FootnoteRef { label: label.to_string() },
                    range: range.clone(),
                    parent,
                });
                let style = l.style() | InlineStyle::SUPERSCRIPT;
                let inner = range.start + 2..range.end - 1; // [^label]
                l.push_content(inner, SegmentKind::Replaced { text: label.to_string(), style });
            }
            Event::SoftBreak => leaf.as_mut().unwrap().push_content(range, SegmentKind::Break { hard: false }),
            Event::HardBreak => leaf.as_mut().unwrap().push_content(range, SegmentKind::Break { hard: true }),
            Event::Rule => {
                close_leaf(&mut leaf, &mut blocks, &containers);
                leaf = Some(OpenLeaf::new(BlockKind::Rule, range.start, Some(range.end)));
                close_leaf(&mut leaf, &mut blocks, &containers);
            }
            Event::TaskListMarker(checked) => {
                if let Some(Container { kind: ContainerKind::ListItem { task, .. }, marker, .. }) =
                    containers.last_mut()
                {
                    *task = Some(checked);
                    // The checkbox becomes part of the item's marker, so it's hidden from the text.
                    marker.end = marker.end.max(skip_spaces(src, range.end));
                }
            }
        }
    }
    close_leaf(&mut leaf, &mut blocks, &containers);
    blocks
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn skip_spaces(src: &str, mut at: usize) -> usize {
    while src.as_bytes().get(at).is_some_and(|b| *b == b' ' || *b == b'\t') {
        at += 1;
    }
    at
}

fn quote_marker(src: &str, start: usize) -> Range<usize> {
    let at = skip_spaces(src, start);
    if src.as_bytes().get(at) == Some(&b'>') {
        let end = at + 1;
        at..if src.as_bytes().get(end) == Some(&b' ') { end + 1 } else { end }
    } else {
        start..start
    }
}

fn list_marker(src: &str, start: usize) -> Range<usize> {
    let bytes = src.as_bytes();
    let mut end = start;
    while bytes.get(end).is_some_and(u8::is_ascii_digit) {
        end += 1;
    }
    if bytes.get(end).is_some_and(|b| matches!(b, b'-' | b'+' | b'*' | b'.' | b')')) {
        end += 1;
    }
    start..skip_spaces(src, end)
}

fn list_number(src: &str, start: usize) -> Option<u64> {
    let digits: String = src[start..].chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}
