# Design notes

Marginal is a WYSIWYG Markdown editor whose controls are extensions of the rendered elements,
not toolbars. These are the product decisions made so far; change them here when they change.

## Principles

- **The file is sacred.** Source text is the single truth. Commands patch only the bytes they
  must, in the document's existing style. The file is written only on ⌘S.
- **Calm by default.** Syntax lives in the margin; controls appear where you're working (caret
  inside, pointer near, or ⌥ held) and fade otherwise.
- **The reader picks the look.** Themes are CSS and never change the file.

## Decisions

| Area | Decision |
|---|---|
| Render stack | Own every pixel: winit + vello (GPU 2D) + parley (text, IME) + accesskit |
| Fidelity | Lossless, byte-exact. Edits patch only touched bytes |
| Platform | macOS first; the architecture stays portable |
| Syntax display | Block syntax as glyphs in the margin; inline marks (`**`) revealed only at the caret |
| Affordances | Appear with the caret inside an element, with pointer proximity (distance-scaled fade), or everywhere while ⌥ is held (x-ray) |
| Dialect | CommonMark + GFM (tables, tasks, strikethrough, autolinks, footnotes), math, Mermaid |
| Diagrams | mermaid.js off-thread to SVG, drawn by vello, cached by content hash (planned) |
| Scope | A single file or a folder as a workspace |
| Saving | Sublime-style hot exit: buffers, windows and undo history journaled continuously and restored after quit or crash; the file is written only on ⌘S |
| Undo | A full undo tree, persisted across restarts |
| External edits | Live 3-way merge against the last loaded or saved disk content (planned) |
| Images | Paste/drop location configurable per workspace (planned) |
| Windows | Tabs and a collapsible sidebar (planned) |
| Commands | Command palette on ⇧⌘P; Markdown autoformats as you type; no slash menu, no toolbar |
| Menus | Native macOS menu bar (File, Edit, Format, View, Window). Every item is a command the keys share. Extras: Open Recent, Reveal in Finder, Copy Path, Go to Heading (⇧⌘O), Show Markdown Source (⌥⌘X, latched x-ray) |
| Find & replace | A strip across the top of the window; plain text by default, regex optional (`.*`), case and whole-word toggles, `$1` groups in replacements |
| Themes | A CSS subset, Typora-style where possible, which can also set layout (glyph side and style). Built-ins: **Terminal** (default), Manuscript, Studio, Editorial. User themes from `~/Library/Application Support/Marginal/Themes/`, reloaded live |
| Appearance | Follows the system by default; View ▸ Appearance can force light or dark. Text zoom with ⌘= / ⌘− and ⇧⌘0 for actual size (⌘0 is Paragraph) |
| Spellcheck | A trait with NSSpellChecker and spellbook (Hunspell) backends; the user picks (planned) |
| Export | Themed HTML and PDF (planned) |
| Keys | Mac-native text keys plus structural keys (⌥⇧↑↓ move, ⌘[ ⌘] heading level) |
| Novel track | Structural editing: structure-aware selection, dragging sections by their glyph, folding |
| List markers | Drawn inline (▸, `1.`, `[x]`); raw markers show in the margin only under x-ray |
| Numbered lists | Muted `1.` in the glyph color, right-aligned; themes can set `--marginal-ol-style` to `decimal`, `circled` or `padded` |
| Caret | The theme decides shape and blink; Terminal uses a blinking block that stops after 30 s idle |
| Fonts | Built-in themes bundle their OFL fonts |
| Backspace at block start | Demotes first (strips `##`, a list marker or `>`); a second press joins with the previous block |
| ⇧Return | A hard line break, written as a trailing `\` |
| Blank lines | Empty lines Marginal inserted and you never filled are tidied away, in the same undo step |
| ⌘K | Toggles a link; pasting a URL over a selection links it |
| Code language | Clicking the chip (or ⌥⌘L) opens a searchable list with recent languages first; only the first word of the info string changes |
| Code highlighting | tree-sitter grammars bundled; colors from theme CSS classes `.hl-*` |
| Tables | Always a grid (x-ray shows the source). Aligned tables are re-padded as you type; compact ones stay compact |
| Table keys | Tab / ⇧Tab walk cells (Tab in the last cell adds a row); Return goes to the cell below (adding a row at the end); ⇧Return writes `<br>`; ⌥⇧↑↓ move the row |
| Table grips | Click selects a row or column (⌫ deletes it), drag reorders (a body row dragged to the top becomes the header), ⊕ on a border inserts there; right-click a column grip for alignment, insert and delete |
| Theme gallery | View ▸ Theme ▸ Browse Themes… (⇧⌘T) opens a panel on the right: search, Light/Dark/Installed/Good-fit filters, rows with thumbnail, author, tags and a fit badge. Moving through the list restyles the open document; Return installs and keeps the theme, Esc restores the previous one. Source: the Typora theme gallery, fetched only when the panel opens and cached |
| Typora compatibility | Grow the theme engine toward real document CSS in four phases (load more themes; selectors with context; box model; decorations and fonts), re-surveying the whole gallery after each. Acceptance set: GitHub, Night, Vue, Drake, OneDark, Purple, Lapis, Pie, Gothic, Newsprint |
| Theme decorations | `::before`/`::after` decorations render (bars, quote marks, ornaments), but ones that only restate Markdown syntax (`#`, `H2`, `>`) are dropped, since the margin already shows syntax |
| License | MIT OR Apache-2.0 |
| Paid infrastructure | Nothing runs on billed services. The CI workflow is an inert example in `.github/examples/` |

## Milestones

1. **M0 – Skeleton** (done): window, vello and parley drawing, Terminal theme.
2. **M1 – Core editing loop** (done): lossless editing, margin glyphs, revealed inline marks,
   autoformat, undo tree, hot-exit journal, IME.
3. **M2 – Structure** (in progress). Done: tables, code highlighting and language picker, find &
   replace, native menus, four themes and a user themes folder. In progress: menu
   responsiveness, an in-app Typora theme gallery with live preview and fit scores. Next:
   images (configurable save location), footnotes and sidenotes, structural selection
   (⌥↑/↓), moving blocks and sections (⌥⇧↑/↓, dragging a heading glyph), folding, proximity
   fades for every block control.
4. **M3 – Workspace**: folder workspaces with a sidebar, relative link navigation, tabs and
   multiple windows restored by hot exit, the ⇧⌘P command palette over commands, files and
   headings, live 3-way merge of external edits, journal compaction.
5. **M4 – Rich blocks**: math, Mermaid.
6. **M5 – Polish**: spellcheck (both backends), HTML and PDF export, theme engine gaps
   (`@font-face`, small-caps, background images) and in-app theme diagnostics.

Cross-cutting work not yet started: VoiceOver support via accesskit, a signed `.app` bundle with
Finder open-file handling, hand-testing IME, and verification debt (fuzzing, CommonMark-spec and
real-README round trips, per-theme snapshot comparisons).

## Open questions

Non-UTF-8 files · CRLF files · narrow windows and margin width · drag semantics for a heading
glyph · conflict wording for merges · default image location · theme diagnostics UX · palette
ranking · CJK and emoji font fallback · very large files · app icon and bundle id.
