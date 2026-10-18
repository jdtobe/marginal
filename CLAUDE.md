# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

Marginal is a lossless WYSIWYG Markdown editor for macOS, in Rust (winit + vello + parley).
`README.md` has the overview and keys, `CONTRIBUTING.md` the workflow, and `docs/design.md`
every product decision; update `docs/design.md` when a decision changes.

## Commands

```sh
cargo run -p marginal-app -- fixtures/sample.md     # open a file (no path: restore last session)
cargo test --workspace                              # everything
cargo test -p marginal-doc table::tests::enter      # one test (module path filter)
cargo test -p marginal-app editor::tests            # one module
cargo fmt                                           # rustfmt.toml: max_width 120
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p marginal-doc --example dump -- file.md # print parsed blocks/segments/table cells
```

- `MARGINAL_TRACE=1` prints any main-thread work over 8 ms (events, menu commands, frames,
  layout, persistence); `MARGINAL_TRACE=all` prints every span. Disk flushes run on the
  `marginal-session::Persister` thread, so they must never show up here.
- `MARGINAL_DATA_DIR=/tmp/x` keeps a dev session (journal, session.json, Themes/) out of
  `~/Library/Application Support/Marginal`.
- Visual checks render offscreen to PNG, never touching the real session:
  `cargo run -p marginal-app -- --snapshot out.png [--light] [--theme Studio] [--xray]
  [--caret N] [--pointer x,y] [--picker Q] [--grip-menu N] [--goto Q] [--find Q --replace R
  --regex] file.md`. Output is 2× (2000×1800 px for the 1000×900 window).
- There is no active CI. `.github/examples/ci.yml` is an inert example; nothing may run on paid
  infrastructure, so never move it into `.github/workflows/`.

## Architecture

**Source text is the single truth.** `marginal_doc::Document` holds the exact bytes and re-derives
`Block`s on every change (`parse.rs`, via pulldown-cmark offsets). A block's `segments` tile its
range: `Text`, `Replaced` (entities, escapes), `Break`, and `Syntax { owner }` (markup, hidden
unless revealed). Tables additionally carry `Block.table` (cell ranges per row) and are split into
per-cell pseudo-blocks with `Block::sub_block`.

**Every edit is a command that returns a `Change`** (`commands.rs`, `table.rs`, `find.rs`): a
`Transaction` of sorted, non-overlapping minimal edits plus the new selection. Commands must
patch only the bytes that change, in the document's own style (bullet char, emphasis marker,
table padding); property tests in `marginal-doc` enforce the untouched-bytes invariant. Tables go
through a line model (`table.rs` `Grid`) that re-renders rows and emits per-line minimal diffs;
aligned tables are re-padded, compact ones never are.

**Undo and hot exit.** `marginal-session::Buffer` applies transactions, records them in the
`marginal-history` undo tree and appends them to a journal; the file on disk is written only by
`Buffer::save` / `save_as`. `Editor::run` (app) is the one place changes are applied; follow-up
fixes that belong to the same user action (tidying an unused empty line, re-padding an aligned
table) are applied with `EditKind::Amend` so they undo together. Typing groups by time
(`GROUP_MS`).

**Display vs source.** `marginal-app/src/view.rs` lays each block out as *display* text (hidden
syntax removed, revealed marks at the caret, IME preedit spliced in) and keeps a `MapPiece` map;
`BlockView::source_offset` / `display_index` convert. All caret logic works in source offsets.
Table cells are their own `BlockView`s in row-major order (`layout_table`), so caret motion, hit
testing (`DocView::hit_test`, `nearest_in_row`) and vertical motion (`vertical_neighbor`, which
walks parley lines rather than probing pixels) work across cells without special cases.

**App layering.** `editor.rs` owns one buffer's view state (selection, scroll, popups: language
picker, grip menu, Go to Heading; table handles from `handles.rs`) and paints in editor
coordinates given a `base` transform. `main.rs` owns the window, the find strip (`findbar.rs`),
zoom and themes; it maps window → editor coordinates (`to_editor`: minus the strip's height,
divided by zoom). Menu items (`menubar.rs`, muda) and keys both dispatch through
`App::command(Command)`; menu key equivalents consume their shortcuts before winit sees them.

**Themes** (`marginal-theme`) are a CSS subset: element/class selectors, `#write`/`body` as the
page, `:root` variables with `@media (prefers-color-scheme)` overrides, and `--marginal-*`
variables for layout (glyph side/style, measure, caret, bullets, list numbers). Code
highlighting colors come from `.hl-<capture>` classes produced by `marginal-highlight`
(tree-sitter). Built-ins and their bundled OFL fonts are registered in `marginal-theme/src/lib.rs`;
user themes load from the data dir's `Themes/` folder (`marginal-app/src/themes.rs`).

## Testing conventions

- Command tests write the source with the caret marked (`|`, or `‸` when the text has pipes)
  and assert the exact resulting text and caret (`run` helpers in `commands.rs`, `table.rs`).
- `editor.rs` tests use `harness(src)`, which runs real layout with real fonts, so motion,
  clicks, handles and popups are testable without a window.

## Working with the user

Look, keys and behavior are the user's decisions: ask before building them, showing options as
rendered mockups rather than ASCII.

Make regular commits. You should commit after every reasonable and independent capability added, after all tests have passed. Provode a detailed commit message.
