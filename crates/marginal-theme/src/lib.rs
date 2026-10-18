//! Marginal themes: a subset of CSS that styles Markdown elements.
//!
//! Selectors are element names (`h1`, `blockquote`, `code`, …), classes for Marginal's own
//! parts (`.glyph`, `.caret`, `.chrome`), or both (`li.task`). `#write` and `body` both mean
//! the document root, so many Typora themes match unchanged. Variables live on `:root`, with
//! overrides under `@media (prefers-color-scheme: dark|light)`. Anything we can't honor is
//! reported in [`Theme::diagnostics`] rather than silently dropped.

use std::collections::HashMap;

use lightningcss::rules::CssRule;
use lightningcss::stylesheet::{ParserOptions, PrinterOptions, StyleSheet};
use lightningcss::traits::ToCss;

mod value;

pub use value::Color;
use value::{Length, parse_color, parse_length};

pub const TERMINAL: &str = include_str!("../themes/terminal.css");

/// The built-in themes by name, default first.
pub const BUILT_IN: &[(&str, &str)] = &[
    ("Terminal", TERMINAL),
    ("Manuscript", include_str!("../themes/manuscript.css")),
    ("Studio", include_str!("../themes/studio.css")),
    ("Editorial", include_str!("../themes/editorial.css")),
];

