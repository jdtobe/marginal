# Images: design

Status: approved in brainstorming, 2026-10-18. Roadmap item: M2 ▸ Images.

## Goal

Images in Marginal render inline the way a reader expects (local, remote with consent, animated,
missing), stay editable without leaving the rendered view, and can be pasted or dropped without the
user thinking about files. Every edit stays lossless: commands patch only the bytes they must.

## Decisions (made with the user)

| Topic | Decision |
|---|---|
| Formats | PNG, JPEG, GIF, WebP (animated too), SVG; HEIC, TIFF and PDF (first page) through macOS ImageIO |
| Remote images | Not fetched until allowed. A per-document strip ("N remote images not loaded · Load · Always for this file") **and** a Load button on each placeholder. "Always" is remembered per file and listed in Settings |
| Pointer near an image | Alt text fades in as a caption under the image and the path as a chip on it (distance-scaled, like table grips) |
| Caret in an image, or click | A muted source line (`![alt](path)` or the `<img>` tag) appears above the still-rendered image and is edited as text |
| ⌥ / x-ray | Every image shows its source line |
| Inline images | Natural size on the baseline; the line grows to fit |
| Resize | A handle on the right edge; release writes `<img src alt width>` (first resize converts `![]()`); double-click resets |
| Default save location | `./assets/` next to the document; `./<doc>.assets/` and Same Folder are options |
| Location setting | Format ▸ Image Location submenu, remembered per folder |
| Dropping a file | Linked in place if under the document's folder, otherwise copied into the save location |
| Pasted image name | Slug from the alt text, else the nearest heading above, else the document name, plus a counter |
| Paste inline | ⌥⌘V (Edit ▸ Paste Image Inline) and a context-menu item write a `data:` URI. ⇧⌘V stays free for a future plain-text paste |
| Convert later | The path chip and context menu offer Embed in Document / Save to assets/ |
| Untitled documents | Images are staged in the data dir and relocated on the first save |
| Unused images | After a save, a strip asks: Move to Trash / Keep / ☐ Remember for this folder. Ignoring it deletes nothing |
| Settings | A ⌘, side panel (like the theme gallery) whose main section is a Folders table of overrides with add, edit and remove |
| Folder inheritance | The nearest ancestor folder with an override wins; the panel shows where a value comes from |
| Layout approach | parley inline boxes for every image (no separate image view) |
| Decoders | Rust crates first (`image`, `resvg`); ImageIO only for HEIC/TIFF/PDF, behind `cfg(target_os = "macos")` |

## Phases

Each phase ships and is committed on its own.

1. **Display:** parsing, decoding, layout, reveal states, missing and remote states, animation,
   resize, theme `img` rules.
2. **Insert:** paste, drop, naming, Paste Image Inline, Embed ↔ Save, staging, unused-image strip,
   Format ▸ Image Location.
3. **Settings:** the ⌘, panel. Its look is decided with the user from rendered mockups at the
   start of the phase; this spec fixes only contents and behavior.

## Units

| Unit | Owns | Depends on |
|---|---|---|
| `marginal-doc` `parse.rs` | `InlineKind::Image { dest, alt, title, width, form }` for both `![alt](dest "title")` and inline/block HTML `<img src alt title width>`; byte ranges for alt, dest and width; `form` is `Markdown` or `Html` | — |
| `marginal-doc` `image.rs` (new) | Commands returning `Change`: `insert_image`, `set_image_dest`, `set_image_width`, `reset_image_size` | `parse.rs` |
| `marginal-assets` (new crate) | Save-location resolution, naming, copy-or-link, staging and relocation, the created-files record, orphan detection, moving to the Trash. No UI | `marginal-doc` |
| `marginal-session` `settings.rs` (new) | `settings.json` in the data dir: global defaults, per-folder rows, per-file remote consent; nearest-ancestor lookup; writes through `Persister` | — |
| `marginal-app` `images/` (new) | `decode.rs`, `imageio.rs` (macOS), `cache.rs` (decode thread, downscaling, LRU budget, animation clock), `remote.rs` (`Fetch` trait, ureq implementation, disk cache), `paste.rs` (clipboard classifier, `NSPasteboard` reader) | `marginal-session` |
| `marginal-app` `view.rs` | Inline boxes and atomic `MapPiece`s for images; splicing the source line at the caret | `images` (sizes only) |
| `marginal-app` `imagectl.rs` (new) + `editor.rs` | Hover caption and path chip, resize handle, image context menu; the grip menu becomes a shared painted menu | `view`, `image.rs` |
| `marginal-app` `notice.rs` (new) + `main.rs` | One generic window strip below the find strip, used for remote consent, unused images and errors | — |
| `marginal-app` `settings/panel.rs` (new) | The ⌘, panel (Phase 3) | `settings.rs` |
| `marginal-theme` | `img` rules: `border-radius`, `margin`, `max-width` (%), `border` | — |

