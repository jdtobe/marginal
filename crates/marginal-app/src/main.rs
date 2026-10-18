mod editor;
mod findbar;
mod goto;
mod handles;
mod menu;
mod menubar;
mod paint;
mod picker;
mod snapshot;
mod themes;
mod view;

use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use marginal_doc::{InlineFormat, Selection};
use marginal_session::{BufferMeta, SessionFile, Store};
use marginal_theme::{Appearance, Theme};
use vello::kurbo::{Affine, Rect};
use vello::util::{RenderContext, RenderSurface};
use vello::wgpu;
use vello::{AaConfig, RenderParams, Renderer, RendererOptions, Scene};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalPosition, LogicalSize};
use winit::event::{ElementState, Ime, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};
use winit::window::{Theme as WinitTheme, Window, WindowId};

use editor::{Direction, Editor, Unit};
use findbar::{Action, FindBar};
use menubar::{Command, MenuBar};
use paint::color;
use picker::PickerInput;
use themes::Themes;
use view::Fonts;

const SAMPLE: &str = include_str!("../../../fixtures/sample.md");
/// Pause after the last edit before the journal is flushed to stable storage.
const SYNC_AFTER: Duration = Duration::from_millis(300);
/// Minimum interval between session.json writes.
const SESSION_EVERY: Duration = Duration::from_secs(1);
const MULTI_CLICK: Duration = Duration::from_millis(450);
/// Text zoom steps for ⌘= / ⌘−.
const ZOOM_STEPS: &[f32] = &[0.67, 0.75, 0.85, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5];

#[derive(Debug)]
enum UserEvent {
    Menu(muda::MenuEvent),
}

struct RenderState {
    surface: RenderSurface<'static>,
    window: Arc<Window>,
}

struct App {
    context: RenderContext,
    renderers: Vec<Option<Renderer>>,
    state: Option<RenderState>,
    scene: Scene,
    fonts: Fonts,
    editor: Editor,
    store: Store,
    session: SessionFile,
    session_written: Instant,
    modifiers: ModifiersState,
    /// The pointer in window coordinates.
    cursor: (f32, f32),
    mouse_down: bool,
    last_click: Option<(Instant, (f32, f32), u8)>,
    drawn_blink: bool,
    title: String,
    menubar: Option<MenuBar>,
    themes: Themes,
    findbar: Option<FindBar>,
    /// The window's size in logical pixels.
    window_size: (f32, f32),
    /// The appearance the system (or the window) reports, used when not overridden.
    system_appearance: Appearance,
}

pub(crate) fn renderer_options() -> RendererOptions {
    RendererOptions {
        use_cpu: false,
        antialiasing_support: vello::AaSupport::area_only(),
        num_init_threads: NonZeroUsize::new(1),
        pipeline_cache: None,
    }
}

impl App {
    fn window(&self) -> Option<Arc<Window>> {
        self.state.as_ref().map(|s| s.window.clone())
    }

    fn redraw(&self) {
        if let Some(w) = self.window() {
            w.request_redraw();
        }
    }

    // ---- geometry: the find strip pushes the page down, zoom scales it -----------------------

    fn zoom(&self) -> f32 {
        self.session.settings.zoom.unwrap_or(1.0)
    }

    fn inset(&self) -> f32 {
        self.findbar.as_ref().map_or(0.0, |b| b.height(self.window_size.0))
    }

    /// Window coordinates to the editor's (zoomed, below the strip).
    fn to_editor(&self, (x, y): (f32, f32)) -> (f32, f32) {
        (x / self.zoom(), (y - self.inset()) / self.zoom())
    }

    fn layout_editor(&mut self) {
        let (w, h) = self.window_size;
        self.editor.width = w / self.zoom();
        self.editor.height = ((h - self.inset()) / self.zoom()).max(1.0);
        self.editor.invalidate();
    }

    // ---- session -------------------------------------------------------------------------------

    fn persist_session(&mut self) {
        let meta = self.editor.buffer.meta(self.editor.selection, self.editor.scroll);
        match self.session.buffers.iter_mut().find(|m| m.id == meta.id) {
            Some(existing) => *existing = meta.clone(),
            None => self.session.buffers.push(meta.clone()),
        }
        self.session.active = Some(meta.id);
        self.session.recent_languages = self.editor.recent_languages.clone();
        if let Some(bar) = &self.findbar {
            self.session.settings.find = bar.options;
        }
        if let Err(e) = self.store.save_session(&self.session) {
            eprintln!("could not save session: {e}");
        }
        self.editor.session_dirty = false;
        self.session_written = Instant::now();
    }

    fn sync_menus(&mut self) {
        if let Some(bar) = &self.menubar {
            let s = &self.session.settings;
            bar.sync(self.themes.selected(), s.appearance.as_deref(), s.xray, self.editor.buffer.path.is_some());
        }
    }

    fn remember_file(&mut self, path: &Path) {
        self.session.settings.remember_file(path);
        if let Some(bar) = &mut self.menubar {
            bar.set_recent(&self.session.settings.recent_files);
        }
    }

    // ---- documents ---------------------------------------------------------------------------

