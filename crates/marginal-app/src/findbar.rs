//! The find & replace strip across the top of the window. Plain text by default; `.*` turns
//! the query into a regex, `Aa` makes it case-sensitive and `\b` matches whole words.

use std::ops::Range;

use marginal_doc::{FindOptions, Finder};
use marginal_theme::{Appearance, Color, Style, Theme};
use unicode_segmentation::UnicodeSegmentation;
use vello::Scene;
use vello::kurbo::{Affine, Line, Rect, RoundedRect, Stroke};
use vello::peniko::Fill;
use winit::keyboard::{Key, NamedKey};

use crate::paint::{color, draw_layout};
use crate::view::{Fonts, simple_layout};

/// A one-line text field: its text and caret, and whether everything is selected (so typing
/// replaces it).
#[derive(Clone, Debug, Default)]
pub struct Field {
    pub text: String,
    pub caret: usize,
    pub all: bool,
}

impl Field {
    pub fn insert(&mut self, s: &str) {
        let s = s.replace(['\n', '\r'], " ");
        if self.all {
            self.text.clear();
            self.caret = 0;
            self.all = false;
        }
        self.text.insert_str(self.caret, &s);
        self.caret += s.len();
    }

    pub fn backspace(&mut self) {
        if self.all {
            self.set("");
            return;
        }
        if let Some((i, _)) = self.text[..self.caret].grapheme_indices(true).next_back() {
            self.text.replace_range(i..self.caret, "");
            self.caret = i;
        }
    }

    pub fn set(&mut self, text: &str) {
        self.text = text.to_owned();
        self.caret = self.text.len();
        self.all = false;
    }