Menus: Edit ▸ Paste Image Inline (⌥⌘V); Format ▸ Image Location ▸ assets/ · <doc>.assets/ · Same
Folder (radio, per folder); View ▸ Load Remote Images; Marginal ▸ Settings… (⌘,).

Dependencies added: `image` (png, jpeg, gif, webp features), `resvg`, `trash` (or
`NSFileManager trashItem` directly), `arboard` with `image-data`, objc2 bindings for ImageIO and
`NSPasteboard` (macOS only).

## Display (Phase 1)

### Parsing

- `![alt](dest "title")`, `![alt](<path with spaces>)` and reference images (`![alt][ref]`) all
  become `Image`. The dest range of a reference image is the definition's dest, so
  `set_image_dest` patches the definition.
- HTML `<img>` in inline HTML or an HTML block is recognized when it is a single tag with
  attributes in any order, any quoting, with or without `/>`. Anything else in the HTML (other
  tags, `style`, `height`) is left alone and kept byte for byte.
- A `data:` dest is one atomic `Replaced` segment that displays as `…(1.2 MB)` in the source line.
  The caret steps over it in one move and never lands inside it.

### Resolution

`dest` is percent-decoded. A relative path resolves against the document's folder, or for a
staged Untitled document against its staging folder. `http(s)` waits for consent; `data:` decodes
in place. The cache key is the resolved path plus mtime (or the URL), and mtimes are rechecked when
the window regains focus.

### Sizing

- Natural size is in points, using the image's DPI (PNG `pHYs`, JPEG/TIFF resolution, ImageIO
  properties). A 144-DPI Retina screenshot shows at half its pixel size.