/// Fonts the built-in themes rely on, all under the SIL Open Font License (see `fonts/*/OFL.txt`).
pub const BUNDLED_FONTS: &[&[u8]] = &[
    include_bytes!("../fonts/jetbrains-mono/JetBrainsMono[wght].ttf"),
    include_bytes!("../fonts/jetbrains-mono/JetBrainsMono-Italic[wght].ttf"),
    include_bytes!("../fonts/source-serif-4/SourceSerif4[opsz,wght].ttf"),
    include_bytes!("../fonts/source-serif-4/SourceSerif4-Italic[opsz,wght].ttf"),
    include_bytes!("../fonts/ibm-plex-mono/IBMPlexMono-Regular.ttf"),
    include_bytes!("../fonts/ibm-plex-mono/IBMPlexMono-Italic.ttf"),
    include_bytes!("../fonts/ibm-plex-mono/IBMPlexMono-Medium.ttf"),
    include_bytes!("../fonts/ibm-plex-mono/IBMPlexMono-SemiBold.ttf"),
    include_bytes!("../fonts/geist/Geist[wght].ttf"),
    include_bytes!("../fonts/geist-mono/GeistMono[wght].ttf"),
    include_bytes!("../fonts/fraunces/Fraunces[SOFT,WONK,opsz,wght].ttf"),
    include_bytes!("../fonts/fraunces/Fraunces-Italic[SOFT,WONK,opsz,wght].ttf"),
    include_bytes!("../fonts/instrument-sans/InstrumentSans[wdth,wght].ttf"),
    include_bytes!("../fonts/instrument-sans/InstrumentSans-Italic[wdth,wght].ttf"),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Appearance {
    Light,
    Dark,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub line: u32,
    pub message: String,
}

#[derive(Clone, Debug)]
struct Selector {
    element: Option<String>,
    classes: Vec<String>,
}

impl Selector {
    fn parse(text: &str) -> Option<Self> {
        // Descendant selectors are only supported under the document root.
        let mut parts: Vec<&str> = text.split_whitespace().collect();
        if parts.len() == 2 && matches!(parts[0], "#write" | "body" | ":root") {
            parts.remove(0);
        }
        let [compound] = parts.as_slice() else { return None };
        let compound = match *compound {
            "#write" | "body" | ":root" | "html" => "body",
            c => c,
        };
        let mut pieces = compound.split('.');
        let element = pieces.next().filter(|e| !e.is_empty()).map(str::to_owned);
        let classes: Vec<String> = pieces.map(str::to_owned).collect();
        let valid = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if element.as_deref().is_some_and(|e| !valid(e)) || classes.iter().any(|c| !valid(c)) {
            return None;
        }
        Some(Self { element, classes })
    }

    fn specificity(&self) -> u32 {
        self.classes.len() as u32 * 10 + u32::from(self.element.is_some())
    }

    fn matches(&self, element: &str, classes: &[&str]) -> bool {
        self.element.as_deref().is_none_or(|e| e == element)
            && self.classes.iter().all(|c| classes.contains(&c.as_str()))
    }
}

#[derive(Clone, Debug)]
struct Rule {
    selector: Selector,
    declarations: Vec<(String, String)>,
    order: usize,
}

/// A parsed theme. Cheap to query; resolve styles per element with [`Theme::style`].
#[derive(Clone, Debug)]
pub struct Theme {
    vars: HashMap<String, String>,
    vars_light: HashMap<String, String>,
    vars_dark: HashMap<String, String>,
    rules: Vec<Rule>,
    pub diagnostics: Vec<Diagnostic>,
}

const SUPPORTED: &[&str] = &[
    "font-family",
    "font-size",
    "font-weight",
    "font-style",
    "font-variant",
    "color",
    "background",
    "background-color",
    "line-height",
    "letter-spacing",
    "text-transform",
    "text-decoration",
    "margin",
    "margin-top",
    "margin-bottom",
    "padding",
    "border-left",
    "border-bottom",
    "border-radius",
    "opacity",
];

impl Theme {
    pub fn parse(css: &str) -> Result<Self, String> {
        let sheet = StyleSheet::parse(css, ParserOptions::default()).map_err(|e| e.to_string())?;
        let mut theme = Theme {
            vars: HashMap::new(),
            vars_light: HashMap::new(),
            vars_dark: HashMap::new(),
            rules: Vec::new(),
            diagnostics: Vec::new(),
        };
        theme.collect(&sheet.rules.0, None);
        Ok(theme)
    }

    pub fn terminal() -> Self {
        Self::parse(TERMINAL).expect("built-in theme parses")
    }

    /// A built-in theme by name.
    pub fn built_in(name: &str) -> Option<Self> {
        BUILT_IN.iter().find(|(n, _)| *n == name).map(|(_, css)| Self::parse(css).expect("built-in theme parses"))
    }

    fn collect(&mut self, rules: &[CssRule], media: Option<Appearance>) {
        for rule in rules {
            match rule {
                CssRule::Style(style) => {
                    let line = style.loc.line + 1;
                    let mut declarations = Vec::new();
                    let all = style.declarations.declarations.iter().chain(&style.declarations.important_declarations);
                    for prop in all {
                        let name = prop.property_id().name().to_owned();
                        let Ok(value) = prop.value_to_css_string(PrinterOptions::default()) else { continue };
                        declarations.push((name, value));
                    }
                    for sel in style.selectors.0.iter() {
                        let text = sel.to_css_string(PrinterOptions::default()).unwrap_or_default();
                        let Some(selector) = Selector::parse(&text) else {
                            self.diagnostics
                                .push(Diagnostic { line, message: format!("unsupported selector `{text}`") });
                            continue;
                        };
                        let is_root = selector.element.as_deref() == Some("body") && selector.classes.is_empty();
                        let mut kept = Vec::new();
                        for (name, value) in &declarations {
                            if name.starts_with("--") {
                                if is_root {
                                    let vars = match media {
                                        None => &mut self.vars,
                                        Some(Appearance::Light) => &mut self.vars_light,
                                        Some(Appearance::Dark) => &mut self.vars_dark,
                                    };
                                    vars.insert(name.clone(), value.trim().to_owned());
                                } else {
                                    self.diagnostics.push(Diagnostic {
                                        line,
                                        message: format!("variable {name} is only supported on :root"),
                                    });
                                }
                            } else if !SUPPORTED.contains(&name.as_str()) {
                                self.diagnostics
                                    .push(Diagnostic { line, message: format!("unsupported property `{name}`") });
                            } else if media.is_some() {
                                self.diagnostics.push(Diagnostic {
                                    line,
                                    message: format!("`{name}` inside @media is not supported yet; use variables"),
                                });
                            } else {
                                kept.push((name.clone(), value.clone()));
                            }
                        }
                        if !kept.is_empty() {
                            let order = self.rules.len();
                            self.rules.push(Rule { selector, declarations: kept, order });
                        }
                    }
                }
                CssRule::Media(m) => {
                    let query = m.query.to_css_string(PrinterOptions::default()).unwrap_or_default();
                    let appearance = if query.contains("prefers-color-scheme: dark") {
                        Some(Appearance::Dark)
                    } else if query.contains("prefers-color-scheme: light") {
                        Some(Appearance::Light)
                    } else {
                        self.diagnostics.push(Diagnostic {
                            line: m.loc.line + 1,
                            message: format!("unsupported media query `{query}`"),
                        });
                        continue;
                    };
                    self.collect(&m.rules.0, appearance);
                }
                CssRule::Ignored => {}
                other => {
                    let line = match other {
                        CssRule::FontFace(f) => f.loc.line + 1,
                        _ => 0,
                    };
                    self.diagnostics.push(Diagnostic { line, message: "unsupported at-rule".into() });
                }
            }
        }
    }

    /// A `--variable` for the given appearance, after resolving nested `var()`s.
    pub fn var(&self, appearance: Appearance, name: &str) -> Option<String> {
        let raw = self.raw_var(appearance, name)?;
        Some(self.substitute(appearance, raw, 0))
    }

    fn raw_var(&self, appearance: Appearance, name: &str) -> Option<&str> {
        let specific = match appearance {
            Appearance::Light => &self.vars_light,
            Appearance::Dark => &self.vars_dark,
        };
        specific.get(name).or_else(|| self.vars.get(name)).map(String::as_str)
    }

    fn substitute(&self, appearance: Appearance, value: &str, depth: u8) -> String {
        if depth > 8 || !value.contains("var(") {
            return value.to_owned();
        }
        let mut out = String::new();
        let mut rest = value;
        while let Some(i) = rest.find("var(") {
            out.push_str(&rest[..i]);
            let after = &rest[i + 4..];
            let close = after.find(')').unwrap_or(after.len());
            let inner = &after[..close];
            let (name, fallback) = match inner.split_once(',') {
                Some((n, f)) => (n.trim(), Some(f.trim())),
                None => (inner.trim(), None),
            };
            let resolved = self.raw_var(appearance, name).or(fallback).unwrap_or("");
            out.push_str(&self.substitute(appearance, resolved, depth + 1));
            rest = after.get(close + 1..).unwrap_or("");
        }
        out.push_str(rest);
        out
    }

    /// Computed style for `element` with `classes`, inheriting from `parent`.
    pub fn style(&self, appearance: Appearance, element: &str, classes: &[&str], parent: Option<&Style>) -> Style {
        let mut style = match parent {
            Some(p) => p.inherit(),
            None => Style::default(),
        };
        let root_size = self.root_font_size(appearance);
        let mut matching: Vec<&Rule> = self.rules.iter().filter(|r| r.selector.matches(element, classes)).collect();
        matching.sort_by_key(|r| (r.selector.specificity(), r.order));
        let parent_size = parent.map_or(16.0, |p| p.font_size);
        for rule in matching {
            for (name, value) in &rule.declarations {
                let value = self.substitute(appearance, value, 0);
                style.apply(name, &value, parent_size, root_size);
            }
        }
        style
    }

    fn root_font_size(&self, appearance: Appearance) -> f32 {
        let mut s = Style::default();
        for rule in self.rules.iter().filter(|r| r.selector.matches("body", &[])) {
            for (name, value) in rule.declarations.iter().filter(|(n, _)| n == "font-size") {
                s.apply(name, &self.substitute(appearance, value, 0), 16.0, 16.0);
            }
        }
        s.font_size
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextTransform {
    #[default]
    None,
    Uppercase,
    Lowercase,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Border {
    pub width: f32,
    pub color: Color,
}

/// Fully resolved style for one element. Lengths are in logical pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Style {
    // Inherited.
    pub font_family: String,
    pub font_size: f32,
    pub font_weight: f32,
    pub italic: bool,
    pub small_caps: bool,
    pub color: Color,
    /// Multiple of font size.
    pub line_height: f32,
    pub letter_spacing: f32,
    pub text_transform: TextTransform,
    // Not inherited.
    pub background: Option<Color>,
    pub underline: bool,
    pub strikethrough: bool,
    pub margin_top: f32,
    pub margin_bottom: f32,
    /// Top, right, bottom, left.
    pub padding: [f32; 4],
    pub border_left: Option<Border>,
    pub border_bottom: Option<Border>,
    pub border_radius: f32,
    pub opacity: f32,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            font_family: "system-ui".into(),
            font_size: 16.0,
            font_weight: 400.0,
            italic: false,
            small_caps: false,
            color: Color::BLACK,
            line_height: 1.5,
            letter_spacing: 0.0,
            text_transform: TextTransform::None,
            background: None,
            underline: false,
            strikethrough: false,
            margin_top: 0.0,
            margin_bottom: 0.0,
            padding: [0.0; 4],
            border_left: None,
            border_bottom: None,
            border_radius: 0.0,
            opacity: 1.0,
        }
    }
}

impl Style {
    fn inherit(&self) -> Self {
        Self {
            font_family: self.font_family.clone(),
            font_size: self.font_size,
            font_weight: self.font_weight,
            italic: self.italic,
            small_caps: self.small_caps,
            color: self.color,
            line_height: self.line_height,
            letter_spacing: self.letter_spacing,
            text_transform: self.text_transform,
            ..Self::default()
        }
    }

    fn apply(&mut self, name: &str, value: &str, parent_size: f32, root_size: f32) {
        let len = |v: &str, basis: f32| parse_length(v).map(|l| l.resolve(basis, root_size));
        match name {
            "font-family" => self.font_family = value.to_owned(),
            "font-size" => {
                if let Some(px) = len(value, parent_size) {
                    self.font_size = px;
                }
            }
            "font-weight" => {
                self.font_weight = match value {
                    "normal" => 400.0,
                    "bold" => 700.0,
                    v => v.parse().unwrap_or(self.font_weight),
                }
            }
            "font-style" => self.italic = matches!(value, "italic" | "oblique"),
            "font-variant" => self.small_caps = value.contains("small-caps"),
            "color" => self.color = parse_color(value).unwrap_or(self.color),
            "background" | "background-color" => self.background = parse_color(value),
            "line-height" => {
                self.line_height = match parse_length(value) {
                    Some(Length::Number(n)) => n,
                    Some(l) => l.resolve(self.font_size, root_size) / self.font_size,
                    None => self.line_height,
                }
            }
            "letter-spacing" => self.letter_spacing = len(value, self.font_size).unwrap_or(0.0),
            "text-transform" => {
                self.text_transform = match value {
                    "uppercase" => TextTransform::Uppercase,
                    "lowercase" => TextTransform::Lowercase,
                    _ => TextTransform::None,
                }
            }
            "text-decoration" => {
                self.underline = value.contains("underline");
                self.strikethrough = value.contains("line-through");
            }
            "margin" => {
                let parts: Vec<f32> = value.split_whitespace().filter_map(|v| len(v, self.font_size)).collect();
                let [top, bottom] = match parts.as_slice() {
                    [a] => [*a, *a],
                    [a, _] => [*a, *a],
                    [a, _, c] | [a, _, c, _] => [*a, *c],
                    _ => return,
                };
                self.margin_top = top;
                self.margin_bottom = bottom;
            }
            "margin-top" => self.margin_top = len(value, self.font_size).unwrap_or(0.0),
            "margin-bottom" => self.margin_bottom = len(value, self.font_size).unwrap_or(0.0),
            "padding" => {
                let p: Vec<f32> = value.split_whitespace().filter_map(|v| len(v, self.font_size)).collect();
                self.padding = match p.as_slice() {
                    [a] => [*a; 4],
                    [a, b] => [*a, *b, *a, *b],
                    [a, b, c] => [*a, *b, *c, *b],
                    [a, b, c, d] => [*a, *b, *c, *d],
                    _ => self.padding,
                };
            }
            "border-left" | "border-bottom" => {
                let mut width = None;
                let mut color = None;
                for part in value.split_whitespace() {
                    if let Some(w) = len(part, self.font_size) {
                        width = Some(w);
                    } else if let Some(c) = parse_color(part) {
                        color = Some(c);
                    }
                }
                let border = match (width, color) {
                    (Some(w), Some(c)) if w > 0.0 => Some(Border { width: w, color: c }),
                    (Some(w), None) if w > 0.0 => Some(Border { width: w, color: self.color }),
                    _ => None,
                };
                if name == "border-left" {
                    self.border_left = border;
                } else {
                    self.border_bottom = border;
                }
            }
            "border-radius" => self.border_radius = len(value, self.font_size).unwrap_or(0.0),
            "opacity" => self.opacity = value.parse().unwrap_or(1.0),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_themes_are_clean() {
        for (name, _) in BUILT_IN {
            let theme = Theme::built_in(name).unwrap();
            assert!(theme.diagnostics.is_empty(), "{name}: {:#?}", theme.diagnostics);
            for appearance in [Appearance::Light, Appearance::Dark] {
                let body = theme.style(appearance, "body", &[], None);
                assert!(body.background.is_some(), "{name} sets a background");
                assert_ne!(body.background, Some(body.color), "{name} text is visible");
            }
        }
    }

    #[test]
    fn terminal_theme_is_clean() {
        let theme = Theme::terminal();
        assert!(theme.diagnostics.is_empty(), "{:#?}", theme.diagnostics);
        let body = theme.style(Appearance::Dark, "body", &[], None);
        assert_eq!(body.color, parse_color("#c9d1d9").unwrap());
        let h1 = theme.style(Appearance::Dark, "h1", &[], Some(&body));
        assert!(h1.font_size > body.font_size);
        assert_eq!(h1.font_family, body.font_family, "font family inherits");
        let light = theme.style(Appearance::Light, "body", &[], None);
        assert_ne!(light.background, body.background);
    }

    #[test]
    fn typora_style_selectors_and_diagnostics() {
        let theme = Theme::parse(
            "#write { color: #111; font-size: 18px } #write h2 { font-size: 1.5em } \
             p:hover { color: red } h1 { display: flex } @media print { h1 { color: red } }",
        )
        .unwrap();
        let body = theme.style(Appearance::Light, "body", &[], None);
        assert_eq!(body.font_size, 18.0);
        assert_eq!(theme.style(Appearance::Light, "h2", &[], Some(&body)).font_size, 27.0);
        let messages: Vec<_> = theme.diagnostics.iter().map(|d| d.message.as_str()).collect();
        assert!(messages.iter().any(|m| m.contains("p:hover")), "{messages:?}");
        assert!(messages.iter().any(|m| m.contains("display")), "{messages:?}");
        assert!(messages.iter().any(|m| m.contains("print")), "{messages:?}");
    }

    #[test]
    fn variables_resolve_per_appearance() {
        let theme = Theme::parse(
            ":root { --ink: #000; --text: var(--ink) } \
             @media (prefers-color-scheme: dark) { :root { --ink: #fff } } \
             p { color: var(--text) }",
        )
        .unwrap();
        assert_eq!(theme.style(Appearance::Light, "p", &[], None).color, Color::BLACK);
        assert_eq!(theme.style(Appearance::Dark, "p", &[], None).color, Color::WHITE);
    }
}