    fn step(&mut self, forward: bool) {
        self.all = false;
        self.caret = if forward {
            self.text[self.caret..].graphemes(true).next().map_or(self.caret, |g| self.caret + g.len())
        } else {
            self.text[..self.caret].grapheme_indices(true).next_back().map_or(0, |(i, _)| i)
        };
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Find,
    Replace,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Toggle {
    Regex,
    Case,
    Word,
}

/// Something the bar asks the app to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// The query or options changed: search again and show the first match.
    Search,
    Next,
    Previous,
    Replace,
    ReplaceAll,
    Close,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Hit {
    Field(Focus),
    Toggle(Toggle),
    Action(Action),
}

pub struct FindBar {
    pub find: Field,
    pub replace: Field,
    /// Which field has the keyboard; `None` while typing in the document.
    pub focus: Option<Focus>,
    pub options: FindOptions,
    pub show_replace: bool,
    pub matches: Vec<Range<usize>>,
    pub current: Option<usize>,
    pub error: Option<String>,
    /// The document version and query the matches were found for.
    searched: Option<(usize, String, FindOptions)>,
    hits: Vec<(Rect, Hit)>,
}

pub const ROW: f32 = 40.0;

impl FindBar {
    pub fn new(options: FindOptions) -> Self {
        Self {
            find: Field::default(),
            replace: Field::default(),
            focus: Some(Focus::Find),
            options,
            show_replace: false,
            matches: Vec::new(),
            current: None,
            error: None,
            searched: None,
            hits: Vec::new(),
        }
    }

    /// The strip's height for a window `width` wide: the replace controls wrap to a second row
    /// when the window is narrow.
    pub fn height(&self, width: f32) -> f32 {
        if self.show_replace && width < 760.0 { ROW * 2.0 } else { ROW }
    }

    pub fn finder(&self) -> Option<Finder> {
        (!self.find.text.is_empty()).then(|| Finder::new(&self.find.text, self.options).ok()).flatten()
    }

    /// Searches `text` again if it or the query changed since the last search. Keeps the current
    /// match on the same occurrence where possible; otherwise picks the first at or after
    /// `caret`. Returns whether the matches changed.
    pub fn refresh(&mut self, text: &str, version: usize, caret: usize) -> bool {
        let key = (version, self.find.text.clone(), self.options);
        if self.searched.as_ref() == Some(&key) {
            return false;
        }
        self.searched = Some(key);
        self.error = None;
        self.matches = if self.find.text.is_empty() {
            Vec::new()
        } else {
            match Finder::new(&self.find.text, self.options) {
                Ok(f) => f.find_all(text),
                Err(e) => {
                    self.error = Some(e);
                    Vec::new()
                }
            }
        };
        self.current = self.matches.iter().position(|m| m.start >= caret).or((!self.matches.is_empty()).then_some(0));
        true
    }

    /// Moves to the next or previous match after `caret`, wrapping around.
    pub fn step(&mut self, caret: usize, forward: bool) -> Option<Range<usize>> {
        if self.matches.is_empty() {
            return None;
        }
        let n = self.matches.len();
        let i = if forward {
            self.matches.iter().position(|m| m.start >= caret).unwrap_or(0)
        } else {
            self.matches.iter().rposition(|m| m.end < caret).unwrap_or(n - 1)
        };
        self.current = Some(i);
        Some(self.matches[i].clone())
    }

    /// Forces the next refresh to search again (after switching documents).
    pub fn invalidate(&mut self) {
        self.searched = None;
    }

    pub fn current_match(&self) -> Option<Range<usize>> {
        self.matches.get(self.current?).cloned()
    }

    fn field(&mut self, focus: Focus) -> &mut Field {
        match focus {
            Focus::Find => &mut self.find,
            Focus::Replace => &mut self.replace,
        }
    }

    /// The focused field, for clipboard commands.
    pub fn focused_field(&mut self) -> Option<&mut Field> {
        let focus = self.focus?;
        Some(self.field(focus))
    }

    pub fn toggle(&mut self, toggle: Toggle) {
        let o = &mut self.options;
        match toggle {
            Toggle::Regex => o.regex = !o.regex,
            Toggle::Case => o.case_sensitive = !o.case_sensitive,
            Toggle::Word => o.whole_word = !o.whole_word,
        }
    }

    /// A key while a field has focus. `None` means the key isn't the bar's.
    pub fn key(&mut self, key: &Key, text: Option<&str>, shift: bool, cmd: bool) -> Option<Option<Action>> {
        let focus = self.focus?;
        let action = match key {
            Key::Named(NamedKey::Escape) => Some(Action::Close),
            Key::Named(NamedKey::Enter) if focus == Focus::Replace && cmd => Some(Action::ReplaceAll),
            Key::Named(NamedKey::Enter) if focus == Focus::Replace => Some(Action::Replace),
            Key::Named(NamedKey::Enter) if shift => Some(Action::Previous),
            Key::Named(NamedKey::Enter) => Some(Action::Next),
            Key::Named(NamedKey::Tab) => {
                if self.show_replace {
                    self.focus = Some(if focus == Focus::Find { Focus::Replace } else { Focus::Find });
                }
                None
            }
            Key::Named(NamedKey::Backspace) => {
                self.field(focus).backspace();
                (focus == Focus::Find).then_some(Action::Search)
            }
            Key::Named(NamedKey::ArrowLeft) => {
                self.field(focus).step(false);
                None
            }
            Key::Named(NamedKey::ArrowRight) => {
                self.field(focus).step(true);
                None
            }
            Key::Named(NamedKey::Home | NamedKey::ArrowUp) => {
                let f = self.field(focus);
                (f.caret, f.all) = (0, false);
                None
            }
            Key::Named(NamedKey::End | NamedKey::ArrowDown) => {
                let f = self.field(focus);
                (f.caret, f.all) = (f.text.len(), false);
                None
            }
            _ if cmd => return None,
            _ => {
                let t = text.filter(|t| !t.is_empty() && !t.chars().any(char::is_control))?;
                self.field(focus).insert(t);
                (focus == Focus::Find).then_some(Action::Search)
            }
        };
        Some(action)
    }

    /// A click in the strip (window coordinates). Returns an action to run, if any.
    pub fn click(&mut self, x: f32, y: f32) -> Option<Action> {
        let hit = self.hits.iter().find(|(r, _)| r.contains((x as f64, y as f64))).map(|(_, h)| *h)?;
        match hit {
            Hit::Field(f) => {
                self.focus = Some(f);
                None
            }
            Hit::Toggle(t) => {
                self.toggle(t);
                Some(Action::Search)
            }
            Hit::Action(a) => Some(a),
        }
    }

    /// The counter text: "3 of 12", "No results", or a regex error.
    fn status(&self) -> (String, bool) {
        if let Some(e) = &self.error {
            return (e.clone(), true);
        }
        if self.find.text.is_empty() {
            return (String::new(), false);
        }
        match (self.current, self.matches.len()) {
            (_, 0) => ("No results".into(), false),
            (Some(i), n) => (format!("{} of {n}", i + 1), false),
            (None, n) => (format!("{n} found"), false),
        }
    }

    /// Draws the strip across the top of a window `width` wide (in window coordinates).
    #[allow(clippy::too_many_arguments)]
    pub fn paint(
        &mut self,
        scene: &mut Scene,
        fonts: &mut Fonts,
        theme: &Theme,
        appearance: Appearance,
        origin: Affine,
        width: f32,
        blink_on: bool,
    ) {
        let body = theme.style(appearance, "body", &[], None);
        let chrome = theme.style(appearance, "span", &["chrome"], Some(&body));
        let glyph = theme.style(appearance, "span", &["glyph"], Some(&body)).color;
        let accent = theme.style(appearance, "a", &[], Some(&body)).color;
        let paper = body.background.unwrap_or(Color::WHITE);
        let mut text_style = Style { font_size: 13.0, line_height: 1.3, color: body.color, ..body.clone() };
        text_style.background = None;
        let mut muted = text_style.clone();
        muted.color = glyph;
        let height = self.height(width);

        let bg = chrome.background.unwrap_or(paper);
        scene.fill(Fill::NonZero, origin, color(bg), None, &Rect::new(0.0, 0.0, width as f64, height as f64));
        let rule = Line::new((0.0, height as f64 - 0.5), (width as f64, height as f64 - 0.5));
        scene.stroke(&Stroke::new(1.0), origin, color(glyph.with_alpha(0.35)), None, &rule);

        let mut hits = Vec::new();
        let (pad, gap, h) = (12.0_f32, 6.0_f32, 26.0_f32);
        let button_w = |fonts: &mut Fonts, label: &str| simple_layout(fonts, label, &text_style, None).width() + 16.0;

        // Lay out right to left the fixed controls of the first row, then give the field the rest.
        let narrow = width < 760.0;
        let mut right_row1: Vec<(String, Hit)> =
            vec![("‹".into(), Hit::Action(Action::Previous)), ("›".into(), Hit::Action(Action::Next))];
        let mut replace_row: Vec<(String, Hit)> = Vec::new();
        if self.show_replace {
            replace_row =
                vec![("Replace".into(), Hit::Action(Action::Replace)), ("All".into(), Hit::Action(Action::ReplaceAll))];
        }
        right_row1.push(("×".into(), Hit::Action(Action::Close)));
        let toggles = [
            (".*", Toggle::Regex, self.options.regex),
            ("Aa", Toggle::Case, self.options.case_sensitive),
            ("\\b", Toggle::Word, self.options.whole_word),
        ];
        let (status, is_error) = self.status();
        let status_w = simple_layout(fonts, if status.is_empty() { "00 of 000" } else { &status }, &text_style, None)
            .width()
            .max(70.0);
        let toggles_w: f32 = toggles.iter().map(|(l, _, _)| button_w(fonts, l) + gap).sum();
        let buttons_w: f32 = right_row1.iter().map(|(l, _)| button_w(fonts, l) + gap).sum::<f32>();
        let replace_buttons_w: f32 = replace_row.iter().map(|(l, _)| button_w(fonts, l) + gap).sum::<f32>();
        let fixed = pad * 2.0 + toggles_w + status_w + gap * 2.0 + buttons_w;
        let (find_w, replace_w) = if self.show_replace && !narrow {
            let rest = (width - fixed - replace_buttons_w - gap * 2.0).max(240.0);
            (rest * 0.5, rest * 0.5)
        } else {
            ((width - fixed).max(140.0), (width - pad * 2.0 - replace_buttons_w - gap).max(140.0))
        };

        let top = (ROW - h) / 2.0;
        let mut x = pad;
        let field = |scene: &mut Scene,
                     fonts: &mut Fonts,
                     x: f32,
                     y: f32,
                     w: f32,
                     which: Focus,
                     hits: &mut Vec<(Rect, Hit)>| {
            let rect = Rect::new(x as f64, y as f64, (x + w) as f64, (y + h) as f64);
            let focused = self.focus == Some(which);
            scene.fill(Fill::NonZero, origin, color(paper), None, &RoundedRect::from_rect(rect, 5.0));
            let border = if focused { accent } else { glyph.with_alpha(0.5) };
            scene.stroke(
                &Stroke::new(if focused { 1.5 } else { 1.0 }),
                origin,
                color(border),
                None,
                &RoundedRect::from_rect(rect, 5.0),
            );
            let f = match which {
                Focus::Find => &self.find,
                Focus::Replace => &self.replace,
            };
            let (shown, style) = if f.text.is_empty() {
                (if which == Focus::Find { "Find" } else { "Replace with" }, &muted)
            } else {
                (f.text.as_str(), &text_style)
            };
            let layout = simple_layout(fonts, shown, style, None);
            let ty = y + (h - layout.height()) / 2.0;
            let tx = x + 8.0;
            if f.all && focused && !f.text.is_empty() {
                let sel = Rect::new(tx as f64, ty as f64, (tx + layout.width()) as f64, (ty + layout.height()) as f64);
                scene.fill(Fill::NonZero, origin, color(accent.with_alpha(0.25)), None, &sel);
            }
            scene.push_clip_layer(Fill::NonZero, origin, &rect);
            draw_layout(scene, &layout, origin * Affine::translate((tx as f64, ty as f64)));
            if focused && blink_on && !f.all {
                let before = if f.text.is_empty() {
                    0.0
                } else {
                    simple_layout(fonts, &f.text[..f.caret], &text_style, None).full_width()
                };
                let cx = (tx + before) as f64;
                scene.fill(
                    Fill::NonZero,
                    origin,
                    color(accent),
                    None,
                    &Rect::new(cx, ty as f64, cx + 1.5, (ty + layout.height()) as f64),
                );
            }
            scene.pop_layer();
            hits.push((rect, Hit::Field(which)));
        };
        let button = |scene: &mut Scene,
                      fonts: &mut Fonts,
                      x: f32,
                      y: f32,
                      label: &str,
                      on: bool,
                      hit: Hit,
                      hits: &mut Vec<(Rect, Hit)>|
         -> f32 {
            let layout = simple_layout(
                fonts,
                label,
                &Style { color: if on { paper } else { text_style.color }, ..text_style.clone() },
                None,
            );
            let w = layout.width() + 16.0;
            let rect = Rect::new(x as f64, y as f64, (x + w) as f64, (y + h) as f64);
            if on {
                scene.fill(Fill::NonZero, origin, color(accent), None, &RoundedRect::from_rect(rect, 5.0));
            } else {
                scene.stroke(
                    &Stroke::new(1.0),
                    origin,
                    color(glyph.with_alpha(0.45)),
                    None,
                    &RoundedRect::from_rect(rect, 5.0),
                );
            }
            draw_layout(
                scene,
                &layout,
                origin * Affine::translate(((x + 8.0) as f64, (y + (h - layout.height()) / 2.0) as f64)),
            );
            hits.push((rect, hit));
            w
        };

        field(scene, fonts, x, top, find_w, Focus::Find, &mut hits);
        x += find_w + gap;
        for (label, t, on) in toggles {
            x += button(scene, fonts, x, top, label, on, Hit::Toggle(t), &mut hits) + gap;
        }
        let status_layout = simple_layout(
            fonts,
            &status,
            &Style { color: if is_error { Color::rgb(0xd0, 0x3b, 0x3b) } else { glyph }, ..text_style.clone() },
            None,
        );
        draw_layout(
            scene,
            &status_layout,
            origin * Affine::translate(((x + 4.0) as f64, (top + (h - status_layout.height()) / 2.0) as f64)),
        );
        x += status_w + gap;
        let (prev, rest) = right_row1.split_at(2);
        for (label, hit) in prev {
            x += button(scene, fonts, x, top, label, false, *hit, &mut hits) + gap;
        }
        if self.show_replace {
            let (rx, ry) = if narrow { (pad, ROW + top) } else { (x + gap, top) };
            field(scene, fonts, rx, ry, replace_w, Focus::Replace, &mut hits);
            let mut bx = rx + replace_w + gap;
            for (label, hit) in &replace_row {
                bx += button(scene, fonts, bx, ry, label, false, *hit, &mut hits) + gap;
            }
            if !narrow {
                x = bx;
            }
        }
        // Close sits at the far right of the first row.
        let close_w = button_w(fonts, &rest[0].0);
        button(scene, fonts, (width - pad - close_w).max(x), top, &rest[0].0, false, rest[0].1, &mut hits);
        self.hits = hits;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_edit_by_grapheme_and_select_all_replaces() {
        let mut f = Field::default();
        f.insert("ab👍🏽c");
        f.step(false);
        f.backspace();
        assert_eq!((f.text.as_str(), f.caret), ("abc", 2));
        f.all = true;
        f.insert("z");
        assert_eq!(f.text, "z");
    }

    #[test]
    fn searching_steps_and_wraps() {
        let mut bar = FindBar::new(FindOptions::default());
        bar.find.set("ab");
        assert!(bar.refresh("ab ab ab", 1, 3));
        assert!(!bar.refresh("ab ab ab", 1, 3), "unchanged input is not searched again");
        assert_eq!(bar.current_match(), Some(3..5));
        assert_eq!(bar.step(5, true), Some(6..8));
        assert_eq!(bar.step(8, true), Some(0..2), "wraps around");
        assert_eq!(bar.step(0, false), Some(6..8), "wraps backwards");
        bar.options.regex = true;
        bar.find.set("(");
        bar.refresh("ab", 1, 0);
        assert!(bar.error.is_some() && bar.matches.is_empty());
    }
}