    /// Shows another buffer in the window, keeping the view settings.
    fn switch_to(&mut self, buffer: marginal_session::Buffer, meta: BufferMeta) {
        self.persist_session();
        let mut editor = Editor::new(buffer, meta.selection, self.editor.theme.clone(), self.editor.appearance);
        editor.scroll = meta.scroll;
        editor.recent_languages = self.editor.recent_languages.clone();
        editor.xray = self.editor.xray;
        self.editor = editor;
        if let Some(bar) = &mut self.findbar {
            bar.invalidate();
        }
        self.layout_editor();
        self.persist_session();
        self.sync_menus();
        self.redraw();
    }

    fn open_path(&mut self, path: &Path) {
        if !path.exists() {
            eprintln!("{} no longer exists", path.display());
            self.session.settings.recent_files.retain(|p| p != path);
            if let Some(bar) = &mut self.menubar {
                bar.set_recent(&self.session.settings.recent_files);
            }
            return;
        }
        self.persist_session();
        match open(&self.store, &self.session, Some(path)) {
            Ok((buffer, meta)) => {
                let canonical = buffer.path.clone().unwrap_or_else(|| path.to_path_buf());
                self.switch_to(buffer, meta);
                self.remember_file(&canonical);
            }
            Err(e) => eprintln!("could not open {}: {e}", path.display()),
        }
    }

    fn save(&mut self, as_new: bool) {
        let result = if self.editor.buffer.path.is_some() && !as_new {
            self.editor.buffer.save()
        } else {
            let name = self.editor.title().trim_end_matches(" — Edited").to_owned();
            let name = if name.ends_with(".md") || name.ends_with(".markdown") { name } else { format!("{name}.md") };
            match rfd::FileDialog::new().add_filter("Markdown", &["md", "markdown"]).set_file_name(&name).save_file() {
                Some(path) => self.editor.buffer.save_as(&path),
                None => return,
            }
        };
        match result {
            Ok(()) => {
                if let Some(path) = self.editor.buffer.path.clone() {
                    self.remember_file(&path);
                }
                self.persist_session();
            }
            Err(e) => eprintln!("save failed: {e}"),
        }
        self.editor.invalidate();
        self.sync_menus();
    }

    // ---- view settings -----------------------------------------------------------------------

    fn set_theme(&mut self, name: &str) {
        match self.themes.load(name) {
            Ok(theme) => {
                self.editor.theme = theme;
                self.editor.invalidate();
                self.session.settings.theme = Some(name.to_owned());
                self.editor.session_dirty = true;
            }
            Err(e) => eprintln!("theme: {e}"),
        }
        self.sync_menus();
    }

    /// Picks up new or edited user themes (on focus and from View ▸ Theme ▸ Reload).
    fn refresh_themes(&mut self) {
        if self.themes.rescan()
            && let Some(bar) = &mut self.menubar
        {
            bar.set_themes(&self.themes.entries, self.themes.selected());
        }
        if let Some(result) = self.themes.reload_if_changed() {
            match result {
                Ok(theme) => {
                    self.editor.theme = theme;
                    self.editor.invalidate();
                }
                Err(e) => eprintln!("theme: {e}"),
            }
        }
        self.sync_menus();
    }

    fn apply_appearance(&mut self) {
        let chosen = self.session.settings.appearance.as_deref();
        self.editor.appearance = match chosen {
            Some("light") => Appearance::Light,
            Some("dark") => Appearance::Dark,
            _ => self.system_appearance,
        };
        if let Some(w) = self.window() {
            w.set_theme(match chosen {
                Some("light") => Some(WinitTheme::Light),
                Some("dark") => Some(WinitTheme::Dark),
                _ => None,
            });
        }
        self.editor.invalidate();
    }

    fn set_zoom(&mut self, zoom: f32) {
        self.session.settings.zoom = (zoom != 1.0).then_some(zoom);
        self.editor.session_dirty = true;
        self.layout_editor();
        self.editor.reveal_caret(&mut self.fonts);
    }

    fn step_zoom(&mut self, up: bool) {
        let z = self.zoom();
        let next = if up {
            ZOOM_STEPS.iter().copied().find(|s| *s > z + 0.01).unwrap_or(z)
        } else {
            ZOOM_STEPS.iter().rev().copied().find(|s| *s < z - 0.01).unwrap_or(z)
        };
        self.set_zoom(next);
    }

    fn update_xray(&mut self) {
        let held = self.modifiers.alt_key() && !self.modifiers.super_key();
        let xray = self.session.settings.xray || held;
        if xray != self.editor.xray {
            self.editor.xray = xray;
            self.editor.invalidate();
        }
    }

    // ---- find & replace ------------------------------------------------------------------------

    fn open_find(&mut self, replace: bool) {
        let selected = self.editor.selected_text().filter(|t| !t.contains('\n') && t.len() < 200);
        let bar = self.findbar.get_or_insert_with(|| FindBar::new(self.session.settings.find));
        bar.show_replace |= replace;
        bar.focus = Some(findbar::Focus::Find);
        if let Some(text) = selected {
            bar.find.set(&text);
        }
        bar.find.all = true;
        bar.invalidate();
        self.layout_editor();
        self.find_action(Action::Search);
    }

    fn close_find(&mut self) {
        if let Some(bar) = self.findbar.take() {
            self.session.settings.find = bar.options;
            self.editor.session_dirty = true;
        }
        self.editor.highlights.clear();
        self.layout_editor();
    }

