# Marginal

A WYSIWYG Markdown editor where the page stays a page. There are no toolbars: Markdown syntax
lives as faint *marginalia* beside each block, inline marks appear only where the caret is, and
editing controls grow out of the element you're working on (a table's grips, a code block's
language chip).

The `.md` file on disk is sacred. Marginal edits only the bytes you change, keeps your line
endings and spacing, and writes the file only when you press ⌘S. Everything else (unsaved
edits, full undo history, open windows) is journaled continuously and restored after a quit or
crash.

> **Status:** pre-alpha, macOS first. The core editing loop, tables, code highlighting and the
> hot-exit journal work. See [the design notes](docs/design.md) for what's decided and what's next.

## Features

- **Lossless editing.** Source text is the single truth; every command compiles to a minimal
  byte patch. Opening and saving without edits is byte-exact.
- **Marginalia.** `#`, `>`, fences and other block syntax sit in the margin. Hold ⌥ to x-ray the
  raw Markdown everywhere.
- **Hot exit.** Unsaved buffers, carets and the whole undo tree survive restarts and crashes.
- **Tables** edit as a grid: Tab between cells, Return for the cell below, grips to select, drag
  and right-click columns, ⊕ on borders to insert. Aligned tables stay aligned in the source;
  compact ones stay compact.
- **Code blocks** with tree-sitter highlighting for 16 languages and a language picker.
- **Themes** are a CSS subset (Typora-style), so the reader picks the look: *Terminal* (default),
  *Manuscript*, *Studio* and *Editorial* are built in, and any `.css` dropped into
  `~/Library/Application Support/Marginal/Themes/` shows up in View ▸ Theme and reloads when
  saved.
- **Find & replace** in a strip at the top of the window: plain text by default, with optional
  regex (`$1` groups in replacements), case and whole-word matching. Replace All is one undo step.
- **Native menus** for everything, plus Go to Heading, Open Recent, text zoom and a light/dark
  override.

## Building

Requires Rust 1.88+ (the repo pins stable via `rust-toolchain.toml`) and macOS 13+.

```sh
cargo run -p marginal-app -- fixtures/sample.md   # open a file
cargo run -p marginal-app                         # restore the last session
cargo test --workspace                            # all tests
```

Debug builds optimize dependencies (see `[profile.dev.package."*"]`), so `cargo run` stays
responsive while Marginal's own crates compile quickly.

### Snapshots

The app can render offscreen to a PNG, which is handy for checking visual changes:

```sh
cargo run -p marginal-app -- --snapshot out.png [--light] [--theme Studio] [--xray] [--caret 120] \
    [--find 'the' --replace 'THE' --regex] [--goto ''] fixtures/sample.md
```

## Keys

| Keys | Action |
|---|---|
| ⌘S, ⇧⌘S | Save (the only thing that writes the file), Save As |
| ⌘N, ⌘O | New, Open (File ▸ Open Recent lists recent files) |
| ⌘F, ⌥⌘F, ⌘G / ⇧⌘G | Find, Find and Replace, next / previous match |
| ⇧⌘O | Go to Heading |
| ⌘= / ⌘−, ⇧⌘0 | Zoom in / out, actual size |
| ⌥⌘X | Keep the Markdown source showing (latched x-ray) |
| ⌥⌘R, ⌥⇧⌘C | Reveal in Finder, Copy Path |
| ⌘Z / ⇧⌘Z | Undo / redo (persisted across restarts) |
| ⌘B ⌘I ⌘E ⇧⌘X | Bold, italic, code, strikethrough |
| ⌘K | Link (pasting a URL over a selection also links it) |
| ⌘1–⌘6, ⌘0, ⌘] ⌘[ | Heading level, paragraph, more/less prominent |
| ⇧Return | Hard line break (written as a trailing `\`) |
| Hold ⌥ | X-ray: show all Markdown syntax |
| ⌥⌘L | Code block language |
| Tab / ⇧Tab, Return | Table: next / previous cell, cell below |
| ⌥⌘↑ ↓ ← → | Table: insert row / column |
| ⌥⌘⌫, ⇧⌥⌘⌫ | Table: delete row, delete column |
| ⌥⇧↑ ↓ | Table: move row (to the top makes it the header) |

## Project layout

| Crate | What it does |
|---|---|
| `marginal-doc` | Parsing (pulldown-cmark), blocks and segments, transactions, every editing command, tables |
| `marginal-history` | The undo tree, with typing groups and amendments |
| `marginal-session` | The hot-exit journal, atomic saves, `session.json` |
| `marginal-theme` | Theme CSS (lightningcss) cascaded into styles; built-in themes and fonts |
| `marginal-highlight` | tree-sitter highlighting for fenced code |
| `marginal-app` | The macOS app: winit, vello, parley; layout, painting, input, handles |

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option. Bundled fonts carry their own licenses (SIL Open Font License) next to the font
files in `crates/marginal-theme/fonts/`.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in
the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without
any additional terms or conditions.
