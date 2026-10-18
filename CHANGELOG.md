# Changelog

All notable changes to Marginal are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- Lossless Markdown editing with margin glyphs, caret-revealed inline marks and ⌥ x-ray.
- Hot exit: unsaved buffers and the full undo tree survive restarts and crashes.
- Markdown autoformat while typing; demote-then-join Backspace; `\` hard breaks.
- Tables edited as a grid, with Tab/Return navigation, grips, border ⊕ handles, a column menu,
  aligned-table re-padding and spreadsheet paste.
- Code block language picker and tree-sitter highlighting for 16 languages.
- Four built-in themes (Terminal, Manuscript, Studio, Editorial) with their OFL fonts bundled, a
  user themes folder with live reload, label-style margin glyphs and theme-chosen bullets.
- Native menu bar; Open Recent, Reveal in Finder, Copy Path; text zoom; light/dark override;
  latched x-ray; Go to Heading.
- Find & replace with optional regex, case and whole-word matching.
- Theme gallery (⇧⌘T): search built-in, installed and Typora-gallery themes, live preview on
  the open document, fit scores, one-step install and removal.
- Theme engine: browser-style base styles, Typora class aliases, table descendant selectors,
  padding longhands, `text-align`, and a fit score per stylesheet.
- Typora theme compatibility: real CSS selectors (descendant, child, `:first-child`,
  `:nth-child`, `:not()`), variables on any element, dark-mode rules, borders on all sides, box
  shadows, page width, side margins, `text-indent`, `list-style-type`, the `font` shorthand,
  `hsl()`/`oklch()`/named colors, `@import`, `::before`/`::after` decorations (syntax-restating
  ones are dropped), heading counters, and fonts shipped with a theme (TTF, OTF, WOFF, WOFF2).
  Across the 229-theme Typora gallery: 119 good fits (was 11), median fit 82% (was 56%).
- Gallery downloads fetch only a theme's stylesheets and fonts from GitHub repositories, fall
  back to the homepage link, and skip broken CSS rules instead of rejecting a theme.
- `MARGINAL_TRACE` timing for main-thread work.
- Numbered lists drawn as muted `1.` (themes can choose circled or padded numbers).

### Fixed

- ↑ no longer gets stuck at the top of a block.
- ← / → reset the column that ↑ / ↓ remember.
- Scrolling the theme gallery with the mouse no longer jumps back to the selected theme.
- Journal flushes and session saves no longer run on the main thread, and the menu bar is only
  updated when its state changes.