    /// Searches again if the document or query changed, and hands the matches to the editor.
    fn refresh_find(&mut self) {
        let Some(bar) = &mut self.findbar else { return };
        let doc = &self.editor.buffer;
        bar.refresh(doc.doc.text(), doc.history.version(), self.editor.selection.range().start);
        self.editor.highlights = bar.matches.clone();
        self.editor.highlight_current = bar.current;
    }

    fn select_match(&mut self, range: std::ops::Range<usize>) {
        self.editor.set_selection(Selection::new(range.start, range.end));
        self.editor.reveal_caret(&mut self.fonts);
    }

    fn find_action(&mut self, action: Action) {
        let sel = self.editor.selection.range();
        match action {
            Action::Search => {
                if let Some(bar) = &mut self.findbar {
                    bar.invalidate();
                }
                self.refresh_find();
                if let Some(m) = self.findbar.as_ref().and_then(FindBar::current_match) {
                    self.select_match(m);
                }
            }
            Action::Next | Action::Previous => {
                self.refresh_find();
                let forward = action == Action::Next;
                let from = if forward { sel.end } else { sel.start };
                if let Some(m) = self.findbar.as_mut().and_then(|b| b.step(from, forward)) {
                    self.select_match(m);
                }
            }
            Action::Replace => {
                self.refresh_find();
                let Some(bar) = &self.findbar else { return };
                let (Some(current), Some(finder)) = (bar.current_match(), bar.finder()) else { return };
                if sel != current {
                    // The first press shows which match will be replaced.
                    self.select_match(current);
                    return;
                }
                let change = finder.replace_one(&self.editor.buffer.doc, current, &bar.replace.text);
                self.editor.run(change);
                self.refresh_find();
                let caret = self.editor.selection.caret;
                if let Some(m) = self.findbar.as_mut().and_then(|b| b.step(caret, true)) {
                    self.select_match(m);
                }
            }
            Action::ReplaceAll => {
                self.refresh_find();
                let Some(bar) = &self.findbar else { return };
                let Some(finder) = bar.finder() else { return };
                if let Some(change) =
                    finder.replace_all(&self.editor.buffer.doc, self.editor.selection, &bar.replace.text)
                {
                    self.editor.run(change);
                    self.editor.reveal_caret(&mut self.fonts);
                }
                self.refresh_find();
            }
            Action::Close => self.close_find(),
        }
    }

    // ---- commands ----------------------------------------------------------------------------

    fn command(&mut self, command: Command, event_loop: &ActiveEventLoop) {
        // Clipboard and selection commands go to the find field while it has the keyboard.
        if let Some(field) = self.findbar.as_mut().and_then(FindBar::focused_field) {
            let searched = |app: &mut Self| {
                if app.findbar.as_ref().is_some_and(|b| b.focus == Some(findbar::Focus::Find)) {
                    app.find_action(Action::Search);
                }
            };
            match command {
                Command::Copy | Command::Cut => {
                    set_clipboard(&field.text);
                    if command == Command::Cut {
                        field.set("");
                        searched(self);
                    }
                    return self.redraw();
                }
                Command::Paste => {
                    if let Some(text) = get_clipboard() {
                        field.insert(&text);
                        searched(self);
                    }
                    return self.redraw();
                }
                Command::SelectAll => {
                    field.all = true;
                    return self.redraw();
                }
                Command::Undo | Command::Redo => return,
                _ => {}
            }
        }
        let fonts = &mut self.fonts;
        let ed = &mut self.editor;
        let mut reveal = true;
        match command {
            Command::New => {
                match self.store.create_buffer(None, "") {
                    Ok(buffer) => {
                        let meta = buffer.meta(Selection::default(), 0.0);
                        self.switch_to(buffer, meta);
                    }
                    Err(e) => eprintln!("could not create a buffer: {e}"),
                }
                return;
            }
            Command::Open => {
                if let Some(path) =
                    rfd::FileDialog::new().add_filter("Markdown", &["md", "markdown", "txt"]).pick_file()
                {
                    self.open_path(&path);
                }
                return;
            }
            Command::OpenRecent(path) => return self.open_path(&path),
            Command::ClearRecent => {
                self.session.settings.recent_files.clear();
                if let Some(bar) = &mut self.menubar {
                    bar.set_recent(&[]);
                }
                self.editor.session_dirty = true;
                return;
            }
            Command::Save => return self.save(false),
            Command::SaveAs => return self.save(true),
            Command::RevealInFinder => {
                if let Some(path) = &ed.buffer.path
                    && let Err(e) = std::process::Command::new("open").arg("-R").arg(path).spawn()
                {
                    eprintln!("could not reveal {}: {e}", path.display());
                }
                return;
            }
            Command::CopyPath => {
                if let Some(path) = &ed.buffer.path {
                    set_clipboard(&path.display().to_string());
                }
                return;
            }
            Command::Close | Command::Quit => {
                // Hot exit: nothing to ask, everything is already journaled.
                self.persist_session();
                event_loop.exit();
                return;
            }
            Command::Undo => ed.undo(),
            Command::Redo => ed.redo(),
            Command::Cut => {
                if let Some(text) = ed.cut() {
                    set_clipboard(&text);
                }
            }
            Command::Copy => {
                if let Some(text) = ed.selected_text() {
                    set_clipboard(&text);
                }
                reveal = false;
            }
            Command::Paste => {
                if let Some(text) = get_clipboard() {
                    ed.paste(&text);
                }
            }
            Command::SelectAll => {
                ed.select_all();
                reveal = false;
            }
            Command::Find => return self.open_find(false),
            Command::FindReplace => return self.open_find(true),
            Command::FindNext | Command::FindPrevious => {
                if self.findbar.is_none() {
                    return self.open_find(false);
                }
                let action = if command == Command::FindNext { Action::Next } else { Action::Previous };
                self.find_action(action);
                return self.redraw();
            }
            Command::Format(f) => ed.toggle(f),
            Command::Link => ed.toggle_link(),
            Command::Heading(level) => ed.heading(level),
            Command::HeadingLarger => ed.shift_heading(true),
            Command::HeadingSmaller => ed.shift_heading(false),
            Command::TableInsertRow { below } => {
                ed.table_command(|d, s| marginal_doc::table_insert_row(d, s, below));
            }
            Command::TableInsertColumn { right } => {
                ed.table_command(|d, s| marginal_doc::table_insert_column(d, s, right));
            }
            Command::TableDeleteRow => {
                ed.table_command(marginal_doc::table_delete_row);
            }
            Command::TableDeleteColumn => {
                ed.table_command(marginal_doc::table_delete_column);
            }
            Command::CodeLanguage => {
                ed.open_picker();
                reveal = false;
            }
            Command::Theme(name) => {
                self.set_theme(&name);
                return self.redraw();
            }
            Command::OpenThemesFolder => {
                match self.themes.ensure_folder() {
                    Ok(dir) => {
                        let _ = std::process::Command::new("open").arg(dir).spawn();
                    }
                    Err(e) => eprintln!("could not create the themes folder: {e}"),
                }
                return self.refresh_themes();
            }
            Command::ReloadThemes => {
                self.refresh_themes();
                return self.redraw();
            }
            Command::Appearance(value) => {
                self.session.settings.appearance = value.map(str::to_owned);
                self.editor.session_dirty = true;
                if value.is_none()
                    && let Some(w) = self.window()
                {
                    w.set_theme(None);
                    self.system_appearance = appearance_of(w.theme());
                }
                self.apply_appearance();
                self.sync_menus();
                return self.redraw();
            }
            Command::ToggleXray => {
                self.session.settings.xray = !self.session.settings.xray;
                self.editor.session_dirty = true;
                self.update_xray();
                self.sync_menus();
                return self.redraw();
            }
            Command::ZoomIn | Command::ZoomOut => {
                self.step_zoom(command == Command::ZoomIn);
                return self.redraw();
            }
            Command::ActualSize => {
                self.set_zoom(1.0);
                return self.redraw();
            }
            Command::GoToHeading => ed.open_goto(),
        }
        if reveal {
            self.editor.reveal_caret(fonts);
        }
        self.sync_menus();
        self.redraw();
    }

