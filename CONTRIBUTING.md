# Contributing to Marginal

## The one rule

**Never touch bytes the user didn't ask to change.** Every editing command in `marginal-doc`
compiles to a `Transaction` of minimal edits on the source text, in the document's own style
(its bullet character, its emphasis marker, its table padding). If a feature seems to need a
rewrite of the whole block or file, it needs a different design. The property tests in
`marginal-doc` check this invariant over random documents and command sequences.

The file on disk is written only by an explicit save. Everything else goes to the journal in
`marginal-session`.

## Development loop

```sh
cargo check --workspace                       # fast feedback (rust-analyzer does this live)
cargo run -p marginal-app -- fixtures/sample.md
cargo test --workspace
cargo fmt
cargo clippy --workspace --all-targets -- -D warnings
```

Run the last three before opening a pull request. There is no active CI: an example GitHub
Actions workflow lives in `.github/examples/ci.yml` and is deliberately outside
`.github/workflows/`, so it never runs (macOS runners are billed).

Set `MARGINAL_TRACE=1` to print any main-thread work slower than 8 ms (`all` prints every
span). Set `MARGINAL_DATA_DIR=/some/tmp/dir` to keep a development session separate from your real
one in `~/Library/Application Support/Marginal`.

## Tests

- **Commands** (`marginal-doc`): write the source with the caret marked (`|`, or `‸` when the
  text contains pipes) and assert the exact resulting text and caret. See `commands.rs` and
  `table.rs` for the helpers.
- **Editor behavior** (`marginal-app/src/editor.rs`): the `harness` runs real layout with real
  fonts, so caret movement, clicks and handles can be tested without a window.
- **Visual changes**: render before and after with `--snapshot out.png` and compare. Add
  `--light`, `--xray`, `--caret N`, `--pointer x,y`, `--picker QUERY` or `--grip-menu N` to show
  a particular state.

## Style

- `rustfmt.toml` sets the format; don't fight it.
- Comments explain *why*, briefly. Doc comments on public items say what they do in a sentence.
- User-facing text names things the way a writer would (a *heading*, a *cell*), not how they're
  stored (a *block*, a *segment*).

## Design decisions

Product decisions live in [docs/design.md](docs/design.md). If a change alters one, update it
there in the same pull request.
