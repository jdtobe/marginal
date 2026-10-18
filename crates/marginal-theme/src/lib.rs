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

mod decor;
mod selector;
mod value;

pub use decor::{ContentPart, Decoration, FontFace, is_icon_glyph, restates_syntax};

use selector::Selector;
pub use selector::{Node, PseudoElement};

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

/// How much of a stylesheet Marginal can honor, for judging themes written for other apps.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Fit {
    /// Declarations (per selector) Marginal applies.
    pub honored: usize,
    /// All declarations (per selector) in the sheet.
    pub total: usize,
    /// Dropped declarations by feature, most common first: `("web fonts", 3)`.
    pub missing: Vec<(String, usize)>,
}

impl Fit {
    pub fn ratio(&self) -> f32 {
        if self.total == 0 { 1.0 } else { self.honored as f32 / self.total as f32 }
    }

    pub fn label(&self) -> &'static str {
        match self.ratio() {
            r if r >= 0.8 => "Good fit",
            r if r >= 0.5 => "Partial",
            _ => "Poor fit",
        }
    }

    /// "no web fonts, background images": the top missing features in a phrase.
    pub fn summary(&self, top: usize) -> String {
        self.missing.iter().take(top).map(|(f, _)| f.as_str()).collect::<Vec<_>>().join(", ")
    }
}

/// Selectors that style another app's own interface (Typora's sidebar, menus, source mode,
/// dialogs), not the document. They don't count for or against a theme's fit.
pub fn is_app_chrome(selector: &str) -> bool {
    const MARKERS: &[&str] = &[
        "#typora-",
        ".typora-",
        ".ty-",
        ".md-focus",
        ".on-focus-mode",
        ".btn",
        ".megamenu",
        ".modal",
        ".dropdown",
        ".file-",
        ".sidebar",
        ".outline-",
        ".nav-group",
        "#md-searchpanel",
        "#md-notification",
        ".md-toc",
        "#toc",
        ".CodeMirror",
        ".cm-",
        ".code-tooltip",
        ".active",
        ".divider",
        ".input-group",
        ".html-for-mac",
        ".context-menu",
        ".md-meta",
        ".md-diagram-panel",
        ".md-mathjax",
        ".md-inline-math",
        ".md-arrow",
        ".form-control",
        ".popover",
        ".window-",
        ".mac-seamless-mode",
        "::-webkit-scrollbar",
        "::selection",
        ".pin-outline",
        ".footnotes-area",
        ".md-rawblock",
        ".md-expand",
        ".md-hover",
        ".unibody-window",
        "#footer-word-count",
        "#sidebar",
        ".md-tag",
        "header",
        "footer",
        "html",
        "body.",
        "content",
        ".md-content",
        "#write-",
        ":hover",
        ".menu-",
    ];
    MARKERS.iter().any(|m| selector.contains(m))
}

/// Properties with no visible effect in Marginal (or browser-specific spellings of ones that
/// have): not counted for or against a theme's fit.
fn is_inert(property: &str) -> bool {
    property.starts_with("-webkit-")
        || property.starts_with("-moz-")
        || property.starts_with("-ms-")
        || property.starts_with("page-break")
        || property.starts_with("break-")
        || property.starts_with("outline")
        || property.starts_with("transition")
        || property.starts_with("animation")
        || property.starts_with("overflow")
        || property.starts_with("stroke")
        || property.starts_with("fill")
        || matches!(
            property,
            "z-index"
                | "appearance"
                | "caret-color"
                | "scroll-behavior"
                | "scrollbar-width"
                | "scrollbar-color"
                | "border-collapse"
                | "border-spacing"
                | "backface-visibility"
                | "cursor"
                | "box-sizing"
                | "word-wrap"
                | "overflow-wrap"
                | "word-break"
                | "text-rendering"
                | "user-select"
                | "orphans"
                | "widows"
                | "resize"
                | "pointer-events"
                | "will-change"
                | "zoom"
                | "font-smoothing"
                | "font-feature-settings"
                | "tab-size"
                | "hyphens"
                | "quotes"
        )
}