    /// Returns false when the key wasn't handled.
    fn key(&mut self, event: &KeyEvent, event_loop: &ActiveEventLoop) -> bool {
        let m = self.modifiers;
        let (cmd, alt, shift, ctrl) = (m.super_key(), m.alt_key(), m.shift_key(), m.control_key());
        let unit = |word_unit: Unit| {
            if cmd {
                Unit::Line
            } else if alt {
                word_unit
            } else {
                Unit::Char
            }
        };
        if let Some(bar) = &mut self.findbar
            && let Some(action) = bar.key(&event.logical_key, event.text.as_deref(), shift, cmd)
        {
            if let Some(action) = action {
                self.find_action(action);
            }
            return true;
        }
        // Popups take the keys they understand; anything else closes them.
        let popup_input = |text_ok: bool| match &event.logical_key {
            Key::Named(NamedKey::Escape) => Some(PickerInput::Cancel),
            Key::Named(NamedKey::ArrowUp) => Some(PickerInput::Up),
            Key::Named(NamedKey::ArrowDown) => Some(PickerInput::Down),
            Key::Named(NamedKey::Enter) => Some(PickerInput::Accept),
            Key::Named(NamedKey::Tab) if text_ok => Some(PickerInput::Accept),
            Key::Named(NamedKey::Backspace) if text_ok => Some(PickerInput::Backspace),
            _ if cmd || ctrl || !text_ok => None,
            _ => event
                .text
                .as_ref()
                .filter(|t| !t.chars().any(char::is_control))
                .map(|t| PickerInput::Text(t.to_string())),
        };
        if self.editor.menu.is_some() {
            match popup_input(false) {
                Some(input) => {
                    self.editor.menu_input(input);
                    return true;
                }
                None => self.editor.menu = None,
            }
        }
        if self.editor.picker.is_some() {
            match popup_input(true) {
                Some(input) => {
                    self.editor.picker_input(input);
                    return true;
                }
                None => self.editor.picker = None,
            }
        }
        if self.editor.goto.is_some() {
            match popup_input(true) {
                Some(input) => {
                    self.editor.goto_input(input);
                    self.editor.settle_scroll(&mut self.fonts);
                    return true;
                }
                None => self.editor.goto = None,
            }
        }
        // Menu shortcuts normally arrive as menu commands; these cover keys the menu can't
        // express (⌥ changes the character) and windows without a menu bar.
        if cmd && alt && event.physical_key == PhysicalKey::Code(KeyCode::KeyL) {
            return self.editor.open_picker();
        }
        let fonts = &mut self.fonts;
        let ed = &mut self.editor;
        let in_table = ed.in_table();
        match &event.logical_key {
            Key::Named(NamedKey::Escape) => {
                if !ed.clear_table_focus() {
                    if self.findbar.is_some() {
                        self.close_find();
                        return true;
                    }
                    return false;
                }
            }
            // Tables: ⌥⇧↑↓ moves the row, Tab walks the cells. (Insert and delete are in the menu.)
            Key::Named(NamedKey::ArrowUp | NamedKey::ArrowDown) if in_table && alt && shift => {
                let up = event.logical_key == Key::Named(NamedKey::ArrowUp);
                ed.table_command(|d, s| marginal_doc::table_move_row(d, s, up));
            }
            Key::Named(NamedKey::Tab) if in_table => {
                ed.table_command(|d, s| marginal_doc::table_tab(d, s, shift));
            }
            Key::Named(NamedKey::ArrowLeft) => ed.motion(fonts, Direction::Back, unit(Unit::Word), shift),
            Key::Named(NamedKey::ArrowRight) => ed.motion(fonts, Direction::Forward, unit(Unit::Word), shift),
            Key::Named(NamedKey::ArrowUp) if cmd => ed.motion(fonts, Direction::Up, Unit::Document, shift),
            Key::Named(NamedKey::ArrowDown) if cmd => ed.motion(fonts, Direction::Down, Unit::Document, shift),
            Key::Named(NamedKey::ArrowUp) => ed.motion(fonts, Direction::Up, Unit::Line, shift),
            Key::Named(NamedKey::ArrowDown) => ed.motion(fonts, Direction::Down, Unit::Line, shift),
            Key::Named(NamedKey::Home) => ed.motion(fonts, Direction::Back, Unit::Line, shift),
            Key::Named(NamedKey::End) => ed.motion(fonts, Direction::Forward, Unit::Line, shift),
            Key::Named(NamedKey::Backspace) if cmd => ed.delete_back_by(fonts, Unit::Line),
            Key::Named(NamedKey::Backspace) if alt => ed.delete_back_by(fonts, Unit::Word),
            Key::Named(NamedKey::Backspace) => ed.backspace(),
            Key::Named(NamedKey::Delete) => ed.delete_forward(),
            Key::Named(NamedKey::Enter) if shift => ed.line_break(),
            Key::Named(NamedKey::Enter) => ed.newline(),
            Key::Named(NamedKey::Tab) => ed.type_text("\t"),
            Key::Character(c) if ctrl && !cmd => match c.as_str() {
                "a" => ed.motion(fonts, Direction::Back, Unit::Line, shift),
                "e" => ed.motion(fonts, Direction::Forward, Unit::Line, shift),
                "b" => ed.motion(fonts, Direction::Back, Unit::Char, shift),
                "f" => ed.motion(fonts, Direction::Forward, Unit::Char, shift),
                "p" => ed.motion(fonts, Direction::Up, Unit::Line, shift),
                "n" => ed.motion(fonts, Direction::Down, Unit::Line, shift),
                "d" => ed.delete_forward(),
                "h" => ed.backspace(),
                _ => return false,
            },
            Key::Character(c) if cmd => {
                let command = match c.to_lowercase().as_str() {
                    "s" if shift => Command::SaveAs,
                    "s" => Command::Save,
                    "q" => Command::Quit,
                    "z" if shift => Command::Redo,
                    "z" => Command::Undo,
                    "a" => Command::SelectAll,
                    "c" => Command::Copy,
                    "x" if shift => Command::Format(InlineFormat::Strike),
                    "x" => Command::Cut,
                    "v" => Command::Paste,
                    "b" => Command::Format(InlineFormat::Strong),
                    "i" => Command::Format(InlineFormat::Emphasis),
                    "e" => Command::Format(InlineFormat::Code),
                    "k" => Command::Link,
                    "f" if alt => Command::FindReplace,
                    "f" => Command::Find,
                    "g" if shift => Command::FindPrevious,
                    "g" => Command::FindNext,
                    "]" => Command::HeadingLarger,
                    "[" => Command::HeadingSmaller,
                    d @ ("0" | "1" | "2" | "3" | "4" | "5" | "6") => Command::Heading(d.parse().unwrap()),
                    _ => return false,
                };
                self.command(command, event_loop);
                return true;
            }
            _ => match event.text.as_ref().filter(|t| !t.chars().any(char::is_control)) {
                Some(text) => ed.type_text(text),
                None => return false,
            },
        }
        self.editor.reveal_caret(&mut self.fonts);
        true
    }

