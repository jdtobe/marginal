//! The macOS menu bar. Every item maps to a [`Command`]; the app runs commands the same way
//! whether they come from the menu or a key.

use std::path::PathBuf;

use marginal_doc::InlineFormat;
use muda::accelerator::Accelerator;
use muda::{AboutMetadata, CheckMenuItem, Menu, MenuId, MenuItem, PredefinedMenuItem, Submenu};

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    New,
    Open,
    OpenRecent(PathBuf),
    ClearRecent,
    Save,
    SaveAs,
    RevealInFinder,
    CopyPath,
    Close,
    Quit,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    Find,
    FindReplace,
    FindNext,
    FindPrevious,
    Format(InlineFormat),
    Link,
    Heading(u8),
    HeadingLarger,
    HeadingSmaller,
    TableInsertRow {
        below: bool,
    },
    TableInsertColumn {
        right: bool,
    },
    TableDeleteRow,
    TableDeleteColumn,
    CodeLanguage,
    Theme(String),
    OpenThemesFolder,
    ReloadThemes,
    /// `None` follows the system.
    Appearance(Option<&'static str>),
    ToggleXray,
    ZoomIn,
    ZoomOut,
    ActualSize,
    GoToHeading,
}

/// The menu bar plus the items whose state changes (checks, enabled, lists).
pub struct MenuBar {
    pub menu: Menu,
    commands: Vec<(MenuId, Command)>,
    recent: Submenu,
    themes: Submenu,
    theme_items: Vec<(String, CheckMenuItem)>,
    appearance: Vec<(Option<&'static str>, CheckMenuItem)>,
    xray: CheckMenuItem,
    needs_file: Vec<MenuItem>,
}

fn accel(s: &str) -> Option<Accelerator> {
    (!s.is_empty()).then(|| s.parse().unwrap_or_else(|e| panic!("bad accelerator {s}: {e}")))
}

impl MenuBar {
    pub fn new() -> Self {
        let menu = Menu::new();
        let mut bar = Self {
            menu,
            commands: Vec::new(),
            recent: Submenu::new("Open Recent", true),
            themes: Submenu::new("Theme", true),
            theme_items: Vec::new(),
            appearance: Vec::new(),
            xray: CheckMenuItem::new("Show Markdown Source", true, false, accel("Alt+CmdOrCtrl+X")),
            needs_file: Vec::new(),
        };
        bar.build();
        bar
    }

    fn item(&mut self, label: &str, keys: &str, command: Command) -> MenuItem {
        let item = MenuItem::new(label, true, accel(keys));
        self.commands.push((item.id().clone(), command));
        item
    }

