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
- Numbered lists drawn as muted `1.` (themes can choose circled or padded numbers).

### Fixed

- ↑ no longer gets stuck at the top of a block.
- ← / → reset the column that ↑ / ↓ remember.