/// A stylesheet under every theme, like a browser's built-in styles: themes written for other
/// apps (Typora) rely on bold `strong`, italic `em`, monospace code and sized headings.
const BASE: &str = r#"
body { font-family: -apple-system, "Helvetica Neue", sans-serif; font-size: 16px; line-height: 1.6;
         color: #333333; background: #ffffff; }
p { margin-bottom: 16px; }
h1, h2, h3, h4, h5, h6 { font-weight: 700; line-height: 1.3; margin-top: 16px; margin-bottom: 12px; }
h1 { font-size: 2em; } h2 { font-size: 1.5em; } h3 { font-size: 1.25em; }
h4 { font-size: 1em; } h5 { font-size: 0.875em; } h6 { font-size: 0.85em; }
strong { font-weight: 700; }
em { font-style: italic; }
del { text-decoration: line-through; }
a { color: #4183c4; text-decoration: underline; }
code { font-family: "SF Mono", Menlo, monospace; }
pre { font-family: "SF Mono", Menlo, monospace; margin-bottom: 16px; padding: 8px 12px; }
blockquote { padding: 0 0 0 14px; border-left: 3px solid #cccccc; }
hr { border-bottom: 1px solid #cccccc; margin-bottom: 16px; }
li { margin-bottom: 4px; }
th { font-weight: 700; padding: 4px 10px; border-bottom: 2px solid #999999; }
td { padding: 4px 10px; border-bottom: 1px solid #cccccc; }
.glyph { color: #999999; font-size: 12px; }
.syntax { color: #999999; }
"#;

/// Replaces `@import "relative.css";` with that file's contents, recursively (to a small depth).
/// Remote imports (web fonts from a CDN) are left for the parser to report.
fn inline_imports(css: &str, base: &std::path::Path, depth: u8) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(i) = rest.find("@import") {
        out.push_str(&rest[..i]);
        let after = &rest[i..];
        let end = after.find(';').map_or(after.len(), |e| e + 1);
        let statement = &after[..end];
        let target = statement["@import".len()..]
            .trim()
            .trim_start_matches("url(")
            .trim_start_matches(['"', '\''])
            .split(['"', '\'', ')'])
            .next()
            .unwrap_or_default();
        let local = !target.contains("://") && !target.starts_with("//") && target.ends_with(".css");
        match std::fs::read_to_string(base.join(target)).ok().filter(|_| local && depth < 4) {
            Some(text) => {
                let dir = base.join(target).parent().map(std::path::Path::to_path_buf).unwrap_or_default();
                out.push_str(&inline_imports(&text, &dir, depth + 1));
            }
            None => out.push_str(statement),
        }
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

/// The feature a dropped declaration belongs to, in words a theme author would use.
fn feature_of(selector: Option<&str>, property: Option<&str>, value: Option<&str>) -> String {
    if selector.is_some_and(|s| s.contains("::") || s.contains(":before") || s.contains(":after")) {
        return "::before/::after decorations".into();
    }
    if value.is_some_and(|v| v.contains("url(")) {
        return "background images".into();
    }
    if let Some(p) = property {
        return match p {
            "box-shadow" | "text-shadow" => "shadows".into(),
            p if p.starts_with("transition") || p.starts_with("animation") => "animation".into(),
            p if p.starts_with("background") => "background images".into(),
            p if p.starts_with("border") => "borders".into(),
            "width" | "max-width" | "min-width" | "height" | "position" | "display" | "float" | "top" | "left"
            | "right" | "bottom" => "layout".into(),
            p => format!("`{p}`"),
        };
    }
    "unsupported selectors".into()
}

#[derive(Clone, Debug)]
struct Rule {
    selector: Selector,
    declarations: Vec<(String, String)>,
    order: usize,
    /// Set for rules inside `@media (prefers-color-scheme: …)`.
    appearance: Option<Appearance>,
}

/// A parsed theme. Cheap to query; resolve styles per element with [`Theme::style`].
#[derive(Clone, Debug)]
pub struct Theme {
    vars: HashMap<String, String>,
    vars_light: HashMap<String, String>,
    vars_dark: HashMap<String, String>,
    rules: Vec<Rule>,
    /// `::before`/`::after` rules, for decorations.
    pseudo_rules: Vec<Rule>,
    /// Fonts the theme ships with, and the folder their paths are relative to.
    pub font_faces: Vec<FontFace>,
    pub base_dir: Option<std::path::PathBuf>,
    pub diagnostics: Vec<Diagnostic>,
    pub fit: Fit,
    /// Dropped declarations by feature, while collecting.
    dropped: HashMap<String, usize>,
}

/// Properties that only matter on `::before`/`::after` boxes.
const DECORATION: &[&str] = &["content", "display", "position", "width", "height", "left", "top", "right"];

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
    "padding-top",
    "padding-right",
    "padding-bottom",
    "padding-left",
    "text-align",
    "border",
    "border-top",
    "border-right",
    "border-left",
    "border-bottom",
    "border-color",
    "border-top-color",
    "border-right-color",
    "border-bottom-color",
    "border-left-color",
    "border-width",
    "border-top-width",
    "border-right-width",
    "border-bottom-width",
    "border-left-width",
    "border-style",
    "margin-left",
    "margin-right",
    "max-width",
    "text-indent",
    "list-style",
    "list-style-type",
    "box-shadow",
    "font",
    "border-radius",
    "opacity",
];

impl Theme {
    pub fn parse(css: &str) -> Result<Self, String> {
        // Skip broken rules instead of rejecting the sheet, as browsers do: themes written for
        // other apps often have a stray syntax error that their app tolerated.
        let warnings = std::sync::Arc::new(std::sync::RwLock::new(Vec::new()));
        let options =
            ParserOptions { error_recovery: true, warnings: Some(warnings.clone()), ..ParserOptions::default() };
        let sheet = StyleSheet::parse(css, options).map_err(|e| e.to_string())?;
        let skipped: Vec<Diagnostic> = warnings
            .read()
            .map(|w| {
                w.iter()
                    .map(|e| Diagnostic {
                        line: e.loc.as_ref().map_or(0, |l| l.line + 1),
                        message: format!("skipped invalid CSS: {}", e.kind),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut theme = Theme {
            vars: HashMap::new(),
            vars_light: HashMap::new(),
            vars_dark: HashMap::new(),
            rules: Vec::new(),
            pseudo_rules: Vec::new(),
            font_faces: Vec::new(),
            base_dir: None,
            diagnostics: Vec::new(),
            fit: Fit::default(),
            dropped: HashMap::new(),
        };
        // Base styles first (lower order, so the theme overrides them), without judging them.
        let base = StyleSheet::parse(BASE, ParserOptions::default()).expect("base stylesheet parses");
        theme.collect(&base.rules.0, None);
        theme.diagnostics.clear();
        theme.dropped.clear();
        theme.fit = Fit::default();
        theme.collect(&sheet.rules.0, None);
        theme.diagnostics.extend(skipped);
        let mut missing: Vec<(String, usize)> = std::mem::take(&mut theme.dropped).into_iter().collect();
        missing.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        theme.fit.missing = missing;
        Ok(theme)
    }

    pub fn terminal() -> Self {
        Self::parse(TERMINAL).expect("built-in theme parses")
    }

    /// Loads a theme file, inlining stylesheets it `@import`s by relative path (themes often
    /// share a base sheet between their light and dark variants).
    pub fn load_file(path: &std::path::Path) -> Result<Self, String> {
        let css = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let base = path.parent().unwrap_or(std::path::Path::new("."));
        let mut theme = Self::parse(&inline_imports(&css, base, 0)).map_err(|e| format!("{}: {e}", path.display()))?;
        theme.base_dir = Some(base.to_path_buf());
        Ok(theme)
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
                        let fit_before = (self.fit.honored, self.fit.total, self.dropped.clone());
                        self.fit.total += declarations.len();
                        let Some(selector) = Selector::parse(&text) else {
                            self.diagnostics
                                .push(Diagnostic { line, message: format!("unsupported selector `{text}`") });
                            *self.dropped.entry(feature_of(Some(&text), None, None)).or_default() += declarations.len();
                            if is_app_chrome(&text) {
                                (self.fit.honored, self.fit.total, self.dropped) = fit_before;
                            }
                            continue;
                        };
                        let is_root = selector.is_root();
                        if selector.pseudo_element.is_some() {
                            for (name, value) in &declarations {
                                if DECORATION.contains(&name.as_str())
                                    || name.starts_with("--")
                                    || (SUPPORTED.contains(&name.as_str()) && !value.contains("url("))
                                {
                                    self.fit.honored += 1;
                                } else if is_inert(name) {
                                    self.fit.total -= 1;
                                } else {
                                    *self.dropped.entry(feature_of(None, Some(name), Some(value))).or_default() += 1;
                                }
                            }
                            let order = self.rules.len();
                            self.pseudo_rules.push(Rule {
                                selector,
                                declarations: declarations.clone(),
                                order,
                                appearance: media,
                            });
                            if is_app_chrome(&text) {
                                (self.fit.honored, self.fit.total, self.dropped) = fit_before;
                            }
                            continue;
                        }
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
                                    self.fit.honored += 1;
                                } else {
                                    // Scoped variables cascade and inherit like other properties.
                                    self.fit.honored += 1;
                                    kept.push((name.clone(), value.trim().to_owned()));
                                }
                            } else if is_inert(name) {
                                self.fit.total -= 1;
                            } else if !SUPPORTED.contains(&name.as_str()) || value.contains("url(") {
                                self.diagnostics
                                    .push(Diagnostic { line, message: format!("unsupported property `{name}`") });
                                *self.dropped.entry(feature_of(None, Some(name), Some(value))).or_default() += 1;
                            } else {
                                self.fit.honored += 1;
                                kept.push((name.clone(), value.clone()));
                            }
                        }
                        if !kept.is_empty() {
                            let order = self.rules.len();
                            self.rules.push(Rule { selector, declarations: kept, order, appearance: media });
                        }
                        if is_app_chrome(&text) {
                            (self.fit.honored, self.fit.total, self.dropped) = fit_before;
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
                CssRule::FontFace(f) => {
                    let text = f.to_css_string(PrinterOptions::default()).unwrap_or_default();
                    self.fit.total += 1;
                    match decor::parse_font_face(&text) {
                        Some(face) => {
                            self.fit.honored += 1;
                            self.font_faces.push(face);
                        }
                        None => *self.dropped.entry("web fonts".into()).or_default() += 1,
                    }
                }
                other => {
                    let (line, feature) = match other {
                        CssRule::FontFace(f) => (f.loc.line + 1, "web fonts"),
                        CssRule::Import(i) => (i.loc.line + 1, "@import"),
                        _ => (0, "other at-rules"),
                    };
                    self.fit.total += 1;
                    *self.dropped.entry(feature.into()).or_default() += 1;
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
        self.substitute_in(appearance, value, &[], depth)
    }

    /// Resolves `var(--x, fallback)`, looking in `scoped` (variables set on the element or its
    /// ancestors, nearest last) before the theme's root variables.
    fn substitute_in(&self, appearance: Appearance, value: &str, scoped: &[(String, String)], depth: u8) -> String {
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
            let local = scoped.iter().rev().find(|(n, _)| n == name).map(|(_, v)| v.as_str());
            let resolved = local.or_else(|| self.raw_var(appearance, name)).or(fallback).unwrap_or("");
            out.push_str(&self.substitute_in(appearance, resolved, scoped, depth + 1));
            rest = after.get(close + 1..).unwrap_or("");
        }
        out.push_str(rest);
        out
    }

    /// Computed style for `element` with `classes` directly in the page, inheriting from
    /// `parent`. Use [`Theme::style_at`] when the element's ancestors are known.
    pub fn style(&self, appearance: Appearance, element: &str, classes: &[&str], parent: Option<&Style>) -> Style {
        let body = Node::new("body");
        if element == "body" {
            return self.style_at(appearance, &[body.with_classes(classes)], parent);
        }
        self.style_at(appearance, &[body, Node::new(element).with_classes(classes)], parent)
    }

    /// The `::before` and `::after` decorations of the last node of `path`, given its
    /// computed style. Only boxes with `content` exist, as in CSS.
    pub fn decorations(&self, appearance: Appearance, path: &[Node], element: &Style) -> Vec<Decoration> {
        let root_size = self.root_font_size(appearance);
        let mut out = Vec::new();
        for pseudo in [PseudoElement::Before, PseudoElement::After] {
            let mut matching: Vec<&Rule> = self
                .pseudo_rules
                .iter()
                .filter(|r| {
                    r.selector.pseudo_element == Some(pseudo)
                        && r.appearance.is_none_or(|a| a == appearance)
                        && r.selector.matches(path)
                })
                .collect();
            if matching.is_empty() {
                continue;
            }
            matching.sort_by_key(|r| (r.selector.specificity(), r.appearance.is_some(), r.order));
            let mut style = element.inherit();
            let mut d = Decoration {
                pseudo,
                content: Vec::new(),
                style: Style::default(),
                block: false,
                absolute: false,
                width: None,
                height: None,
                left: None,
                top: None,
                right: None,
            };
            let mut content = None;
            for rule in &matching {
                for (name, value) in rule.declarations.iter().filter(|(n, _)| n.starts_with("--")) {
                    style.vars.retain(|(n, _)| n != name);
                    style.vars.push((name.clone(), value.clone()));
                }
            }
            for rule in matching {
                for (name, value) in rule.declarations.iter().filter(|(n, _)| !n.starts_with("--")) {
                    let value = self.substitute_in(appearance, value, &style.vars, 0);
                    let v = value.trim().trim_end_matches("!important").trim();
                    let length = |v: &str| parse_length(v).map(|l| l.resolve(style.font_size, root_size));
                    match name.as_str() {
                        "content" => content = decor::parse_content(v),
                        "display" => d.block = matches!(v, "block" | "flex" | "table" | "list-item" | "grid"),
                        "position" => d.absolute = matches!(v, "absolute" | "fixed"),
                        "width" => d.width = length(v),
                        "height" => d.height = length(v),
                        "left" => d.left = length(v),
                        "top" => d.top = length(v),
                        "right" => d.right = length(v),
                        _ => style.apply(name, v, element.font_size, root_size),
                    }
                }
            }
            if let Some(content) = content {
                d.content = content;
                d.style = style;
                out.push(d);
            }
        }
        out
    }

    /// Computed style for the last node of `path` (the page first), inheriting from `parent`.
    pub fn style_at(&self, appearance: Appearance, path: &[Node], parent: Option<&Style>) -> Style {
        let mut style = match parent {
            Some(p) => p.inherit(),
            None => Style::default(),
        };
        let root_size = self.root_font_size(appearance);
        let mut matching: Vec<&Rule> = self
            .rules
            .iter()
            .filter(|r| r.appearance.is_none_or(|a| a == appearance) && r.selector.matches(path))
            .collect();
        matching.sort_by_key(|r| (r.selector.specificity(), r.appearance.is_some(), r.order));
        let parent_size = parent.map_or(16.0, |p| p.font_size);
        // Variables first, so declarations on the same element can use them.
        for rule in &matching {
            for (name, value) in rule.declarations.iter().filter(|(n, _)| n.starts_with("--")) {
                style.vars.retain(|(n, _)| n != name);
                style.vars.push((name.clone(), value.clone()));
            }
        }
        for rule in matching {
            for (name, value) in rule.declarations.iter().filter(|(n, _)| !n.starts_with("--")) {
                let value = self.substitute_in(appearance, value, &style.vars, 0);
                style.apply(name, &value, parent_size, root_size);
            }
        }
        style
    }

    fn root_font_size(&self, appearance: Appearance) -> f32 {
        let mut s = Style::default();
        let body = [Node::new("body")];
        for rule in
            self.rules.iter().filter(|r| r.appearance.is_none_or(|a| a == appearance) && r.selector.matches(&body))
        {
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextAlign {
    #[default]
    Start,
    Center,
    End,
    Justify,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Border {
    pub width: f32,
    pub color: Color,
}

/// A `box-shadow` (the first, non-inset one).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shadow {
    pub dx: f32,
    pub dy: f32,
    pub blur: f32,
    pub spread: f32,
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
    pub text_align: TextAlign,
    /// First-line indent.
    pub text_indent: f32,
    /// `list-style-type` (`decimal`, `disc`, `lower-alpha`…), when the theme sets one.
    pub list_style: Option<String>,
    /// Custom properties set on this element or inherited (`--name`, value), nearest last.
    pub vars: Vec<(String, String)>,
    // Not inherited.
    pub background: Option<Color>,
    pub underline: bool,
    pub strikethrough: bool,
    pub margin_top: f32,
    pub margin_bottom: f32,
    /// Top, right, bottom, left.
    pub padding: [f32; 4],
    pub margin_left: f32,
    pub margin_right: f32,
    pub border_top: Option<Border>,
    pub border_right: Option<Border>,
    pub border_left: Option<Border>,
    pub border_bottom: Option<Border>,
    pub border_radius: f32,
    pub box_shadow: Option<Shadow>,
    /// `max-width` (used on the page: how wide the text column may get).
    pub max_width: Option<f32>,
    pub opacity: f32,
}

/// Sides a border property sets: 0 top, 1 right, 2 bottom, 3 left.
fn sides(property: &str) -> Vec<usize> {
    match property {
        "border-top" => vec![0],
        "border-right" => vec![1],
        "border-bottom" => vec![2],
        "border-left" => vec![3],
        _ => vec![0, 1, 2, 3],
    }
}

/// The value for `side` from a 1–4 value box shorthand (`margin`, `border-color`…).
fn box_value<T: Copy>(values: &[T], side: usize) -> Option<T> {
    match (values.len(), side) {
        (0, _) => None,
        (1, _) => Some(values[0]),
        (2, s) => Some(values[s % 2]),
        (3, 3) => Some(values[1]),
        (3, s) => Some(values[s]),
        (_, s) => values.get(s).copied(),
    }
}

/// Splits a value on spaces, keeping `rgb(…)` and other functions whole.
fn split_value(value: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut depth = 0;
    for c in value.chars() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ => {}
        }
        if c.is_whitespace() && depth == 0 {
            if !current.is_empty() {
                parts.push(std::mem::take(&mut current));
            }
        } else {
            current.push(c);
        }
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
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
            text_align: TextAlign::Start,
            text_indent: 0.0,
            list_style: None,
            vars: Vec::new(),
            background: None,
            underline: false,
            strikethrough: false,
            margin_top: 0.0,
            margin_bottom: 0.0,
            padding: [0.0; 4],
            margin_left: 0.0,
            margin_right: 0.0,
            border_top: None,
            border_right: None,
            border_left: None,
            border_bottom: None,
            border_radius: 0.0,
            box_shadow: None,
            max_width: None,
            opacity: 1.0,
        }
    }
}

impl Style {
    fn border_mut(&mut self, side: usize) -> &mut Option<Border> {
        match side {
            0 => &mut self.border_top,
            1 => &mut self.border_right,
            2 => &mut self.border_bottom,
            _ => &mut self.border_left,
        }
    }

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
            text_align: self.text_align,
            text_indent: self.text_indent,
            list_style: self.list_style.clone(),
            vars: self.vars.clone(),
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
                // `auto` centers in CSS; here it means no extra side margin.
                let parts: Vec<f32> = value
                    .split_whitespace()
                    .filter_map(|v| if v == "auto" { Some(0.0) } else { len(v, self.font_size) })
                    .collect();
                if parts.is_empty() {
                    return;
                }
                self.margin_top = box_value(&parts, 0).unwrap_or(0.0);
                self.margin_right = box_value(&parts, 1).unwrap_or(0.0);
                self.margin_bottom = box_value(&parts, 2).unwrap_or(0.0);
                self.margin_left = box_value(&parts, 3).unwrap_or(0.0);
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
            "padding-top" | "padding-right" | "padding-bottom" | "padding-left" => {
                let side = ["padding-top", "padding-right", "padding-bottom", "padding-left"]
                    .iter()
                    .position(|p| *p == name)
                    .unwrap_or(0);
                if let Some(v) = len(value, self.font_size) {
                    self.padding[side] = v;
                }
            }
            "text-align" => {
                self.text_align = match value.trim() {
                    "center" => TextAlign::Center,
                    "right" | "end" => TextAlign::End,
                    "justify" => TextAlign::Justify,
                    _ => TextAlign::Start,
                }
            }
            "border" | "border-top" | "border-right" | "border-bottom" | "border-left" => {
                let mut width = None;
                let mut color = None;
                let mut none = false;
                for part in split_value(value) {
                    match part.as_str() {
                        "none" | "hidden" => none = true,
                        "thin" => width = Some(1.0),
                        "medium" => width = Some(3.0),
                        "thick" => width = Some(5.0),
                        p => {
                            if let Some(w) = len(p, self.font_size) {
                                width = Some(w);
                            } else if let Some(c) = parse_color(p) {
                                color = Some(c);
                            }
                        }
                    }
                }
                // A border style with no width is `medium` (3px) in CSS, but themes that write
                // `border: solid #ddd` mean a hairline.
                let border = match (none, width) {
                    (true, _) | (_, Some(0.0)) => None,
                    (false, w) => Some(Border { width: w.unwrap_or(1.0), color: color.unwrap_or(self.color) }),
                };
                for side in sides(name) {
                    *self.border_mut(side) = border;
                }
            }
            "border-color"
            | "border-top-color"
            | "border-right-color"
            | "border-bottom-color"
            | "border-left-color" => {
                let colors: Vec<Color> = split_value(value).iter().filter_map(|p| parse_color(p)).collect();
                for (i, side) in sides(&name.replace("-color", "")).into_iter().enumerate() {
                    let c = if name == "border-color" { box_value(&colors, side) } else { colors.get(i).copied() };
                    if let (Some(b), Some(c)) = (self.border_mut(side).as_mut(), c) {
                        b.color = c;
                    }
                }
            }
            "border-width"
            | "border-top-width"
            | "border-right-width"
            | "border-bottom-width"
            | "border-left-width" => {
                let widths: Vec<f32> = split_value(value).iter().filter_map(|p| len(p, self.font_size)).collect();
                for side in sides(&name.replace("-width", "")) {
                    let w = if name == "border-width" { box_value(&widths, side) } else { widths.first().copied() };
                    if let Some(w) = w {
                        let color = self.color;
                        let b = self.border_mut(side);
                        *b = (w > 0.0).then(|| Border { width: w, color: b.map_or(color, |b| b.color) });
                    }
                }
            }
            "border-style" => {
                if value.trim() == "none" || value.trim() == "hidden" {
                    for side in 0..4 {
                        *self.border_mut(side) = None;
                    }
                }
            }
            "margin-left" => self.margin_left = len(value, self.font_size).unwrap_or(0.0),
            "margin-right" => self.margin_right = len(value, self.font_size).unwrap_or(0.0),
            "max-width" => self.max_width = len(value, self.font_size),
            "text-indent" => self.text_indent = len(value, self.font_size).unwrap_or(0.0),
            "list-style-type" | "list-style" => {
                if let Some(t) =
                    split_value(value).into_iter().find(|p| !p.contains("url(") && p != "inside" && p != "outside")
                {
                    self.list_style = Some(t);
                }
            }
            "box-shadow" => {
                let first = value.split(',').next().unwrap_or_default();
                if value.trim() == "none" || first.contains("inset") {
                    self.box_shadow = None;
                } else {
                    let parts = split_value(first);
                    let lengths: Vec<f32> = parts.iter().filter_map(|p| len(p, self.font_size)).collect();
                    let color = parts.iter().find_map(|p| parse_color(p)).unwrap_or(Color { r: 0, g: 0, b: 0, a: 64 });
                    self.box_shadow = match lengths.as_slice() {
                        [dx, dy, rest @ ..] => Some(Shadow {
                            dx: *dx,
                            dy: *dy,
                            blur: rest.first().copied().unwrap_or(0.0),
                            spread: rest.get(1).copied().unwrap_or(0.0),
                            color,
                        }),
                        _ => None,
                    };
                }
            }
            "font" => {
                // [style] [weight] size[/line-height] family
                let parts = split_value(value);
                let Some(size_at) =
                    parts.iter().position(|p| len(p.split('/').next().unwrap_or_default(), self.font_size).is_some())
                else {
                    return;
                };
                for p in &parts[..size_at] {
                    match p.as_str() {
                        "italic" | "oblique" => self.italic = true,
                        "bold" | "bolder" => self.font_weight = 700.0,
                        n if n.parse::<f32>().is_ok() => self.font_weight = n.parse().unwrap(),
                        _ => {}
                    }
                }
                let (size, line) = match parts[size_at].split_once('/') {
                    Some((s, l)) => (s.to_owned(), Some(l.to_owned())),
                    None => (parts[size_at].clone(), None),
                };
                self.apply("font-size", &size, parent_size, root_size);
                if let Some(l) = line {
                    self.apply("line-height", &l, parent_size, root_size);
                }
                let family = parts[size_at + 1..].join(" ");
                if !family.is_empty() {
                    self.apply("font-family", &family, parent_size, root_size);
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
    fn fit_scores_typora_style_sheets() {
        let css = r##"
            @import url("theme/fonts.css");
            @font-face { font-family: "X"; src: url("x.woff2"); }
            #write { font-family: "X", serif; color: #333; max-width: 860px; }
            #write h1 { font-size: 2em; }
            #write h1::before { content: "#"; }
            .md-fences { background: #f6f8fa; box-shadow: 0 1px 2px #ccc; }
            body { background-image: url("paper.png"); }
        "##;
        let theme = Theme::parse(css).unwrap();
        assert_eq!((theme.fit.honored, theme.fit.total), (8, 10), "{:?}", theme.fit);
        assert_eq!(theme.fit.label(), "Good fit");
        assert_eq!(theme.font_faces.len(), 1);
        let features: Vec<&str> = theme.fit.missing.iter().map(|(f, _)| f.as_str()).collect();
        for f in ["@import", "background images"] {
            assert!(features.contains(&f), "{f} missing from {features:?}");
        }
        let pre = theme.style(Appearance::Light, "pre", &[], None);
        assert!(pre.background.is_some(), ".md-fences styles code blocks");
        for (name, _) in BUILT_IN {
            assert_eq!(Theme::built_in(name).unwrap().fit.label(), "Good fit", "{name}");
        }
    }

    #[test]
    fn imports_are_inlined() {
        let dir = std::env::temp_dir().join(format!("marginal-import-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("base")).unwrap();
        std::fs::write(dir.join("base/core.css"), "#write { color: #010203; }").unwrap();
        std::fs::write(dir.join("t.css"), "@import \"base/core.css\";\nh1 { color: #040506; }").unwrap();
        let theme = Theme::load_file(&dir.join("t.css")).unwrap();
        assert_eq!(theme.style(Appearance::Light, "body", &[], None).color, parse_color("#010203").unwrap());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn decorations_cascade() {
        let css = r##"blockquote::before { content: "\201C"; font-size: 3em; color: #42b983; position: absolute; left: -20px; }
            h2::after { content: ""; display: block; width: 40px; height: 3px; background: #42b983; }
            #write h2::after { width: 60px; }
            h1::before { content: counter(h1) ". "; }
            p::before { content: none; }"##;
        let t = Theme::parse(css).unwrap();
        let body = t.style(Appearance::Light, "body", &[], None);
        let at = |el: &str| {
            let path = [Node::new("body"), Node::new(el)];
            let style = t.style_at(Appearance::Light, &path, Some(&body));
            t.decorations(Appearance::Light, &path, &style)
        };
        let q = at("blockquote");
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].text(|_, _| String::new()), "“");
        assert!(q[0].absolute && q[0].left == Some(-20.0) && q[0].style.font_size == 48.0);
        let h2 = at("h2");
        assert!(h2[0].block && h2[0].is_shape() && h2[0].width == Some(60.0) && h2[0].height == Some(3.0));
        assert!(h2[0].style.background.is_some());
        assert_eq!(at("h1")[0].text(|name, _| if name == "h1" { "2".into() } else { "?".into() }), "2. ");
        assert!(at("p").is_empty(), "content: none makes no box");
    }

    #[test]
    fn box_model_properties() {
        let css = "#write { max-width: 860px; } pre { border: 1px solid #ddd; border-top-color: #f00; border-radius: 4px; \
                   box-shadow: 0 2px 6px rgba(0, 0, 0, 0.2); margin: 0 12px 16px; } \
                   h2 { border-bottom: none; border-left: 4px solid #42b983; font: italic bold 20px/1.4 Georgia, serif; } \
                   ol { list-style-type: lower-roman; } p { text-indent: 2em; }";
        let t = Theme::parse(css).unwrap();
        let body = t.style(Appearance::Light, "body", &[], None);
        assert_eq!(body.max_width, Some(860.0));
        let pre = t.style(Appearance::Light, "pre", &[], Some(&body));
        assert_eq!(pre.border_top.unwrap().color, parse_color("#f00").unwrap());
        assert_eq!(pre.border_left.unwrap().color, parse_color("#ddd").unwrap());
        assert_eq!((pre.margin_left, pre.margin_right, pre.margin_bottom), (12.0, 12.0, 16.0));
        assert!(pre.box_shadow.is_some_and(|s| s.blur == 6.0 && s.dy == 2.0));
        let h2 = t.style(Appearance::Light, "h2", &[], Some(&body));
        assert!(h2.border_bottom.is_none() && h2.border_left.is_some_and(|b| b.width == 4.0));
        assert!(h2.italic && h2.font_weight == 700.0 && h2.font_size == 20.0 && h2.font_family.contains("Georgia"));
        assert_eq!(t.style(Appearance::Light, "ol", &[], Some(&body)).list_style.as_deref(), Some("lower-roman"));
        assert_eq!(t.style(Appearance::Light, "p", &[], Some(&body)).text_indent, 32.0);
    }

    #[test]
    fn broken_rules_are_skipped_not_fatal() {
        let theme = Theme::parse("h1 > { color: red } #write { color: #123456; } , { }").unwrap();
        assert_eq!(theme.style(Appearance::Light, "body", &[], None).color, parse_color("#123456").unwrap());
        assert!(theme.diagnostics.iter().any(|d| d.message.starts_with("skipped invalid CSS")));
    }

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