    fn build(&mut self) {
        let sep = PredefinedMenuItem::separator;

        let app = Submenu::new("Marginal", true);
        let quit = self.item("Quit Marginal", "CmdOrCtrl+Q", Command::Quit);
        let about = AboutMetadata {
            name: Some("Marginal".into()),
            version: Some(env!("CARGO_PKG_VERSION").into()),
            ..Default::default()
        };
        app.append_items(&[
            &PredefinedMenuItem::about(Some("About Marginal"), Some(about)),
            &sep(),
            &PredefinedMenuItem::services(None),
            &sep(),
            &PredefinedMenuItem::hide(None),
            &PredefinedMenuItem::hide_others(None),
            &PredefinedMenuItem::show_all(None),
            &sep(),
            &quit,
        ])
        .unwrap();

        let file = Submenu::new("File", true);
        let reveal = self.item("Reveal in Finder", "Alt+CmdOrCtrl+R", Command::RevealInFinder);
        let copy_path = self.item("Copy Path", "Alt+Shift+CmdOrCtrl+C", Command::CopyPath);
        self.needs_file = vec![reveal.clone(), copy_path.clone()];
        let recent = self.recent.clone();
        file.append_items(&[
            &self.item("New", "CmdOrCtrl+N", Command::New),
            &self.item("Open…", "CmdOrCtrl+O", Command::Open),
            &recent,
            &sep(),
            &self.item("Close Window", "CmdOrCtrl+W", Command::Close),
            &self.item("Save", "CmdOrCtrl+S", Command::Save),
            &self.item("Save As…", "Shift+CmdOrCtrl+S", Command::SaveAs),
            &sep(),
            &reveal,
            &copy_path,
        ])
        .unwrap();

        let edit = Submenu::new("Edit", true);
        let find = Submenu::new("Find", true);
        find.append_items(&[
            &self.item("Find…", "CmdOrCtrl+F", Command::Find),
            &self.item("Find and Replace…", "Alt+CmdOrCtrl+F", Command::FindReplace),
            &self.item("Find Next", "CmdOrCtrl+G", Command::FindNext),
            &self.item("Find Previous", "Shift+CmdOrCtrl+G", Command::FindPrevious),
        ])
        .unwrap();
        edit.append_items(&[
            &self.item("Undo", "CmdOrCtrl+Z", Command::Undo),
            &self.item("Redo", "Shift+CmdOrCtrl+Z", Command::Redo),
            &sep(),
            &self.item("Cut", "CmdOrCtrl+X", Command::Cut),
            &self.item("Copy", "CmdOrCtrl+C", Command::Copy),
            &self.item("Paste", "CmdOrCtrl+V", Command::Paste),
            &self.item("Select All", "CmdOrCtrl+A", Command::SelectAll),
            &sep(),
            &find,
        ])
        .unwrap();

        let format = Submenu::new("Format", true);
        let heading = Submenu::new("Heading", true);
        heading.append(&self.item("Paragraph", "CmdOrCtrl+0", Command::Heading(0))).unwrap();
        for level in 1..=6u8 {
            heading
                .append(&self.item(&format!("Heading {level}"), &format!("CmdOrCtrl+{level}"), Command::Heading(level)))
                .unwrap();
        }
        heading
            .append_items(&[
                &sep(),
                &self.item("Larger", "CmdOrCtrl+]", Command::HeadingLarger),
                &self.item("Smaller", "CmdOrCtrl+[", Command::HeadingSmaller),
            ])
            .unwrap();
        let table = Submenu::new("Table", true);
        table
            .append_items(&[
                &self.item("Insert Row Above", "Alt+CmdOrCtrl+Up", Command::TableInsertRow { below: false }),
                &self.item("Insert Row Below", "Alt+CmdOrCtrl+Down", Command::TableInsertRow { below: true }),
                &self.item("Insert Column Left", "Alt+CmdOrCtrl+Left", Command::TableInsertColumn { right: false }),
                &self.item("Insert Column Right", "Alt+CmdOrCtrl+Right", Command::TableInsertColumn { right: true }),
                &sep(),
                &self.item("Delete Row", "Alt+CmdOrCtrl+Backspace", Command::TableDeleteRow),
                &self.item("Delete Column", "Alt+Shift+CmdOrCtrl+Backspace", Command::TableDeleteColumn),
            ])
            .unwrap();
        format
            .append_items(&[
                &self.item("Bold", "CmdOrCtrl+B", Command::Format(InlineFormat::Strong)),
                &self.item("Italic", "CmdOrCtrl+I", Command::Format(InlineFormat::Emphasis)),
                &self.item("Code", "CmdOrCtrl+E", Command::Format(InlineFormat::Code)),
                &self.item("Strikethrough", "Shift+CmdOrCtrl+X", Command::Format(InlineFormat::Strike)),
                &self.item("Link", "CmdOrCtrl+K", Command::Link),
                &sep(),
                &heading,
                &table,
                &self.item("Code Block Language…", "Alt+CmdOrCtrl+L", Command::CodeLanguage),
            ])
            .unwrap();

        let view = Submenu::new("View", true);
        let appearance = Submenu::new("Appearance", true);
        for (label, value) in [("Match System", None), ("Light", Some("light")), ("Dark", Some("dark"))] {
            let item = CheckMenuItem::new(label, true, false, None);
            self.commands.push((item.id().clone(), Command::Appearance(value)));
            appearance.append(&item).unwrap();
            self.appearance.push((value, item));
        }
        self.commands.push((self.xray.id().clone(), Command::ToggleXray));
        let (themes, xray) = (self.themes.clone(), self.xray.clone());
        view.append_items(&[
            &themes,
            &appearance,
            &sep(),
            &xray,
            &sep(),
            &self.item("Zoom In", "CmdOrCtrl+=", Command::ZoomIn),
            &self.item("Zoom Out", "CmdOrCtrl+-", Command::ZoomOut),
            &self.item("Actual Size", "Shift+CmdOrCtrl+0", Command::ActualSize),
            &sep(),
            &self.item("Go to Heading…", "Shift+CmdOrCtrl+O", Command::GoToHeading),
        ])
        .unwrap();

        let window = Submenu::new("Window", true);
        window
            .append_items(&[
                &PredefinedMenuItem::minimize(None),
                &PredefinedMenuItem::maximize(Some("Zoom")),
                &sep(),
                &PredefinedMenuItem::bring_all_to_front(None),
            ])
            .unwrap();
        window.set_as_windows_menu_for_nsapp();

        self.menu.append_items(&[&app, &file, &edit, &format, &view, &window]).unwrap();
    }