    fn click_count(&mut self, at: (f32, f32)) -> u8 {
        let now = Instant::now();
        let count = match self.last_click {
            Some((t, p, n))
                if now.duration_since(t) < MULTI_CLICK && (p.0 - at.0).abs() < 4.0 && (p.1 - at.1).abs() < 4.0 =>
            {
                (n % 3) + 1
            }
            _ => 1,
        };
        self.last_click = Some((now, at, count));
        count
    }

    fn render(&mut self) {
        let Some(window) = self.window() else { return };
        let scale = window.scale_factor();
        let now = Instant::now();
        self.refresh_find();
        self.editor.settle_scroll(&mut self.fonts);
        let (inset, zoom) = (self.inset() as f64, self.zoom() as f64);
        let (w, h) = (self.window_size.0 as f64, self.window_size.1 as f64);
        self.scene.reset();
        let screen = Affine::scale(scale);
        self.scene.push_clip_layer(vello::peniko::Fill::NonZero, screen, &Rect::new(0.0, inset, w, h));
        let base = screen * Affine::translate((0.0, inset)) * Affine::scale(zoom);
        self.editor.paint(&mut self.scene, &mut self.fonts, base, now);
        self.scene.pop_layer();
        let blink_on = self.editor.blink_phase(now).0;
        if let Some(bar) = &mut self.findbar {
            bar.paint(
                &mut self.scene,
                &mut self.fonts,
                &self.editor.theme,
                self.editor.appearance,
                screen,
                w as f32,
                blink_on,
            );
        }
        self.drawn_blink = blink_on;

        let Some(state) = &mut self.state else { return };
        let surface = &state.surface;
        let device_handle = &self.context.devices[surface.dev_id];
        let (width, height) = (surface.config.width, surface.config.height);
        self.renderers[surface.dev_id]
            .as_mut()
            .unwrap()
            .render_to_texture(
                &device_handle.device,
                &device_handle.queue,
                &self.scene,
                &surface.target_view,
                &RenderParams {
                    base_color: color(self.editor.background()),
                    width,
                    height,
                    antialiasing_method: AaConfig::Area,
                },
            )
            .expect("render");
        let surface_texture = match surface.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            _ => {
                window.request_redraw();
                return;
            }
        };
        let mut encoder =
            device_handle.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("blit") });
        surface.blitter.copy(
            &device_handle.device,
            &mut encoder,
            &surface.target_view,
            &surface_texture.texture.create_view(&wgpu::TextureViewDescriptor::default()),
        );
        device_handle.queue.submit([encoder.finish()]);
        device_handle.queue.present(surface_texture);

        if let Some((x, y, cw, ch)) = self.editor.caret_window_rect(&mut self.fonts) {
            let z = zoom as f32;
            window.set_ime_cursor_area(
                LogicalPosition::new(x * z, y * z + inset as f32),
                LogicalSize::new((cw * z).max(1.0), ch * z),
            );
        }
        let title = self.editor.title();
        if title != self.title {
            window.set_title(&title);
            self.title = title;
        }
    }
}