- `<img width>` is points; height keeps the aspect ratio. Every image is capped at the measure
  (and by a theme's `img { max-width }`).
- Before the header is decoded, the box uses the size remembered from the last session (stored
  with session data, keyed by resolved path), otherwise 16:9 at the measure. When the real size
  arrives the layout reflows with scroll anchored to the top visible block.

### Layout

Each image is a parley inline box in its block's display text, mapped by one atomic `MapPiece` to
the whole image source range. An image alone in a paragraph is a line holding one box. Caret
motion, `hit_test`, `nearest_in_row`, `vertical_neighbor`, selection and x-ray treat the box like
one character. Inline images sit on the baseline and grow their line.

### Reveal states

1. **Pointer near:** the caption (alt text, italic, muted, centered under a standalone image) and
   the path chip (top-left of the image) fade in by distance. Clicking the caption puts the caret
   in the alt text (state 2). Clicking the chip opens Replace Image… (a file picker; only `dest`
   changes).
2. **Caret inside or click:** the source line is spliced into display text above the box, muted,
   and edited as ordinary text. Arrowing out of the image hides it again.
3. **x-ray:** every image shows its source line.

### Decoding and cache

- One background decode thread. Large images are downscaled to at most 2× their display size in
  device pixels; a memory budget (default 256 MB) evicts least-recently-drawn images first.
- `image` decodes PNG, JPEG, GIF and WebP including animation frames; `resvg` rasterizes SVG at the
  display size (re-rasterized on zoom); ImageIO decodes HEIC, TIFF and the first page of a PDF.
- Animation frames advance only while the image is visible and the window is focused. With macOS
  Reduce Motion on, an animation shows its first frame and plays only while hovered.

### Missing image

A dashed box at the remembered or default size showing the alt text, "<path> not found" and a
Locate… button (a file picker that rewrites `dest`, relative when possible).

### Remote images

- Before consent each remote image is a placeholder showing its alt text and host, with a Load
  button.
- The notice strip shows "N remote images not loaded · Load · Always for this file". "Always" is
  stored per file in `settings.json`. View ▸ Load Remote Images does the same as Load.
- Fetches run off-thread with a 10 s timeout and are cached on disk in the data dir by URL with
  HTTP validators. A failure shows "couldn't load (<status or reason>)" in the placeholder.

### Resize

The handle sits on the right edge and fades like the table grips (pointer proximity, caret inside,
or ⌥). Dragging previews without editing and shows "340 px · 57%"; ⇧ snaps to 25/50/75/100% of the
measure. Release applies one `set_image_width` transaction; the first one converts `![]()` to
`<img src alt title width>`. Double-click resets: an image Marginal converted (an `<img>` with
only `src`, `alt`, `title`, `width`) goes back to `![]()`, and any other `<img>` just loses
`width`.

### Context menu

Right-click on an image: Replace Image…, Reveal in Finder, Copy Image, Copy Path, Embed in Document
or Save to assets/, Reset Size. Right-click on text: Cut, Copy, Paste, Paste Image Inline. Both use
the shared painted menu.

## Insert (Phase 2)

### Clipboard classification for ⌘V

1. File URLs (Finder copies) → handled as a drop. Finder also puts the file's icon on the
   clipboard as image data; it must be ignored.
2. Image data → an image, even when a URL or HTML is also present (Safari's Copy Image).
3. Otherwise text, as today.

Original encoded bytes (PNG, JPEG, HEIC, …) are written as they are; only bare bitmaps are encoded
as PNG.

### Placement and naming

- On an empty line the image becomes its own paragraph; inside text it is inline. Marginal writes
  `![slug](<location>/slug-1.png)` and selects the alt text.
- Slug: from the alt text, else the nearest heading above, else the document's file stem;
  lowercase ASCII kebab-case, at most 48 characters, with `-1`, `-2`, … to avoid collisions in the
  target folder. Dropped files keep their own name, with a counter if taken.

### Drop

- A file under the document's folder is linked with a relative path; any other file is copied into
  the save location.
- Several files insert one image per line. Non-image files are ignored in this milestone.
- The image goes at the drop point when the pointer position is known during the drag, otherwise
  at the caret. Whether winit on macOS reports it is verified first in Phase 2.

### Paste Image Inline and conversion

⌥⌘V and the context menu write a `data:<mime>;base64,` URI with the original bytes. Embed in
Document and Save to assets/ convert in either direction, each as one undoable change.

### Untitled documents

Files go to `<data dir>/staging/<buffer id>/assets/…` and links are written as plain
`assets/<name>`, resolving against the staging folder. On the first save, staged files move to the
real save location. When that location is `assets/`, no text changes. When it is `<doc>.assets/` or
the same folder, the links are rewritten in an `EditKind::Amend` change applied just before the
bytes are written. Staging survives hot exit and is cleaned up when its buffer is closed without
being saved.

### Unused images

- Every file Marginal creates (paste, drop copy, Save to assets/, relocation) is recorded with its
  buffer in the journal, so the record survives restarts.
- After each save completes, recorded files that no open buffer references are reported in the
  notice strip: "N unused images · Move to Trash · Keep · ☐ Remember for this folder". The save is
  never blocked; ignoring the strip deletes nothing. The remembered answer (Ask, Trash, Keep) is a
  per-folder setting.
- Trash is the macOS Trash. A link to a trashed file shows the Missing state.

### Errors

Write failures (read-only folder, full disk) show in the notice strip with an "Embed Inline
Instead" action. A link is inserted only after its file is written.

### Image Location menu

Format ▸ Image Location sets the current document's folder row (assets/, <doc>.assets/, Same
Folder) with a check on the effective value. Before Phase 3 this is the only way to change it.

## Settings (Phase 3)

`settings.json` in the data dir holds global defaults, per-folder rows and per-file remote consent.
Lookup walks up from the document's folder; the nearest row with a value wins.

The ⌘, panel opens on the right like the theme gallery, is keyboard navigable, and closes with Esc:

- **General:** default image location; default for unused images (Ask, Trash, Keep).
- **Folders:** a table of folders with overrides, shortened to `~/…`, with columns for image
  location and unused images. Inherited values are dimmed with "from <folder>". A folder that no
  longer exists is dimmed with Locate… and Remove. "+" adds a folder through a picker; Return or a
  double-click edits a row; ⌫ removes it.
- **Remote images:** files allowed to load remote images, each removable.

## Testing

- `marginal-doc`: command tests with caret notation for `insert_image`, `set_image_dest`,
  `set_image_width` (the `![]()` ↔ `<img>` round trip and both reset cases) and Embed/Save;
  property tests extended to the image commands (untouched bytes); parse tests for `<img>` variants,
  `<path>`, percent-encoding, titles, reference images and data URIs.
- `marginal-assets`: temp-dir tests for naming and collisions, copy-or-link, staging relocation
  under each location style, orphan detection, and a read-only folder.
- `settings.rs`: nearest-ancestor lookup, missing folders, a round trip through `Persister`.
- `images/`: decode fixtures (a 144-DPI PNG's point size, animated GIF frame count and delays, SVG,
  HEIC/TIFF/PDF gated on macOS), cache eviction under a small budget, remote loading through a fake
  `Fetch`.
- `editor.rs` harness: caret motion across inline and standalone images, `hit_test` on a box,
  `vertical_neighbor` past a tall image, the three reveal states, the resize transaction on
  release, and the clipboard classifier with a fake pasteboard (Finder file plus icon → the file).
- Visual: `fixtures/images.md` with `fixtures/assets/`, and new snapshot flags `--hover-image N`
  and `--remote-blocked` to render each state offscreen.

## Docs to update

`docs/design.md` (Images row, plus rows for Settings, Remote images and Resize), `README.md`
(keys: ⌥⌘V, ⌘,), `CHANGELOG.md`.

## Out of scope

Non-image file drops (links to arbitrary files), image editing (crop, rotate), captions written to
the file, `<picture>`/`srcset`, a folder-local settings file (possible opt-in in M3), and a "Clean
Up Unused Images" command for files Marginal didn't create.