    pub fn command(&self, id: &MenuId) -> Option<Command> {
        self.commands.iter().find(|(i, _)| i == id).map(|(_, c)| c.clone())
    }

    /// Empties a submenu and forgets the commands of its items.
    fn clear(&mut self, menu: &Submenu) {
        while let Some(item) = menu.remove_at(0) {
            self.commands.retain(|(id, _)| id != item.id());
        }
    }

    /// Rebuilds the Open Recent submenu.
    pub fn set_recent(&mut self, files: &[PathBuf]) {
        let recent = self.recent.clone();
        self.clear(&recent);
        for path in files {
            let label =
                path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
            let item = self.item(&label, "", Command::OpenRecent(path.clone()));
            self.recent.append(&item).unwrap();
        }
        let clear = self.item("Clear Menu", "", Command::ClearRecent);
        clear.set_enabled(!files.is_empty());
        self.recent.append_items(&[&PredefinedMenuItem::separator(), &clear]).unwrap();
    }

    /// Rebuilds the Theme submenu: built-ins, then user themes, then the folder actions.
    pub fn set_themes(&mut self, themes: &[crate::themes::ThemeEntry], selected: &str) {
        let menu = self.themes.clone();
        self.clear(&menu);
        self.theme_items.clear();
        let mut was_built_in = true;
        for entry in themes {
            if was_built_in && entry.path.is_some() {
                self.themes.append(&PredefinedMenuItem::separator()).unwrap();
                was_built_in = false;
            }
            let item = CheckMenuItem::new(&entry.name, true, entry.name == selected, None);
            self.commands.push((item.id().clone(), Command::Theme(entry.name.clone())));
            self.themes.append(&item).unwrap();
            self.theme_items.push((entry.name.clone(), item));
        }
        self.themes.append(&PredefinedMenuItem::separator()).unwrap();
        let open = self.item("Open Themes Folder", "", Command::OpenThemesFolder);
        let reload = self.item("Reload Themes", "", Command::ReloadThemes);
        self.themes.append_items(&[&open, &reload]).unwrap();
    }

    /// Updates check marks and enabled states.
    pub fn sync(&self, theme: &str, appearance: Option<&str>, xray: bool, has_file: bool) {
        for (name, item) in &self.theme_items {
            item.set_checked(name == theme);
        }
        for (value, item) in &self.appearance {
            item.set_checked(*value == appearance);
        }
        self.xray.set_checked(xray);
        for item in &self.needs_file {
            item.set_enabled(has_file);
        }
    }
}