fn appearance_of(theme: Option<WinitTheme>) -> Appearance {
    match theme {
        Some(WinitTheme::Light) => Appearance::Light,
        _ => Appearance::Dark,
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title(self.editor.title())
            .with_inner_size(LogicalSize::new(1000.0, 800.0));
        let window = Arc::new(event_loop.create_window(attrs).expect("create window"));
        window.set_ime_allowed(true);
        let size = window.inner_size();
        let surface = pollster::block_on(self.context.create_surface(
            window.clone(),
            size.width,
            size.height,
            wgpu::PresentMode::AutoVsync,
        ))
        .expect("create surface");
        self.renderers.resize_with(self.context.devices.len(), || None);
        self.renderers[surface.dev_id].get_or_insert_with(|| {
            Renderer::new(&self.context.devices[surface.dev_id].device, renderer_options()).expect("create renderer")
        });
        self.system_appearance = appearance_of(window.theme());
        let logical = size.to_logical::<f32>(window.scale_factor());
        self.window_size = (logical.width, logical.height);
        self.state = Some(RenderState { surface, window });
        self.apply_appearance();
        self.layout_editor();

        let mut bar = MenuBar::new();
        #[cfg(target_os = "macos")]
        bar.menu.init_for_nsapp();
        bar.set_recent(&self.session.settings.recent_files);
        bar.set_themes(&self.themes.entries, self.themes.selected());
        self.menubar = Some(bar);
        self.sync_menus();
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        let UserEvent::Menu(e) = event;
        if let Some(command) = self.menubar.as_ref().and_then(|b| b.command(e.id())) {
            self.command(command, event_loop);
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(state) = &mut self.state else { return };
        let scale = state.window.scale_factor();
        let window = state.window.clone();
        match event {
            WindowEvent::CloseRequested => {
                // Hot exit: nothing to ask, everything is already journaled.
                self.persist_session();
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                if size.width > 0 && size.height > 0 {
                    self.context.resize_surface(&mut state.surface, size.width, size.height);
                    let logical = size.to_logical::<f32>(scale);
                    self.window_size = (logical.width, logical.height);
                    self.layout_editor();
                }
                window.request_redraw();
            }
            WindowEvent::ThemeChanged(theme) => {
                if self.session.settings.appearance.is_none() {
                    self.system_appearance = appearance_of(Some(theme));
                    self.apply_appearance();
                }
                window.request_redraw();
            }
            WindowEvent::Focused(true) => {
                self.refresh_themes();
                window.request_redraw();
            }
            WindowEvent::ModifiersChanged(m) => {
                self.modifiers = m.state();
                let before = self.editor.xray;
                self.update_xray();
                if before != self.editor.xray {
                    window.request_redraw();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let p = position.to_logical::<f32>(scale);
                self.cursor = (p.x, p.y);
                let at = (p.y >= self.inset()).then(|| self.to_editor((p.x, p.y)));
                if self.editor.pointer_moved(&mut self.fonts, at) {
                    window.request_redraw();
                }
                if self.mouse_down {
                    let (x, y) = self.to_editor((p.x, p.y));
                    self.editor.mouse_drag(&mut self.fonts, x, y);
                    window.request_redraw();
                }
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Right, .. } => {
                let (x, y) = self.to_editor(self.cursor);
                if self.editor.context_click(&mut self.fonts, x, y) {
                    window.request_redraw();
                }
            }
            WindowEvent::MouseInput { state: button_state, button: MouseButton::Left, .. } => {
                if button_state == ElementState::Pressed {
                    if self.cursor.1 < self.inset() {
                        let (x, y) = self.cursor;
                        if let Some(action) = self.findbar.as_mut().and_then(|b| b.click(x, y)) {
                            self.find_action(action);
                        }
                    } else {
                        if let Some(bar) = &mut self.findbar {
                            bar.focus = None;
                        }
                        let clicks = self.click_count(self.cursor);
                        let (x, y) = self.to_editor(self.cursor);
                        self.editor.mouse_down(&mut self.fonts, x, y, self.modifiers.shift_key(), clicks);
                        self.mouse_down = true;
                    }
                } else {
                    self.editor.mouse_up(&mut self.fonts);
                    self.mouse_down = false;
                }
                window.request_redraw();
            }
            WindowEvent::CursorLeft { .. } => {
                if self.editor.pointer_moved(&mut self.fonts, None) {
                    window.request_redraw();
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_, y) => -y * 40.0,
                    MouseScrollDelta::PixelDelta(p) => -(p.to_logical::<f32>(scale).y),
                };
                let zoom = self.zoom();
                self.editor.scroll_by(&mut self.fonts, dy / zoom);
                window.request_redraw();
            }
            WindowEvent::Ime(ime) => {
                let bar_focused = self.findbar.as_ref().is_some_and(|b| b.focus.is_some());
                match ime {
                    Ime::Commit(text) if bar_focused => {
                        let bar = self.findbar.as_mut().unwrap();
                        if let Some(field) = bar.focused_field() {
                            field.insert(&text);
                        }
                        if bar.focus == Some(findbar::Focus::Find) {
                            self.find_action(Action::Search);
                        }
                    }
                    Ime::Preedit(..) if bar_focused => {}
                    Ime::Commit(text) if self.editor.picker.is_some() => {
                        self.editor.picker_input(PickerInput::Text(text))
                    }
                    Ime::Commit(text) if self.editor.goto.is_some() => self.editor.goto_input(PickerInput::Text(text)),
                    Ime::Preedit(..) if self.editor.picker.is_some() || self.editor.goto.is_some() => {}
                    Ime::Preedit(text, _) => {
                        self.editor.preedit = (!text.is_empty()).then_some(text);
                        self.editor.invalidate();
                    }
                    Ime::Commit(text) => self.editor.type_text(&text),
                    Ime::Enabled | Ime::Disabled => {}
                }
                window.request_redraw();
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                if self.key(&event, event_loop) {
                    self.sync_menus();
                    window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => self.render(),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        if now.duration_since(self.editor.activity) >= SYNC_AFTER
            && let Err(e) = self.editor.buffer.sync()
        {
            eprintln!("journal sync failed: {e}");
        }
        if self.editor.session_dirty && now.duration_since(self.session_written) >= SESSION_EVERY {
            self.persist_session();
        }
        let idle = now.duration_since(self.editor.activity);
        // Like macOS, stop blinking after a while idle to save power.
        let (visible, next_blink) =
            if idle > Duration::from_secs(30) { (true, None) } else { self.editor.blink_phase(now) };
        if visible != self.drawn_blink {
            self.redraw();
        }
        let wake = [
            next_blink,
            self.editor.session_dirty.then(|| self.session_written + SESSION_EVERY),
            (idle < SYNC_AFTER).then(|| self.editor.activity + SYNC_AFTER),
        ]
        .into_iter()
        .flatten()
        .min();
        event_loop.set_control_flow(match wake {
            Some(t) => ControlFlow::WaitUntil(t),
            None => ControlFlow::Wait,
        });
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.persist_session();
        let _ = self.editor.buffer.sync();
    }
}

fn set_clipboard(text: &str) {
    if let Err(e) = arboard::Clipboard::new().and_then(|mut c| c.set_text(text)) {
        eprintln!("clipboard: {e}");
    }
}

fn get_clipboard() -> Option<String> {
    arboard::Clipboard::new().and_then(|mut c| c.get_text()).ok()
}

/// Opens `path` (restoring unsaved edits if it's already in the session), or restores the last
/// active buffer, or starts an untitled one.
fn open(
    store: &Store,
    session: &SessionFile,
    path: Option<&Path>,
) -> anyhow::Result<(marginal_session::Buffer, BufferMeta)> {
    if let Some(path) = path {
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let disk = std::fs::read_to_string(&canonical).unwrap_or_default();
        if let Some(meta) = session.buffers.iter().find(|m| m.path.as_deref() == Some(canonical.as_path())) {
            let buffer = store.open_buffer(meta)?;
            if buffer.disk_text == disk || buffer.is_dirty() {
                // TODO(M3): when dirty *and* changed on disk, three-way merge instead of keeping ours.
                return Ok((buffer, meta.clone()));
            }
        }
        let buffer = store.create_buffer(Some(&canonical), &disk)?;
        let meta = buffer.meta(Selection::default(), 0.0);
        return Ok((buffer, meta));
    }
    if let Some(meta) = session.active.as_ref().and_then(|id| session.buffers.iter().find(|m| &m.id == id))
        && let Ok(buffer) = store.open_buffer(meta)
    {
        return Ok((buffer, meta.clone()));
    }
    let buffer = store.create_buffer(None, SAMPLE)?;
    let meta = buffer.meta(Selection::default(), 0.0);
    Ok((buffer, meta))
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let mut path: Option<PathBuf> = None;
    let mut snapshot_out: Option<PathBuf> = None;
    let mut appearance = Appearance::Dark;
    let mut caret: Option<usize> = None;
    let mut xray = false;
    let mut picker_query: Option<String> = None;
    let mut pointer: Option<(f32, f32)> = None;
    let mut grip_menu: Option<usize> = None;
    let mut theme_name: Option<String> = None;
    let mut goto_query: Option<String> = None;
    let mut find_query: Option<String> = None;
    let mut replace_with: Option<String> = None;
    let mut regex = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--snapshot" => snapshot_out = args.next().map(PathBuf::from),
            "--light" => appearance = Appearance::Light,
            "--xray" => xray = true,
            "--caret" => caret = args.next().and_then(|c| c.parse().ok()),
            "--picker" => picker_query = args.next(),
            "--grip-menu" => grip_menu = args.next().and_then(|c| c.parse().ok()),
            "--theme" => theme_name = args.next(),
            "--goto" => goto_query = args.next(),
            "--find" => find_query = args.next(),
            "--replace" => replace_with = args.next(),
            "--regex" => regex = true,
            "--pointer" => {
                pointer = args.next().and_then(|p| {
                    let (x, y) = p.split_once(',')?;
                    Some((x.parse().ok()?, y.parse().ok()?))
                })
            }
            _ => path = Some(PathBuf::from(arg)),
        }
    }
    let mut fonts = Fonts::new();

    if let Some(out) = snapshot_out {
        // Snapshots never touch the real session.
        let tmp = std::env::temp_dir().join(format!("marginal-snapshot-{}", std::process::id()));
        let store = Store::at(&tmp)?;
        let text = match &path {
            Some(p) => std::fs::read_to_string(p)?,
            None => SAMPLE.to_owned(),
        };
        let buffer = store.create_buffer(None, &text)?;
        let theme = theme_name.as_deref().and_then(Theme::built_in).unwrap_or_else(Theme::terminal);
        let mut editor = Editor::new(buffer, Selection::default(), theme, appearance);
        editor.xray = xray;
        if let Some(c) = caret {
            editor.set_selection(Selection::caret(c));
        }
        if let Some(q) = picker_query {
            editor.recent_languages = vec!["rust".into(), "bash".into()];
            if editor.open_picker() {
                editor.picker_input(PickerInput::Text(q));
            }
        }
        if let Some(q) = goto_query {
            editor.open_goto();
            if !q.is_empty() {
                editor.goto_input(PickerInput::Text(q));
            }
        }
        editor.width = 1000.0;
        editor.height = 900.0;
        editor.pointer_moved(&mut fonts, pointer);
        if let Some(col) = grip_menu {
            editor.open_grip_menu(&mut fonts, col);
        }
        let mut bar = find_query.map(|q| {
            let mut bar = FindBar::new(marginal_doc::FindOptions { regex, ..Default::default() });
            bar.find.set(&q);
            if let Some(r) = &replace_with {
                bar.show_replace = true;
                bar.replace.set(r);
            }
            bar
        });
        let result = snapshot::snapshot(&mut editor, &mut fonts, bar.as_mut(), &out, 2.0);
        drop(editor);
        let _ = std::fs::remove_dir_all(&tmp);
        return result;
    }

    let store = Store::default_location()?;
    let mut session = store.load_session();
    let mut themes = Themes::new(store.root().join("Themes"), session.settings.theme.clone());
    let selected = themes.selected().to_owned();
    let theme = themes.load(&selected).unwrap_or_else(|e| {
        eprintln!("theme: {e}");
        Theme::terminal()
    });
    let (buffer, meta) = open(&store, &session, path.as_deref())?;
    if let Some(p) = &buffer.path {
        session.settings.remember_file(p);
    }
    let mut editor = Editor::new(buffer, meta.selection, theme, appearance);
    editor.scroll = meta.scroll;
    editor.recent_languages = session.recent_languages.clone();
    editor.xray = session.settings.xray;

    let mut builder = EventLoop::<UserEvent>::with_user_event();
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::EventLoopBuilderExtMacOS;
        builder.with_default_menu(false);
    }
    let event_loop = builder.build()?;
    let proxy = event_loop.create_proxy();
    muda::MenuEvent::set_event_handler(Some(move |e| {
        let _ = proxy.send_event(UserEvent::Menu(e));
    }));
    let mut app = App {
        context: RenderContext::new(),
        renderers: Vec::new(),
        state: None,
        scene: Scene::new(),
        title: editor.title(),
        fonts,
        editor,
        store,
        session,
        session_written: Instant::now(),
        modifiers: ModifiersState::empty(),
        cursor: (0.0, 0.0),
        mouse_down: false,
        last_click: None,
        drawn_blink: true,
        menubar: None,
        themes,
        findbar: None,
        window_size: (1000.0, 800.0),
        system_appearance: appearance,
    };
    app.persist_session();
    event_loop.run_app(&mut app)?;
    Ok(())
}
