//! Application state and the per-frame UI orchestration.
//!
//! Ported from bible-rs (MIT).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use anyhow::Result;
use eframe::egui::{self, Align2, Frame, Id, Key, Modifiers, Sense, Stroke, Ui, vec2};
use quran_core::catalog::CatalogEntry;
use quran_core::library::{DEFAULT_ID, Library};
use quran_core::meta::{AyahRef, Juz, SurahId};
use quran_core::study::StudyStore;
use serde::{Deserialize, Serialize};

use crate::actions::{Action, ExportKind, SettingsTab, SidebarTab};
use crate::note_popover::NotePopover;
use crate::palette::CommandPalette;
use crate::reader::layout::{Mode, Typography};
use crate::reader::{Env, Reader};
use crate::settings_ui::SettingsUi;
use crate::sidebar::Sidebar;
use crate::theme::{self, Theme};
use crate::widgets;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub translation: String,
    pub parallel: Vec<String>,
    pub parallel_on: bool,
    pub theme: Theme,
    pub typo: Typography,
    pub sidebar_open: bool,
    pub sidebar_tab: SidebarTab,
    /// Follow the system's light or dark appearance instead of `theme`.
    pub theme_system: bool,
    /// Where reading left off: surah and ayah.
    pub surah: u8,
    pub ayah: u16,
    pub whole_words: bool,
    /// Appearance: the accent colour, the translation picker in the toolbar, the sidebar
    /// attached to the window's edge rather than floating, names for the highlight colours
    /// (empty for the colour's own), less motion, and the flash on an ayah jumped to.
    pub accent: theme::Accent,
    pub toolbar_translation: bool,
    pub sidebar_attached: bool,
    pub highlight_names: Vec<String>,
    pub reduce_motion: bool,
    pub flash_jumps: bool,
    /// Data: what exports include (highlights, notes, bookmarks, history), and automatic
    /// backups (on, and weekly rather than daily).
    pub export_include: [bool; 4],
    pub backup_auto: bool,
    pub backup_weekly: bool,
    /// Chords chosen in Settings › Shortcuts, by row id (`keymap::ALL`).
    pub shortcuts: std::collections::BTreeMap<String, String>,
    /// The share sheet's style and options, as last used.
    pub share: crate::share_sheet::ShareOptions,
    /// Remind about the day's plan reading at 7:00 while the app is open.
    pub plan_reminder: bool,
    /// The first-launch screens are done.
    pub onboarded: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            translation: DEFAULT_ID.into(),
            parallel: vec!["en.pickthall".into()],
            parallel_on: false,
            theme: Theme::Light,
            typo: Typography::default(),
            sidebar_open: true,
            sidebar_tab: SidebarTab::Surahs,
            theme_system: false,
            surah: 1,
            ayah: 1,
            whole_words: false,
            plan_reminder: false,
            accent: theme::Accent::Emerald,
            toolbar_translation: true,
            sidebar_attached: false,
            highlight_names: Vec::new(),
            reduce_motion: false,
            flash_jumps: true,
            shortcuts: Default::default(),
            export_include: [true, true, true, false],
            backup_auto: true,
            backup_weekly: false,
            share: Default::default(),
            onboarded: false,
        }
    }
}

impl Settings {
    /// A highlight colour's name: the one given in Settings, else the colour's.
    pub fn highlight_name(&self, c: quran_core::study::HighlightColor) -> String {
        let i = quran_core::study::HighlightColor::ALL.iter().position(|x| *x == c).unwrap_or(0);
        match self.highlight_names.get(i).map(|n| n.trim()).filter(|n| !n.is_empty()) {
            Some(n) => n.to_string(),
            None => {
                let name = c.name();
                name[..1].to_uppercase() + &name[1..]
            }
        }
    }

    pub fn load(study: &StudyStore) -> Self {
        study.setting("app").and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self, study: &StudyStore) {
        if let Ok(s) = serde_json::to_string(self) {
            let _ = study.set_setting("app", &s);
        }
    }

    pub fn position(&self) -> AyahRef {
        AyahRef::checked(self.surah as u32, self.ayah as u32).unwrap_or(AyahRef::FIRST)
    }
}

/// Everything loaded before the first frame (done on a background thread while the window opens).
pub struct Init {
    pub data_dir: PathBuf,
    pub study: StudyStore,
    pub library: Library,
    pub settings: Settings,
    pub elapsed: Duration,
}

pub fn initialize(data_dir: PathBuf, notify: Arc<dyn Fn() + Send + Sync>) -> Result<Init> {
    let t = Instant::now();
    let study = StudyStore::open(&data_dir.join("study.sqlite"))?;
    let mut settings = Settings::load(&study);
    let library = Library::open(&data_dir, notify);
    if !library.contains(&settings.translation) {
        settings.translation = DEFAULT_ID.into();
    }
    // Decode the text being read now; the rest warm up after the first frame.
    let _ = quran_core::text::quran();
    let _ = library.load(&settings.translation);
    Ok(Init { data_dir, study, library, settings, elapsed: t.elapsed() })
}

/// Back/forward navigation history.
#[derive(Default)]
pub struct History {
    back: Vec<AyahRef>,
    forward: Vec<AyahRef>,
}

impl History {
    pub fn push(&mut self, here: AyahRef) {
        if self.back.last() != Some(&here) {
            self.back.push(here);
            if self.back.len() > 200 {
                self.back.remove(0);
            }
        }
        self.forward.clear();
    }

    pub fn back(&mut self, here: AyahRef) -> Option<AyahRef> {
        let to = self.back.pop()?;
        self.forward.push(here);
        Some(to)
    }

    pub fn forward(&mut self, here: AyahRef) -> Option<AyahRef> {
        let to = self.forward.pop()?;
        self.back.push(here);
        Some(to)
    }

    pub fn can_back(&self) -> bool {
        !self.back.is_empty()
    }

    pub fn can_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    pub fn recent(&self) -> impl Iterator<Item = &AyahRef> {
        self.back.iter().rev()
    }
}

/// Work done off the UI thread (network), reported back here.
pub enum TaskResult {
    Catalog(Result<Vec<CatalogEntry>, String>),
    Downloaded(String, Result<Box<quran_core::Translation>, String>),
}

pub struct Startup {
    pub t0: Instant,
    pub probe: bool,
    pub init: Duration,
    pub first_ui: Option<Duration>,
    pub frames: u32,
}

pub struct QuranApp {
    pub data_dir: PathBuf,
    pub library: Library,
    pub study: StudyStore,
    pub settings: Settings,
    pub reader: Reader,
    pub history: History,
    pub sidebar: Sidebar,
    /// The sidebar's slide (0 hidden, 1 shown), and this frame's value and full width.
    sidebar_motion: crate::motion::Spring,
    sidebar_shown: f32,
    sidebar_extent: f32,
    /// The previous frame's time (egui seconds), and the clock motion runs on: frame time, except
    /// that one frame advances it by at most [`QuranApp::motion_step`], so a late frame delays a
    /// slide by a frame rather than making it jump.
    last_frame_time: f64,
    motion_clock: f64,
    motion_step: f64,
    palette: Option<CommandPalette>,
    settings_ui: Option<SettingsUi>,
    shortcuts_open: bool,
    shortcuts_overlay: crate::shortcuts_overlay::ShortcutsOverlay,
    /// The compact toolbar's More menu: open it next frame (demo), and its keyboard item.
    open_more: bool,
    more_highlight: Option<usize>,
    toasts: Vec<(String, f64)>,
    /// The main area (right of the sidebar), where toasts centre.
    main_area: egui::Rect,
    pub task_tx: Sender<TaskResult>,
    task_rx: Receiver<TaskResult>,
    /// Translations downloading now.
    pub downloading: Vec<String>,
    /// The translation catalogue, once listed (or why it couldn't be).
    pub catalog: Option<Result<Vec<CatalogEntry>, String>>,
    startup: Startup,
    /// Serialized settings as last written, and when.
    saved: (String, f64),
    warmed: bool,
    reading: crate::reading::Tracker,
    verse_menu: Option<crate::verse_menu::VerseMenu>,
    note_popover: Option<NotePopover>,
    /// The first-launch screens, until they are done.
    onboarding: Option<crate::onboarding::Onboarding>,
    /// Compare an ayah across translations, as a popover on it.
    compare: Option<crate::compare_popover::ComparePopover>,
    /// Sharing the selected ayahs as an image or text.
    share_sheet: Option<crate::share_sheet::ShareSheet>,
    /// Focus mode: full screen, the chrome hidden, larger type.
    focus: bool,
    /// Screenshots: show focus mode's reveal bar as if the pointer were at the top.
    focus_reveal: bool,
    /// When an automatic backup was last considered (egui time).
    backup_checked: Option<f64>,
    /// The day the plan reminder last showed.
    reminded: Option<i64>,
    /// The fonts include the system fallback for other scripts.
    other_scripts: bool,
    /// The fonts changed last frame: cached layouts point at the old glyph atlas.
    fonts_changed: bool,
    /// The glyph atlas as last seen: its options and how full it was. egui rebuilds the atlas
    /// when its options change (light and dark text are rendered differently) or it fills up,
    /// and layouts kept from before would draw the wrong glyphs.
    atlas_seen: Option<(egui::epaint::text::TextOptions, f32)>,
    /// The page in the main area (the reader unless a library or study page is open).
    pub pages: crate::pages::Pages,
    /// `QURAN_RS_DEMO`: steps that put the window into a given state (screenshots), one per frame.
    demo: std::collections::VecDeque<String>,
    chrome: Chrome,
    #[cfg(target_os = "macos")]
    toolbar_items: Vec<egui::Rect>,
    #[cfg(target_os = "macos")]
    no_drag: crate::macos_window::NoDragAreas,
    #[cfg(target_os = "macos")]
    menu: Option<crate::macos_menu::AppMenu>,
}

/// Toolbar geometry for the current frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Chrome {
    pub height: f32,
    /// Where the first toolbar control starts when the sidebar is hidden.
    pub leading: f32,
}

/// On macOS the toolbar follows the window's traffic lights: its centre line is theirs and its
/// first control sits a fixed gap after them. Full screen hides them, so the toolbar starts at the
/// window's leading inset.
#[cfg(target_os = "macos")]
fn chrome(frame: &eframe::Frame, ctx: &egui::Context) -> Chrome {
    use theme::metrics::{AFTER_TRAFFIC_LIGHTS, LEADING_INSET, TOOLBAR_HEIGHT};
    let zoom = ctx.zoom_factor();
    match crate::macos_window::traffic_lights(frame) {
        Some(r) => Chrome {
            height: (2.0 * r.center().y / zoom).clamp(28.0, 2.0 * TOOLBAR_HEIGHT),
            leading: r.right() / zoom + AFTER_TRAFFIC_LIGHTS,
        },
        // Rendering offscreen (`examples/shot.rs`): lay out as if AppKit's lights were there.
        None if crate::offscreen() => {
            let lights = theme::titlebar::TRAFFIC_LIGHT_FIRST_CENTER_X
                - theme::titlebar::TRAFFIC_LIGHT_DIAMETER / 2.0
                + 3.0 * theme::titlebar::TRAFFIC_LIGHT_DIAMETER
                + 2.0 * theme::titlebar::TRAFFIC_LIGHT_GAP;
            Chrome { height: TOOLBAR_HEIGHT, leading: lights + AFTER_TRAFFIC_LIGHTS }
        }
        None => Chrome { height: TOOLBAR_HEIGHT, leading: LEADING_INSET },
    }
}

#[cfg(not(target_os = "macos"))]
fn chrome(_frame: &eframe::Frame, _ctx: &egui::Context) -> Chrome {
    Chrome { height: theme::metrics::COMPACT_TOOLBAR_HEIGHT, leading: theme::metrics::LEADING_INSET }
}

/// Whether a translation's language needs the system's fallback font.
fn needs_other_scripts(library: &Library) -> bool {
    const COVERED: [&str; 24] = [
        "English",
        "Urdu",
        "Arabic",
        "Persian",
        "Turkish",
        "Indonesian",
        "Malay",
        "French",
        "German",
        "Spanish",
        "Italian",
        "Dutch",
        "Portuguese",
        "Albanian",
        "Azerbaijani",
        "Bosnian",
        "Czech",
        "Norwegian",
        "Polish",
        "Romanian",
        "Swedish",
        "Swahili",
        "Hausa",
        "Somali",
    ];
    library.entries().iter().any(|e| !COVERED.contains(&e.meta.language.as_str()))
}

impl QuranApp {
    pub fn new(cc: &eframe::CreationContext<'_>, init: Init, startup: Startup) -> Self {
        let Init { data_dir, study, library, settings, elapsed } = init;
        theme::set_accent(settings.accent);
        theme::apply(&cc.egui_ctx, settings.theme);
        if settings.reduce_motion {
            cc.egui_ctx.all_styles_mut(|s| s.animation_time = 0.0);
        }
        let other_scripts = needs_other_scripts(&library);
        if other_scripts {
            cc.egui_ctx.set_fonts(theme::font_definitions_with(true));
        }
        #[cfg(target_os = "macos")]
        crate::macos_window::install_unified_toolbar(cc);
        let reader = Reader::new(settings.position());
        let (task_tx, task_rx) = channel();
        let sidebar = Sidebar::new(settings.sidebar_tab);
        let onboarding = (!settings.onboarded && !crate::probing()).then(|| {
            cc.egui_ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(crate::onboarding::SIZE));
            crate::onboarding::Onboarding::new(&cc.egui_ctx)
        });
        Self {
            data_dir,
            library,
            study,
            reader,
            history: History::default(),
            sidebar,
            sidebar_motion: crate::motion::Spring::new(if settings.sidebar_open { 1.0 } else { 0.0 }),
            sidebar_shown: if settings.sidebar_open { 1.0 } else { 0.0 },
            sidebar_extent: 0.0,
            last_frame_time: 0.0,
            motion_clock: 0.0,
            motion_step: 1.5 / 60.0,
            palette: None,
            settings_ui: None,
            shortcuts_open: false,
            shortcuts_overlay: Default::default(),
            open_more: false,
            more_highlight: None,
            toasts: Vec::new(),
            main_area: egui::Rect::NOTHING,
            task_tx,
            task_rx,
            downloading: Vec::new(),
            catalog: None,
            startup: Startup { init: elapsed, ..startup },
            saved: (serde_json::to_string(&settings).unwrap_or_default(), 0.0),
            warmed: false,
            reading: Default::default(),
            verse_menu: None,
            note_popover: None,
            onboarding,
            share_sheet: None,
            compare: None,
            reminded: None,
            focus: false,
            focus_reveal: false,
            backup_checked: None,
            other_scripts,
            fonts_changed: false,
            atlas_seen: None,
            pages: Default::default(),
            demo: std::env::var("QURAN_RS_DEMO")
                .map(|s| s.split(';').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect())
                .unwrap_or_default(),
            chrome: Chrome { height: theme::metrics::TOOLBAR_HEIGHT, leading: theme::metrics::LEADING_INSET },
            #[cfg(target_os = "macos")]
            toolbar_items: Vec::new(),
            #[cfg(target_os = "macos")]
            no_drag: Default::default(),
            settings,
            #[cfg(target_os = "macos")]
            menu: if crate::offscreen() { None } else { crate::macos_menu::AppMenu::install() },
        }
    }

    /// The translations shown under each ayah: the reading one, then the parallel ones.
    pub fn columns(&self) -> Vec<String> {
        let mut cols = vec![self.settings.translation.clone()];
        if self.settings.parallel_on {
            for id in &self.settings.parallel {
                if cols.len() < 4 && !cols.contains(id) && self.library.contains(id) {
                    cols.push(id.clone());
                }
            }
            if cols.len() == 1
                && let Some(e) = self.library.translations().find(|e| e.meta.id != cols[0])
            {
                cols.push(e.meta.id.clone());
            }
        }
        cols
    }

    pub fn toast(&mut self, ctx: &egui::Context, msg: impl Into<String>) {
        self.toasts.push((msg.into(), ctx.input(|i| i.time)));
    }

    fn here(&self) -> AyahRef {
        self.reader.location
    }

    fn jump(&mut self, to: AyahRef, flash: bool) {
        let here = self.here();
        self.history.push(here);
        let flash = flash && self.settings.flash_jumps && !self.settings.reduce_motion;
        self.reader.jump(to, flash);
    }

    fn poll_tasks(&mut self, ctx: &egui::Context) {
        while let Ok(r) = self.task_rx.try_recv() {
            match r {
                TaskResult::Catalog(r) => {
                    // The catalogue names translations in their own scripts.
                    if r.is_ok() && !self.other_scripts {
                        self.other_scripts = true;
                        ctx.set_fonts(theme::font_definitions_with(true));
                        self.fonts_changed = true;
                    }
                    self.catalog = Some(r);
                }
                TaskResult::Downloaded(id, r) => {
                    self.downloading.retain(|d| *d != id);
                    match r {
                        Ok(t) => {
                            let name = t.meta.name.clone();
                            match self.library.install(*t) {
                                Ok(id) => {
                                    self.toast(ctx, format!("Added {name}"));
                                    if !self.other_scripts && needs_other_scripts(&self.library) {
                                        self.other_scripts = true;
                                        ctx.set_fonts(theme::font_definitions_with(true));
                                        self.fonts_changed = true;
                                    }
                                    self.pages.translations.selected = Some(id);
                                }
                                Err(e) => self.toast(ctx, format!("Couldn't save {name}: {e:#}")),
                            }
                        }
                        Err(e) => self.toast(ctx, format!("Couldn't download {id}: {e}")),
                    }
                }
            }
        }
    }

    /// Lists the catalogue of translations in the background (once).
    pub fn fetch_catalog(&mut self) {
        if self.catalog.is_some() {
            return;
        }
        self.catalog = Some(Err("Loading…".into()));
        let tx = self.task_tx.clone();
        std::thread::spawn(move || {
            let r = quran_core::catalog::fetch_list().map_err(|e| format!("{e:#}"));
            let _ = tx.send(TaskResult::Catalog(r));
        });
    }

    fn shortcuts(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let typing = ctx.egui_wants_keyboard_input();
        // The surah picker takes the single keys (arrows, return, escape) while it's open.
        let picker = crate::surah_picker::is_open(ctx);
        let overlay = self.palette.is_some() || self.settings_ui.is_some() || self.shortcuts_open;
        let bindings = crate::keymap::bindings(&self.settings.shortcuts);
        let anywhere = |a: &Action| matches!(a, Action::OpenPalette | Action::OpenSearch(_));
        ctx.input_mut(|i| {
            let cmd = Modifiers::COMMAND;
            let cmd_shift = Modifiers::COMMAND | Modifiers::SHIFT;
            let mut key = |m: Modifiers, k: Key, a: Action| {
                if i.consume_key(m, k) {
                    actions.push(a);
                }
            };
            for (c, a) in bindings.iter().filter(|(_, a)| anywhere(a)) {
                key(c.modifiers(), c.key, a.clone());
            }
            key(cmd, Key::Comma, Action::OpenSettings(SettingsTab::Reading));
            key(cmd, Key::Slash, Action::ShowShortcuts);
            if overlay {
                return;
            }
            // Command chords (shifted ones first: egui matches shift loosely); copying waits until
            // no field has the keyboard.
            for (c, a) in
                bindings.iter().filter(|(c, a)| c.is_command() && !anywhere(a) && *a != Action::Copy)
            {
                key(c.modifiers(), c.key, a.clone());
            }
            key(cmd_shift, Key::E, Action::Export(ExportKind::Json));
            key(cmd, Key::OpenBracket, Action::Back);
            key(cmd, Key::CloseBracket, Action::Forward);
            key(Modifiers::ALT, Key::ArrowLeft, Action::Back);
            key(Modifiers::ALT, Key::ArrowRight, Action::Forward);
            key(cmd, Key::Plus, Action::FontSize(1.0));
            key(cmd, Key::Equals, Action::FontSize(1.0));
            key(cmd, Key::Minus, Action::FontSize(-1.0));
            key(cmd, Key::Num0, Action::FontReset);
            key(cmd, Key::T, Action::TextFormat);
            key(cmd, Key::D, Action::ToggleBookmark);
            key(cmd, Key::E, Action::Export(ExportKind::Markdown));
            key(cmd, Key::I, Action::ImportTranslation);
            for (n, k) in [
                Key::Num1,
                Key::Num2,
                Key::Num3,
                Key::Num4,
                Key::Num5,
                Key::Num6,
                Key::Num7,
                Key::Num8,
                Key::Num9,
            ]
            .into_iter()
            .enumerate()
            {
                key(cmd, k, Action::TranslationNumber(n));
            }
            if typing {
                return;
            }
            for (c, a) in bindings.iter().filter(|(c, a)| *a == Action::Copy || !(c.is_command() || picker)) {
                key(c.modifiers(), c.key, a.clone());
            }
            key(cmd, Key::C, Action::CopyText);
            if picker {
                return;
            }
            key(Modifiers::NONE, Key::ArrowLeft, Action::PrevSurah);
            key(Modifiers::NONE, Key::ArrowRight, Action::NextSurah);
            key(Modifiers::SHIFT, Key::ArrowLeft, Action::PrevJuz);
            key(Modifiers::SHIFT, Key::ArrowRight, Action::NextJuz);
            key(Modifiers::NONE, Key::ArrowDown, Action::ScrollLines(3.0));
            key(Modifiers::NONE, Key::ArrowUp, Action::ScrollLines(-3.0));
            key(Modifiers::NONE, Key::PageDown, Action::ScrollPages(1.0));
            key(Modifiers::NONE, Key::PageUp, Action::ScrollPages(-1.0));
            key(Modifiers::SHIFT, Key::Space, Action::ScrollPages(-1.0));
            key(Modifiers::NONE, Key::Space, Action::ScrollPages(1.0));
            key(Modifiers::NONE, Key::J, Action::SelectNext);
            key(Modifiers::NONE, Key::K, Action::SelectPrev);
            key(Modifiers::NONE, Key::Escape, Action::ClearSelection);
            key(Modifiers::NONE, Key::Slash, Action::OpenSearch(None));
            key(Modifiers::SHIFT, Key::Questionmark, Action::ShowShortcuts);
            key(Modifiers::NONE, Key::Questionmark, Action::ShowShortcuts);
            for (n, k) in
                [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6].into_iter().enumerate()
            {
                key(Modifiers::NONE, k, Action::Highlight(Some(quran_core::study::HighlightColor::ALL[n])));
            }
            key(Modifiers::NONE, Key::Num0, Action::Highlight(None));
            key(Modifiers::NONE, Key::Backspace, Action::Highlight(None));
        });
    }

    fn apply(&mut self, ctx: &egui::Context, actions: Vec<Action>) {
        for a in actions {
            self.apply_one(ctx, a);
        }
    }

    fn selection_or_location(&self) -> Vec<AyahRef> {
        let sel = self.reader.selected_refs();
        if sel.is_empty() { vec![self.here()] } else { sel }
    }

    fn copy_with(&mut self, ctx: &egui::Context, refs: &[AyahRef], translation: bool) {
        let t = self.library.load(&self.settings.translation).ok();
        let q = quran_core::text::quran();
        let parts = quran_core::copy::CopyParts { arabic: true, translation };
        if let Some(text) =
            quran_core::copy::format_ayahs(|r| q.arabic(r).to_string(), t.as_deref(), refs, parts)
        {
            ctx.copy_text(text);
            self.toast(ctx, if translation { "Copied with translation" } else { "Copied" });
        }
    }

    fn apply_one(&mut self, ctx: &egui::Context, a: Action) {
        match a {
            Action::Jump { to, flash } => {
                self.pages.open(crate::pages::Page::Reader);
                self.jump(to, flash);
                // Landing on a specific ayah selects it, so N/B/C/1–6 act on it right away.
                if flash {
                    self.reader.set_selection(Some(to));
                }
            }
            Action::Back if !self.pages.reader() => self.pages.open(crate::pages::Page::Reader),
            Action::Back => {
                if let Some(to) = self.history.back(self.here()) {
                    self.reader.jump(to, false);
                }
            }
            Action::Forward => {
                self.pages.open(crate::pages::Page::Reader);
                if let Some(to) = self.history.forward(self.here()) {
                    self.reader.jump(to, false);
                }
            }
            Action::PrevSurah => {
                // Within a surah, ← first returns to its start; at its start, to the one before.
                let here = self.here();
                let s = here.surah_id();
                let sec = crate::reader::layout::sections()[self.reader.anchor.section as usize];
                let at_start = here.ayah <= 1 && sec.starts_surah() && self.reader.anchor.offset <= 40.0;
                if let Some(t) = if at_start { s.prev() } else { Some(s) } {
                    self.reader.jump(t.first_ayah(), false);
                }
            }
            Action::NextSurah => {
                if let Some(n) = self.here().surah_id().next() {
                    self.reader.jump(n.first_ayah(), false);
                }
            }
            Action::PrevJuz => {
                let j = self.here().juz();
                let start = Juz(j).start();
                let to = if self.here() > start {
                    start
                } else {
                    Juz::new(j as u32 - 1).map_or(start, |p| p.start())
                };
                self.reader.jump(to, false);
            }
            Action::NextJuz => {
                if let Some(n) = Juz::new(self.here().juz() as u32 + 1) {
                    self.reader.jump(n.start(), false);
                }
            }
            Action::SetTranslation(id) => {
                if self.library.contains(&id) {
                    self.settings.translation = id.clone();
                    self.settings.parallel.retain(|p| *p != id);
                    self.library.warm(&[id]);
                    self.sidebar.invalidate_search();
                }
            }
            Action::TranslationNumber(n) => {
                let id = self.library.translations().nth(n).map(|e| e.meta.id.clone());
                if let Some(id) = id {
                    self.apply_one(ctx, Action::SetTranslation(id));
                }
            }
            Action::ToggleParallel => {
                self.settings.parallel_on = !self.settings.parallel_on;
                if self.settings.parallel_on {
                    let cols = self.columns();
                    self.library.warm(&cols);
                }
            }
            Action::CompareVerse(v) => {
                self.reader.set_selection(Some(v));
                self.apply_one(ctx, Action::Compare);
            }
            Action::AddParallel(id) => {
                if id != self.settings.translation && !self.settings.parallel.contains(&id) {
                    self.settings.parallel.push(id.clone());
                }
                self.settings.parallel_on = true;
                self.library.warm(&[id]);
            }
            Action::RemoveParallel(id) => {
                self.settings.parallel.retain(|p| *p != id);
            }
            Action::Highlight(color) => {
                let refs = self.reader.selected_refs();
                for r in &refs {
                    if let Err(e) = self.study.set_highlight(*r, color) {
                        self.toast(ctx, format!("Couldn't save highlight: {e:#}"));
                        break;
                    }
                }
            }
            Action::EditNote(v) => {
                self.reader.set_selection(Some(v));
                self.note_popover = Some(NotePopover::new(v, &self.study));
            }
            Action::EditSelectedNote => {
                let v = self.selection_or_location()[0];
                self.apply_one(ctx, Action::EditNote(v));
            }
            Action::ToggleBookmark => {
                let v = self.selection_or_location()[0];
                match self.study.toggle_bookmark(v) {
                    Ok(true) => self.toast(ctx, format!("Bookmarked {}", v.long())),
                    Ok(false) => self.toast(ctx, format!("Removed bookmark {}", v.long())),
                    Err(e) => self.toast(ctx, format!("Couldn't save bookmark: {e:#}")),
                }
            }
            Action::RemoveBookmark(v) => {
                if let Err(e) = self.study.set_bookmark(v, None) {
                    self.toast(ctx, format!("{e:#}"));
                }
            }
            Action::RemoveHighlight(v) => {
                if let Err(e) = self.study.set_highlight(v, None) {
                    self.toast(ctx, format!("{e:#}"));
                }
            }
            Action::RemoveNote(v) => {
                if let Err(e) = self.study.set_note(v, "") {
                    self.toast(ctx, format!("{e:#}"));
                }
                if self.note_popover.as_ref().is_some_and(|n| n.verse == v) {
                    self.note_popover = None;
                }
            }
            Action::CopyText => {
                let refs = self.reader.selected_refs();
                if !refs.is_empty() {
                    self.copy_with(ctx, &refs, false);
                }
            }
            Action::Copy => {
                let refs = self.reader.selected_refs();
                if !refs.is_empty() {
                    self.copy_with(ctx, &refs, true);
                }
            }
            Action::OpenVerseMenu(pos) => {
                if !self.reader.selected_refs().is_empty() {
                    self.verse_menu = Some(crate::verse_menu::VerseMenu::new(pos));
                }
            }
            Action::ShareImage => {
                let refs = self.reader.selected_refs();
                if !refs.is_empty() {
                    self.share_sheet = Some(crate::share_sheet::ShareSheet::new(refs));
                }
            }
            Action::Compare => {
                let v = self.selection_or_location()[0];
                self.reader.set_selection(Some(v));
                self.compare = Some(crate::compare_popover::ComparePopover::new(v));
                let ids: Vec<String> = self.library.entries().iter().map(|e| e.meta.id.clone()).collect();
                self.library.warm(&ids);
            }
            Action::SearchStep(d) => self.sidebar.step_result(d),
            Action::ClearSearch => {
                self.sidebar.query.clear();
                self.reader.selection = None;
            }
            Action::ClearSelection => {
                if self.sidebar.searching() {
                    self.apply_one(ctx, Action::ClearSearch);
                } else {
                    self.reader.selection = None;
                }
            }
            Action::SelectNext => self.reader.step_selection(1),
            Action::SelectPrev => self.reader.step_selection(-1),
            Action::OpenPalette => self.palette = Some(CommandPalette::default()),
            Action::OpenSearch(q) => {
                self.settings.sidebar_open = true;
                self.settings.sidebar_tab = SidebarTab::Search;
                self.sidebar.focus_search(q);
            }
            Action::ToggleSidebar => self.settings.sidebar_open = !self.settings.sidebar_open,
            Action::SidebarTab(t) => {
                self.settings.sidebar_open = true;
                self.settings.sidebar_tab = t;
            }
            Action::OpenSettings(tab) => {
                if tab == SettingsTab::Translations {
                    self.fetch_catalog();
                }
                match &mut self.settings_ui {
                    Some(s) => s.reopen(tab),
                    None => self.settings_ui = Some(SettingsUi::new(tab)),
                }
            }
            Action::FontSize(d) => {
                let t = &mut self.settings.typo;
                t.size = (t.size + d).clamp(12.0, 34.0);
                t.arabic_size = (t.arabic_size + d * 1.5).clamp(20.0, 56.0);
            }
            Action::FontReset => {
                let d = Typography::default();
                self.settings.typo.size = d.size;
                self.settings.typo.arabic_size = d.arabic_size;
            }
            Action::SetTheme(t) => {
                self.settings.theme = t;
                theme::apply(ctx, t);
            }
            Action::CycleTheme => {
                let t = self.settings.theme.next();
                self.apply_one(ctx, Action::SetTheme(t));
            }
            Action::ToggleMode => {
                self.settings.typo.mode = if self.settings.typo.mode == Mode::Translation {
                    Mode::Reading
                } else {
                    Mode::Translation
                };
            }
            Action::ToggleTransliteration => {
                self.settings.typo.transliteration = !self.settings.typo.transliteration;
                if self.settings.typo.transliteration {
                    self.settings.typo.mode = Mode::Translation;
                }
            }
            Action::ToggleTranslation => {
                self.settings.typo.show_translation = !self.settings.typo.show_translation;
                if self.settings.typo.show_translation {
                    self.settings.typo.mode = Mode::Translation;
                }
            }
            Action::Export(kind) => self.export(ctx, kind),
            Action::ImportStudy => self.import_study(ctx),
            Action::ImportTranslation => self.import_translation(ctx),
            Action::DownloadTranslation(id) => {
                if !self.downloading.contains(&id) && !self.library.contains(&id) {
                    self.downloading.push(id.clone());
                    let tx = self.task_tx.clone();
                    let ctx2 = ctx.clone();
                    std::thread::spawn(move || {
                        let r =
                            quran_core::catalog::download(&id).map(Box::new).map_err(|e| format!("{e:#}"));
                        let _ = tx.send(TaskResult::Downloaded(id, r));
                        ctx2.request_repaint();
                    });
                }
            }
            Action::RemoveTranslation(id) => match self.library.remove(&id) {
                Ok(()) => {
                    if self.settings.translation == id {
                        self.settings.translation = DEFAULT_ID.into();
                    }
                    self.settings.parallel.retain(|x| *x != id);
                    if self.pages.translations.selected.as_deref() == Some(id.as_str()) {
                        self.pages.translations.selected = None;
                    }
                    self.reader.invalidate();
                    self.toast(ctx, format!("Removed {id}"));
                }
                Err(e) => self.toast(ctx, format!("{e:#}")),
            },
            Action::OpenPage(page) => {
                if page == crate::pages::Page::Translations {
                    self.fetch_catalog();
                }
                self.pages.open(page)
            }
            Action::SaveNote(v, body) => {
                if let Err(e) = self.study.set_note(v, &body) {
                    self.toast(ctx, format!("Couldn't save note: {e:#}"));
                }
            }
            Action::StartPlan(id) => {
                let today = crate::time::day_start(quran_core::study::now());
                if let Err(e) = self.study.start_plan(&id, today) {
                    self.toast(ctx, format!("{e:#}"));
                }
            }
            Action::StopPlan(id) => {
                if let Err(e) = self.study.stop_plan(&id) {
                    self.toast(ctx, format!("{e:#}"));
                }
            }
            Action::SetPlanRead { plan, day, passage, read } => {
                if let Err(e) = self.study.set_plan_read(&plan, day, passage, read) {
                    self.toast(ctx, format!("{e:#}"));
                }
            }
            Action::SetPlanReminder(on) => self.settings.plan_reminder = on,
            Action::EraseStudy => {
                // A backup first, so it can be undone.
                match self
                    .backup_now()
                    .map_err(|e| format!("{e:#}"))
                    .and_then(|path| self.study.erase_all().map(|_| path).map_err(|e| format!("{e:#}")))
                {
                    Ok(path) => {
                        self.reader.invalidate();
                        self.toast(ctx, format!("Erased. A backup was saved first: {}", path.display()));
                    }
                    Err(e) => self.toast(ctx, format!("Couldn't erase: {e}")),
                }
            }
            Action::RevealDataDir => reveal(&self.data_dir),
            Action::TextFormat => egui::Popup::open_id(ctx, Id::new(TEXT_POPUP)),
            Action::ToggleFocus => {
                self.focus = !self.focus;
                if self.focus {
                    self.pages.open(crate::pages::Page::Reader);
                }
                if !crate::offscreen() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.focus));
                }
            }
            Action::SetAccent(a) => {
                self.settings.accent = a;
                theme::set_accent(a);
                theme::apply(ctx, self.settings.theme);
            }
            Action::SetReduceMotion(on) => {
                self.settings.reduce_motion = on;
                let t = if on { 0.0 } else { egui::Style::default().animation_time };
                ctx.all_styles_mut(|s| s.animation_time = t);
            }
            Action::ClearReading(since) => {
                if let Err(e) = self.study.clear_reading(since) {
                    self.toast(ctx, format!("{e:#}"));
                }
            }
            Action::ToggleBookmarkAt(v) => {
                if let Err(e) = self.study.toggle_bookmark(v) {
                    self.toast(ctx, format!("{e:#}"));
                }
            }
            Action::ShowShortcuts => self.shortcuts_open = !self.shortcuts_open,
            Action::ScrollLines(n) => {
                let lh = self.settings.typo.size * self.settings.typo.line_height;
                self.reader.scroll_by(n * lh);
            }
            Action::ScrollPages(n) => {
                let page = self.reader.page();
                self.reader.scroll_by(n * page);
            }
            Action::Toast(m) => self.toast(ctx, m),
            Action::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
        }
    }

    /// One `QURAN_RS_DEMO` step: `jump:<reference>`, `select:<reference>`, `palette:<query>`,
    /// `search:<query>`, `note:<reference>`, `compare:<reference>`, `study:<kind>`,
    /// `settings:<tab>`, `page:<name>`, `mode:reading`, `theme:<light|sepia|dark>`, `parallel`,
    /// `translit`, `focus`, `shortcuts`, `share`, `menu:<reference>@x,y`, `size:<w>x<h>`,
    /// `tab:<surahs|juz|search|study>`, `onboarding:<step>`, `plans-tab:<tab>`, `plan-day:<id>@<day>`.
    fn demo_step(&mut self, ctx: &egui::Context, step: &str, actions: &mut Vec<Action>) {
        let (cmd, arg) = step.split_once(':').unwrap_or((step, ""));
        let ayah = || quran_core::reference::parse(arg).map(|t| t.start());
        match cmd {
            "jump" => {
                if let Some(t) = quran_core::reference::parse(arg) {
                    actions.push(Action::Jump { to: t.start(), flash: t.is_ayah() });
                }
            }
            "select" => self.reader.set_selection(ayah()),
            "scroll" => self.reader.anchor.offset += arg.parse::<f32>().unwrap_or(0.0),
            "palette" => self.palette = Some(CommandPalette::with_query(arg)),
            "search" => actions.push(Action::OpenSearch(Some(arg.to_string()))),
            "step" => actions.push(Action::SearchStep(arg.parse().unwrap_or(1))),
            "note" => actions.extend(ayah().map(Action::EditNote)),
            "compare" => {
                self.reader.set_selection(ayah());
                actions.push(Action::Compare);
            }
            "study" => self.sidebar.open_study(arg),
            "tab" => actions.push(Action::SidebarTab(match arg {
                "juz" => SidebarTab::Juz,
                "search" => SidebarTab::Search,
                "study" => SidebarTab::Study,
                _ => SidebarTab::Surahs,
            })),
            "mode" => {
                self.settings.typo.mode = if arg == "reading" { Mode::Reading } else { Mode::Translation };
            }
            "theme" => actions.push(Action::SetTheme(match arg {
                "dark" => Theme::Dark,
                "sepia" => Theme::Sepia,
                _ => Theme::Light,
            })),
            "parallel" => actions.push(Action::ToggleParallel),
            "translation" => actions.push(Action::SetTranslation(arg.to_string())),
            "picker" => egui::Popup::open_id(ctx, Id::new("surah-picker")),
            "translit" => actions.push(Action::ToggleTransliteration),
            "more" => {
                self.open_more = true;
                self.more_highlight = arg.parse().ok();
            }
            "menu" => {
                let (r, at) = arg.split_once('@').unwrap_or((arg, "600,352"));
                let (x, y) = at.split_once(',').unwrap_or(("600", "352"));
                self.reader.set_selection(quran_core::reference::parse(r).map(|t| t.start()));
                let pos = egui::pos2(x.parse().unwrap_or(600.0), y.parse().unwrap_or(352.0));
                self.verse_menu = Some(crate::verse_menu::VerseMenu::new(pos).with_first_highlighted());
            }
            "share" => actions.push(Action::ShareImage),
            "size" => {
                if let Some((w, h)) = arg.split_once('x')
                    && let (Ok(w), Ok(h)) = (w.parse::<f32>(), h.parse::<f32>())
                {
                    ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(w, h)));
                }
            }
            "focus" => actions.push(Action::ToggleFocus),
            "reveal" => self.focus_reveal = true,
            "shortcuts" => self.shortcuts_open = true,
            "page" => {
                if let Some(page) = crate::pages::Page::from_name(arg) {
                    actions.push(Action::OpenPage(page));
                }
            }
            "pick" => self.pages.translations.selected = Some(arg.to_string()),
            "plans-tab" => self.pages.plans.set_tab(arg),
            "plan-day" => {
                let (id, day) = arg.split_once('@').unwrap_or((arg, "1"));
                self.pages.plans.open = Some((id.to_string(), day.parse().unwrap_or(1)));
            }
            "note-select" => {
                if let Some(r) = ayah() {
                    self.pages.notes.select(r);
                }
            }
            "onboarding" => {
                if let Some(o) = &mut self.onboarding {
                    o.step = arg.parse::<usize>().unwrap_or(1).saturating_sub(1);
                }
            }
            "record" => {
                if let Some(s) = &mut self.settings_ui {
                    s.record(arg);
                }
            }
            "settings" => actions.push(Action::OpenSettings(match arg {
                "translations" => SettingsTab::Translations,
                "data" => SettingsTab::Data,
                "appearance" => SettingsTab::Appearance,
                "shortcuts" => SettingsTab::Shortcuts,
                "about" => SettingsTab::About,
                _ => SettingsTab::Reading,
            })),
            _ => eprintln!("QURAN_RS_DEMO: unknown step {step:?}"),
        }
    }

    fn export(&mut self, ctx: &egui::Context, kind: ExportKind) {
        let (ext, label) = match kind {
            ExportKind::Json => ("json", "JSON"),
            ExportKind::Markdown => ("md", "Markdown"),
        };
        let Some(path) = rfd::FileDialog::new()
            .set_title(format!("Export highlights, notes and bookmarks as {label}"))
            .set_file_name(format!("quran-notes.{ext}"))
            .save_file()
        else {
            return;
        };
        match self.export_to(&path, kind) {
            Ok(()) => self.toast(ctx, format!("Exported to {}", path.display())),
            Err(e) => self.toast(ctx, format!("Export failed: {e:#}")),
        }
    }

    pub fn export_to(&self, path: &std::path::Path, kind: ExportKind) -> Result<()> {
        let [highlights, notes, bookmarks, history] = self.settings.export_include;
        let include = quran_core::study::Include { highlights, notes, bookmarks, history };
        self.write_export(path, kind, include)
    }

    fn write_export(
        &self,
        path: &std::path::Path,
        kind: ExportKind,
        include: quran_core::study::Include,
    ) -> Result<()> {
        let t = self.library.load(&self.settings.translation)?;
        let text = |r: AyahRef| Some(t.get(r).to_string());
        let abbrev = t.meta.abbrev.clone();
        let body = match kind {
            ExportKind::Json => self.study.export_json_with(&abbrev, text, include)?,
            ExportKind::Markdown => self.study.export_markdown_with(&abbrev, text, include),
        };
        std::fs::write(path, body)?;
        Ok(())
    }

    /// Writes a full JSON backup into the data folder's `backups`, keeping the newest ten.
    pub fn backup_now(&self) -> Result<PathBuf> {
        let dir = backups_dir(&self.data_dir);
        std::fs::create_dir_all(&dir)?;
        let stamp = quran_core::study::rfc3339(quran_core::study::now()).replace(':', "-");
        let path = dir.join(format!("quran-rs-backup-{stamp}.json"));
        self.write_export(&path, ExportKind::Json, quran_core::study::Include::EVERYTHING)?;
        let mut files: Vec<_> = std::fs::read_dir(&dir)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .collect();
        files.sort();
        for old in files.iter().rev().skip(10) {
            let _ = std::fs::remove_file(old);
        }
        Ok(path)
    }

    /// An automatic backup when one is due (checked once a launch and then every hour).
    fn auto_backup(&mut self, ctx: &egui::Context) {
        if !self.settings.backup_auto || self.startup.frames < 10 {
            return;
        }
        let now = ctx.input(|i| i.time);
        if self.backup_checked.is_some_and(|t| now - t < 3600.0) {
            return;
        }
        self.backup_checked = Some(now);
        let every = if self.settings.backup_weekly { 7 * 86_400 } else { 86_400 };
        let due = last_backup(&self.data_dir).is_none_or(|(at, _)| quran_core::study::now() - at >= every);
        let (h, n, b) = self.study.counts();
        if due && h + n + b > 0 {
            let _ = self.backup_now();
        }
    }

    fn import_study(&mut self, ctx: &egui::Context) {
        let Some(path) = rfd::FileDialog::new().add_filter("Quran notes export", &["json"]).pick_file()
        else {
            return;
        };
        let r = std::fs::read_to_string(&path)
            .map_err(anyhow::Error::from)
            .and_then(|s| self.study.import_json(&s));
        match r {
            Ok((h, n, b)) => self.toast(ctx, format!("Imported {h} highlights, {n} notes, {b} bookmarks")),
            Err(e) => self.toast(ctx, format!("Import failed: {e:#}")),
        }
    }

    /// Imports a translation from a text file: Tanzil's `sura|aya|text` lines or 6,236 lines.
    fn import_translation(&mut self, ctx: &egui::Context) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Import a translation (Tanzil text: sura|aya|text, or one line per ayah)")
            .add_filter("Text", &["txt", "text", "csv", "tsv"])
            .pick_file()
        else {
            return;
        };
        match crate::cli::import_file(&path, None, None) {
            Ok(t) => {
                let name = t.meta.name.clone();
                match self.library.install(t) {
                    Ok(id) => {
                        self.toast(ctx, format!("Imported {name}"));
                        self.settings.translation = id.clone();
                        self.pages.translations.selected = Some(id);
                        self.reader.invalidate();
                    }
                    Err(e) => self.toast(ctx, format!("Couldn't save the import: {e:#}")),
                }
            }
            Err(e) => self.toast(ctx, format!("Import failed: {e:#}")),
        }
    }

    /// The daily plan reminder: once a day from 7:00, while a plan has today's reading to do.
    fn plan_reminder(&mut self, ctx: &egui::Context) {
        if !self.settings.plan_reminder || self.startup.frames < 3 {
            return;
        }
        let now = quran_core::study::now();
        let today = crate::time::day_start(now);
        if self.reminded == Some(today) || now - today < 7 * 3600 {
            return;
        }
        self.reminded = Some(today);
        let due = self.study.plans().into_iter().filter(|p| p.finished.is_none()).find_map(|p| {
            let plan = quran_core::plans::get(&p.id)?;
            let day = ((today - p.started) / 86_400 + 1) as usize;
            let reads = self.study.plan_reads(&p.id);
            let todo = (0..plan.day(day).len()).any(|i| !reads.contains(&(day as u16, i as u8)));
            todo.then(|| format!("Today's reading: {}", quran_core::plans::label(plan.day(day))))
        });
        if let Some(msg) = due {
            self.toast(ctx, msg);
        }
    }

    /// Saves settings (including the reading position) when they change, at most every 1.5 s
    /// so scrolling never waits on the database.
    fn persist(&mut self, ctx: &egui::Context, force: bool) {
        let now = ctx.input(|i| i.time);
        self.settings.surah = self.reader.location.surah;
        self.settings.ayah = self.reader.location.ayah;
        let json = serde_json::to_string(&self.settings).unwrap_or_default();
        if json == self.saved.0 {
            return;
        }
        if force || now - self.saved.1 > 1.5 {
            let _ = self.study.set_setting("app", &json);
            self.saved = (json, now);
        } else {
            ctx.request_repaint_after(Duration::from_millis(1600));
        }
    }

    /// How far out the sidebar is this frame (0 hidden, 1 shown), moving it toward where the
    /// settings and focus mode want it and repainting until it gets there. Reduce Motion (the
    /// app's setting or the system's) and offscreen rendering show and hide it at once.
    fn sidebar_open_fraction(&mut self, ctx: &egui::Context, frame: &eframe::Frame) -> f32 {
        use theme::metrics::{SIDEBAR_INSET, SIDEBAR_WIDTH};
        let width = if self.sidebar.searching() { SEARCH_SIDEBAR_WIDTH } else { SIDEBAR_WIDTH };
        let inset = if self.settings.sidebar_attached { 0.0 } else { SIDEBAR_INSET };
        self.sidebar_extent = inset + width;
        let target = if self.settings.sidebar_open && !self.focus { 1.0 } else { 0.0 };
        let changed = target != self.sidebar_motion.target();
        let instant = changed
            && (self.settings.reduce_motion
                || crate::offscreen_still()
                || crate::motion::system_reduces_motion());
        if changed && !instant {
            #[cfg(target_os = "macos")]
            let refresh = crate::macos_window::refresh_interval(frame);
            #[cfg(not(target_os = "macos"))]
            let refresh: Option<f64> = {
                let _ = frame;
                None
            };
            self.motion_step = 1.5 * refresh.unwrap_or(1.0 / 60.0);
        }
        let now = ctx.input(|i| i.time);
        let since = self.motion_clock;
        self.motion_clock += (now - self.last_frame_time).clamp(0.0, self.motion_step);
        self.last_frame_time = now;
        let open = self.sidebar_motion.update(target, since, self.motion_clock, instant);
        if self.sidebar_motion.moving() {
            ctx.request_repaint();
        }
        self.sidebar_shown = open;
        open
    }

    /// The toolbar over the reading area: back/forward (after the sidebar toggle when the sidebar
    /// is hidden), the surah and translation pickers centred, view tools and the search field at
    /// the end. Every control is a glass capsule centred on the traffic lights' line; once text
    /// scrolls beneath, the bar takes the toolbar material.
    fn toolbar(&mut self, ui: &mut Ui, main: egui::Rect, material: f32, actions: &mut Vec<Action>) {
        use theme::metrics::{
            BUTTON_WIDTH, CAPSULE_HEIGHT, GROUP_GAP, LEADING_INSET, SEARCH_FIELD_WIDTH,
            SEARCH_FIELD_WIDTH_COMPACT, TRAILING_INSET,
        };
        use widgets::CapsuleButton;
        let p = self.settings.theme.palette();
        let bar = egui::Rect::from_min_size(main.min, vec2(main.width(), self.chrome.height));
        if material > 0.0 {
            ui.painter().rect_filled(bar, 0.0, p.toolbar.gamma_multiply(material));
            ui.painter().hline(
                bar.x_range(),
                bar.bottom() - 0.5,
                Stroke::new(1.0, p.separator.gamma_multiply(material)),
            );
        }
        let cy = bar.center().y;
        let mut items = Vec::new();

        // Leading: the sidebar toggle, then back and forward.
        let open = self.sidebar_shown;
        let extent = self.sidebar_extent;
        let window_left = bar.left() - extent * open;
        let docked = self.chrome.leading.max(window_left + LEADING_INSET);
        let lead = Leading::new(open, extent, window_left, docked);
        let target_open = self.sidebar_motion.target() > 0.5;
        items.push(sidebar_toggle(ui, p, lead.toggle_x, cy, lead.capsule, target_open, actions));
        let x = lead.nav_x;
        let nav = [
            CapsuleButton {
                tooltip: "Back (⌘[)",
                enabled: self.history.can_back() || !self.pages.reader(),
                on: false,
                icon: crate::icons::chevron_left,
            },
            CapsuleButton {
                tooltip: "Forward (⌘])",
                enabled: self.history.can_forward() && self.pages.reader(),
                on: false,
                icon: crate::icons::chevron_right,
            },
        ];
        let (nav_rect, resp) = widgets::button_capsule(ui, p, x, cy, BUTTON_WIDTH, true, &nav);
        if resp[0].clicked() {
            actions.push(Action::Back);
        }
        if resp[1].clicked() {
            actions.push(Action::Forward);
        }
        items.push(nav_rect);

        // On a page: a wider field that filters it, and nothing else.
        if !self.pages.reader() {
            let w = PAGE_SEARCH_WIDTH;
            let search = egui::Rect::from_min_size(
                egui::pos2(bar.right() - TRAILING_INSET - w, cy - CAPSULE_HEIGHT / 2.0),
                vec2(w, CAPSULE_HEIGHT),
            );
            let placeholder = self.pages.page.placeholder();
            let (field, cleared) = widgets::search_field(
                ui,
                search,
                p,
                &mut self.pages.query,
                placeholder,
                Id::new("page-search"),
            );
            if cleared || (field.has_focus() && ui.input(|i| i.key_pressed(Key::Escape))) {
                self.pages.query.clear();
                field.surrender_focus();
            }
            items.push(search);
            #[cfg(target_os = "macos")]
            self.toolbar_items.extend(items);
            return;
        }

        // Trailing: view tools, then the search field. A compact window folds both into one
        // capsule, Search and More, and shows the narrower field while searching.
        let compact = bar.width() + extent * (open - self.sidebar_motion.target()) < COMPACT_TOOLBAR;
        let focus_field = self.sidebar.take_focus();
        let show_field = !compact || focus_field || self.sidebar.searching() || self.sidebar.field_focused;
        let trailing_left;
        let mut text_anchor: Option<(egui::Response, bool)> = None;
        if show_field {
            let w = if compact { SEARCH_FIELD_WIDTH_COMPACT } else { SEARCH_FIELD_WIDTH };
            let search = egui::Rect::from_min_size(
                egui::pos2(bar.right() - TRAILING_INSET - w, cy - CAPSULE_HEIGHT / 2.0),
                vec2(w, CAPSULE_HEIGHT),
            );
            trailing_left = if compact {
                search.left()
            } else {
                let reading = self.settings.typo.mode == Mode::Reading;
                let tools = [
                    CapsuleButton {
                        tooltip: "Reading view: the Arabic alone, like a mushaf (⌘L)",
                        enabled: true,
                        on: reading,
                        icon: crate::icons::book,
                    },
                    CapsuleButton {
                        tooltip: "More translations under each ayah (⌘⇧P)",
                        enabled: true,
                        on: self.settings.parallel_on && !reading,
                        icon: crate::icons::columns,
                    },
                    CapsuleButton {
                        tooltip: "Text & layout (⌘T)",
                        enabled: true,
                        on: false,
                        icon: crate::icons::text_size,
                    },
                ];
                let tools_left = search.left() - GROUP_GAP - 3.0 * BUTTON_WIDTH;
                let (tools_rect, resp) =
                    widgets::button_capsule(ui, p, tools_left, cy, BUTTON_WIDTH, false, &tools);
                if resp[0].clicked() {
                    actions.push(Action::ToggleMode);
                }
                if resp[1].clicked() {
                    actions.push(Action::ToggleParallel);
                }
                text_anchor = Some((resp[2].clone(), true));
                items.push(tools_rect);
                tools_rect.left()
            };
            let field_id = Id::new("toolbar-search");
            let (field, cleared) =
                widgets::search_field(ui, search, p, &mut self.sidebar.query, "Search", field_id);
            if focus_field {
                field.request_focus();
            }
            if field.changed() && self.sidebar.searching() {
                self.settings.sidebar_open = true;
                self.settings.sidebar_tab = SidebarTab::Search;
            }
            if cleared || (field.has_focus() && ui.input(|i| i.key_pressed(Key::Escape))) {
                self.sidebar.query.clear();
                field.surrender_focus();
            }
            self.sidebar.field_focused = field.has_focus();
            items.push(search);
        } else {
            let buttons = [
                CapsuleButton {
                    tooltip: "Search (⌘F)",
                    enabled: true,
                    on: false,
                    icon: crate::icons::search_thin,
                },
                CapsuleButton { tooltip: "More", enabled: true, on: false, icon: crate::icons::ellipsis },
            ];
            let left = bar.right() - TRAILING_INSET - 2.0 * BUTTON_WIDTH;
            let (r, resp) = widgets::button_capsule(ui, p, left, cy, BUTTON_WIDTH, false, &buttons);
            if resp[0].clicked() {
                actions.push(Action::OpenSearch(None));
            }
            if egui::Popup::is_id_open(ui.ctx(), Id::new(MORE_POPUP)) {
                let b = resp[1].rect;
                ui.painter().rect_filled(b, CAPSULE_HEIGHT / 2.0, p.fill);
                let icon = egui::Rect::from_center_size(b.center(), vec2(20.0, 20.0));
                crate::icons::ellipsis(ui.painter(), icon, p.ink);
            }
            self.more_menu(ui, &resp[1], actions);
            text_anchor = Some((resp[1].clone(), false));
            trailing_left = r.left();
            items.push(r);
        }
        if let Some((anchor, toggles)) = text_anchor {
            let open = (toggles && anchor.clicked()).then_some(egui::SetOpenCommand::Toggle);
            egui::Popup::menu(&anchor)
                .id(Id::new(TEXT_POPUP))
                .open_memory(open)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .show(|ui| {
                    ui.set_min_width(260.0);
                    crate::settings_ui::quick_text_settings(ui, &mut self.settings, actions);
                });
        }

        // Principal: the surah (and juz) and the translation in one capsule, centred between the
        // bar's paddings unless that would collide with the groups at either end.
        let here = self.reader.location;
        let s = here.surah_id();
        let title = if compact {
            format!("{} · {}", s.0, s.name())
        } else {
            format!("{} · {}  ·  Juz {}", s.0, s.name(), here.juz())
        };
        let tmeta = self.library.get(&self.settings.translation).map(|e| e.meta.clone());
        let abbrev = if self.settings.typo.mode == Mode::Reading {
            "Arabic".to_string()
        } else if self.settings.parallel_on {
            self.columns()
                .iter()
                .filter_map(|id| self.library.get(id).map(|e| e.meta.abbrev.clone()))
                .collect::<Vec<_>>()
                .join(" · ")
        } else {
            tmeta.as_ref().map(|m| m.abbrev.clone()).unwrap_or_default()
        };
        let show_translation = self.settings.toolbar_translation;
        let book = widgets::Picker::new(ui, &title, 14.0, if show_translation { 10.0 } else { 12.0 });
        let translation = widgets::Picker::new(ui, &abbrev, 10.0, 12.0);
        let width = book.width() + if show_translation { 1.0 + translation.width() } else { 0.0 };
        let pad_left = LEADING_INSET + (GROUP_GAP - LEADING_INSET) * open;
        let centre = (bar.left() + pad_left + bar.right() - TRAILING_INSET) / 2.0;
        let lo = nav_rect.right() + GROUP_GAP;
        let hi = trailing_left - GROUP_GAP - width;
        if hi < lo {
            #[cfg(target_os = "macos")]
            self.toolbar_items.extend(items);
            return;
        }
        let left = (centre - width / 2.0).min(hi).max(lo).round();
        let pickers = egui::Rect::from_min_size(
            egui::pos2(left, cy - CAPSULE_HEIGHT / 2.0),
            vec2(width, CAPSULE_HEIGHT),
        );
        widgets::capsule(ui.painter(), pickers, p);
        let book_rect = egui::Rect::from_min_size(pickers.min, vec2(book.width(), CAPSULE_HEIGHT));
        let tr_rect = egui::Rect::from_min_size(
            egui::pos2(book_rect.right() + 1.0, pickers.top()),
            vec2(translation.width(), CAPSULE_HEIGHT),
        );
        let loc = book.show(ui, book_rect, p, "surah", "Go to a surah or juz (⌘K)");
        crate::surah_picker::show(&loc, p, here, actions);
        if show_translation {
            widgets::capsule_divider(ui.painter(), book_rect.right(), cy, p);
            let tr = translation.show(ui, tr_rect, p, "translation", "Translation (⌘1…⌘9)");
            crate::menu::popup(&tr, p).show(|ui| {
                ui.set_min_width(300.0);
                translation_menu(ui, &self.library, &self.settings, actions);
            });
        }
        items.push(pickers);

        #[cfg(target_os = "macos")]
        {
            self.toolbar_items.extend(items);
        }
        #[cfg(not(target_os = "macos"))]
        let _ = items;
    }

    /// Focus mode's chrome: a reveal bar while the pointer is near the top (Exit Focus, text
    /// options, the shortcut), and a reading-progress hairline with what's left of the surah.
    fn focus_chrome(&mut self, ui: &mut Ui, full: egui::Rect, actions: &mut Vec<Action>) {
        let p = self.settings.theme.palette();
        let ctx = ui.ctx().clone();
        let near = self.focus_reveal
            || ctx.input(|i| i.pointer.hover_pos().is_some_and(|q| q.y < full.top() + 64.0));
        let t = ctx.animate_bool_with_time(Id::new("focus-reveal"), near, theme::motion::BASE_MS / 1000.0);
        if t > 0.0 {
            let chord = crate::keymap::ALL
                .iter()
                .find(|s| s.id == "focus")
                .map(|s| crate::keymap::keycaps(s, &self.settings.shortcuts).concat())
                .unwrap_or_default();
            let hint = format!("Exit Focus {chord}");
            let hint_w = ui
                .painter()
                .layout_no_wrap(hint.clone(), theme::ui_medium(12.0), egui::Color32::PLACEHOLDER)
                .size()
                .x;
            let width = 73.0 + 10.0 + hint_w;
            let left = (full.center().x - width / 2.0).round();
            let mut bar = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(egui::Rect::from_min_size(egui::pos2(left, full.top()), vec2(width, 52.0))),
            );
            bar.set_opacity(0.9 * t);
            let buttons = [
                widgets::CapsuleButton {
                    tooltip: "Exit Focus",
                    enabled: true,
                    on: false,
                    icon: crate::icons::contract,
                },
                widgets::CapsuleButton {
                    tooltip: "Text & theme",
                    enabled: true,
                    on: false,
                    icon: crate::icons::text_size,
                },
            ];
            let (capsule, resp) =
                widgets::button_capsule(&mut bar, p, left, full.top() + 26.0, 36.0, true, &buttons);
            bar.painter().text(
                egui::pos2(capsule.right() + 10.0, full.top() + 26.0),
                Align2::LEFT_CENTER,
                hint,
                theme::ui_medium(12.0),
                p.ink_3,
            );
            if resp[0].clicked() {
                actions.push(Action::ToggleFocus);
            }
            egui::Popup::menu(&resp[1]).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(
                |ui| {
                    ui.set_min_width(260.0);
                    crate::settings_ui::quick_text_settings(ui, &mut self.settings, actions);
                },
            );
        }

        // Progress through the surah.
        let s = self.reader.location.surah_id();
        let total = s.ayahs() as usize;
        let band = egui::Rect::from_min_max(egui::pos2(full.left(), full.bottom() - 30.0), full.max);
        ui.painter().rect_filled(band, 0.0, p.paper);
        let read = (self.reader.furthest_in_view as usize).min(total);
        // About 130 words of Arabic recitation or 230 of reading a minute; reading time follows
        // the translation's words.
        let words: usize = self
            .library
            .loaded(&self.settings.translation)
            .map(|t| {
                (read + 1..=total)
                    .map(|a| t.get(AyahRef::new(s.0, a as u16)).split_whitespace().count())
                    .sum()
            })
            .unwrap_or(0);
        let track =
            egui::Rect::from_min_size(egui::pos2(full.left(), full.bottom() - 30.0), vec2(full.width(), 2.0));
        ui.painter().rect_filled(track, 0.0, p.fill);
        let done =
            egui::Rect::from_min_size(track.min, vec2(full.width() * read as f32 / total.max(1) as f32, 2.0));
        ui.painter().rect_filled(done, 0.0, p.accent.gamma_multiply(0.5));
        let left = if read >= total {
            "the end of the surah".to_string()
        } else {
            format!("about {} min left", words.div_ceil(230).max(1))
        };
        ui.painter().text(
            egui::pos2(full.center().x, full.bottom() - 14.0),
            Align2::CENTER_CENTER,
            format!("{} · {read} of {total} ayahs · {left}", s.name()),
            theme::ui_medium(11.0),
            p.ink_3,
        );
    }

    /// While a search has results: "Result 5 of 7", a stepper and a hint, floating over the top
    /// of the text.
    fn result_navigator(&mut self, ui: &mut Ui, reader_rect: egui::Rect, actions: &mut Vec<Action>) {
        let Some((at, of)) = self.sidebar.result_position() else { return };
        let p = self.settings.theme.palette();
        let painter = ui.painter();
        let font = theme::ui_medium(theme::text::SMALL.size);
        let label = painter.layout_no_wrap(format!("Result {at} of {of}"), font, p.ink_2);
        let hint = painter.layout_no_wrap("Esc to clear".into(), theme::text::SMALL.font(), p.ink_3);
        let w = 14.0 + label.size().x + 8.0 + 24.0 + 8.0 + 1.0 + 8.0 + hint.size().x + 8.0;
        let top = reader_rect.top() + self.chrome.height + crate::reader::layout::PAGE_TOP;
        let r = egui::Rect::from_min_size(
            egui::pos2((reader_rect.center().x - w / 2.0).round(), top),
            vec2(w, 32.0),
        );
        widgets::capsule(painter, r, p);
        let cy = r.center().y;
        let mut x = r.left() + 14.0;
        let lw = label.size().x;
        painter.galley(egui::pos2(x, cy - label.size().y / 2.0), label, p.ink_2);
        x += lw + 8.0;
        let b = egui::Rect::from_min_size(egui::pos2(x, cy - 12.0), vec2(24.0, 24.0));
        let resp = ui.interact(b, Id::new("result-step"), Sense::click());
        if let Some(fill) = widgets::hover_fill(ui, &resp, p, false) {
            ui.painter().circle_filled(b.center(), 12.0, fill);
        }
        crate::icons::heavy(|| {
            crate::icons::chevrons_up_down(
                ui.painter(),
                egui::Rect::from_center_size(b.center(), vec2(14.0, 14.0)),
                p.ink_2,
            )
        });
        if resp.clicked() {
            let up = resp.interact_pointer_pos().is_some_and(|pt| pt.y < b.center().y);
            actions.push(Action::SearchStep(if up { -1 } else { 1 }));
        }
        let _ = resp.on_hover_text("Previous / next result (↑ ↓ in the search field)");
        x = b.right() + 8.0;
        widgets::capsule_divider(ui.painter(), x, cy, p);
        x += 1.0 + 8.0;
        ui.painter().galley(egui::pos2(x, cy - hint.size().y / 2.0), hint, p.ink_3);
        #[cfg(target_os = "macos")]
        self.toolbar_items.push(r);
    }

    /// The compact toolbar's More menu: the view tools, the sidebar and focus mode, with their
    /// shortcuts as currently bound.
    fn more_menu(&mut self, ui: &mut Ui, more: &egui::Response, actions: &mut Vec<Action>) {
        let p = self.settings.theme.palette();
        let chord = |id: &str| {
            crate::keymap::ALL
                .iter()
                .find(|s| s.id == id)
                .map(|s| crate::keymap::keycaps(s, &self.settings.shortcuts).concat())
                .unwrap_or_default()
        };
        let sidebar = if self.settings.sidebar_open { "Hide Sidebar" } else { "Show Sidebar" };
        let mode = if self.settings.typo.mode == Mode::Reading { "Translation View" } else { "Reading View" };
        let items: [Option<(widgets::IconFn, &str, String, Action)>; 6] = [
            Some((crate::icons::book, mode, chord("mode"), Action::ToggleMode)),
            Some((crate::icons::columns, "More Translations", chord("parallel"), Action::ToggleParallel)),
            Some((crate::icons::text_size, "Text & Layout", "⌘T".into(), Action::TextFormat)),
            None,
            Some((crate::icons::sidebar, sidebar, chord("sidebar"), Action::ToggleSidebar)),
            Some((crate::icons::expand, "Focus Mode", chord("focus"), Action::ToggleFocus)),
        ];
        let id = Id::new(MORE_POPUP);
        if std::mem::take(&mut self.open_more) {
            egui::Popup::open_id(ui.ctx(), id);
        }
        if egui::Popup::is_id_open(ui.ctx(), id) {
            let enabled: Vec<usize> = (0..items.len()).filter(|i| items[*i].is_some()).collect();
            let (down, up, enter) = ui.input_mut(|i| {
                (
                    i.consume_key(Modifiers::NONE, Key::ArrowDown),
                    i.consume_key(Modifiers::NONE, Key::ArrowUp),
                    i.consume_key(Modifiers::NONE, Key::Enter),
                )
            });
            let at = self.more_highlight.and_then(|h| enabled.iter().position(|i| *i == h));
            let n = enabled.len();
            if down {
                self.more_highlight = Some(enabled[at.map_or(0, |a| (a + 1) % n)]);
            }
            if up {
                self.more_highlight = Some(enabled[at.map_or(n - 1, |a| (a + n - 1) % n)]);
            }
            if enter && let Some(Some((.., a))) = self.more_highlight.and_then(|h| items.get(h)) {
                actions.push(a.clone());
                egui::Popup::close_id(ui.ctx(), id);
            }
        } else {
            self.more_highlight = None;
        }
        let popup = crate::menu::popup(more, p).id(id).align(egui::RectAlign::BOTTOM_END).gap(6.0);
        popup.show(|ui| {
            for (i, item) in items.iter().enumerate() {
                let Some((icon, label, keys, action)) = item else {
                    let (r, _) =
                        ui.allocate_exact_size(vec2(220.0, crate::menu::SEPARATOR_HEIGHT), Sense::hover());
                    crate::menu::paint_separator(ui.painter(), p, r.left(), r.right(), r.top());
                    continue;
                };
                let (r, resp) = ui.allocate_exact_size(vec2(220.0, crate::menu::ITEM_HEIGHT), Sense::click());
                if resp.hovered() && ui.input(|inp| inp.pointer.delta() != egui::Vec2::ZERO) {
                    self.more_highlight = Some(i);
                }
                let row = crate::menu::Row {
                    icon: *icon,
                    label,
                    shortcut: keys,
                    enabled: true,
                    lit: self.more_highlight == Some(i),
                    pressed: resp.is_pointer_button_down_on(),
                };
                crate::menu::paint_row(ui.painter(), p, r, row);
                if resp.clicked() {
                    actions.push(action.clone());
                }
            }
        });
    }

    /// Toasts at the bottom centre of the main area, 24 pt above the window edge: inverse
    /// capsules 40 high. Messages go after 4 s.
    fn toasts(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        self.toasts.retain(|(_, t)| now - t < 4.0);
        let p = self.settings.theme.palette();
        let area = if self.main_area.is_positive() { self.main_area } else { ctx.content_rect() };
        let dark = self.settings.theme == Theme::Dark;
        let [r, g, b, _] = if dark { [0xF2, 0xF0, 0xEB, 0xFF] } else { p.ink.to_array() };
        let fill = egui::Color32::from_rgba_unmultiplied(r, g, b, 0xF2);
        let ink = p.paper;
        let mut msgs: Vec<String> = self.toasts.iter().map(|(m, _)| m.clone()).collect();
        for id in &self.downloading {
            msgs.push(format!("Downloading {id}…"));
        }
        let mut y = area.bottom() - 24.0;
        for (i, m) in msgs.iter().enumerate() {
            let id = Id::new(("toast", i));
            let resp = egui::Area::new(id)
                .order(egui::Order::Tooltip)
                .fixed_pos(egui::pos2(area.center().x, y))
                .pivot(Align2::CENTER_BOTTOM)
                .interactable(false)
                .show(ctx, |ui| {
                    let painter = ui.painter().clone();
                    let font = theme::ui_medium(13.0);
                    let m = crate::pages::elide(&painter, m, &font, 480.0);
                    let msg = painter.layout_no_wrap(m, font, ink);
                    let (rect, _) =
                        ui.allocate_exact_size(vec2(16.0 + msg.size().x + 16.0, 40.0), Sense::hover());
                    painter.add(
                        egui::epaint::RectShape::filled(
                            rect.translate(vec2(0.0, 8.0)),
                            20.0,
                            egui::Color32::from_black_alpha(0x2E),
                        )
                        .with_blur_width(48.0),
                    );
                    painter.rect_filled(rect, 20.0, fill);
                    let cy = rect.center().y;
                    painter.galley(egui::pos2(rect.left() + 16.0, cy - msg.size().y / 2.0), msg, ink);
                });
            y -= resp.response.rect.height() + 8.0;
        }
        if !self.toasts.is_empty() || !self.downloading.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }

    fn shortcuts_help(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        if !self.shortcuts_open {
            return;
        }
        let p = self.settings.theme.palette();
        if self.shortcuts_overlay.show(ctx, p, &self.settings.shortcuts, actions) {
            self.shortcuts_open = false;
        }
    }
}

/// Below this toolbar width the view tools and the search field fold into one capsule.
const COMPACT_TOOLBAR: f32 = 800.0;

/// The text & layout popover and the compact toolbar's More menu.
const TEXT_POPUP: &str = "text-popover";
const MORE_POPUP: &str = "more-menu";

/// The toolbar's field on pages (it filters the page).
const PAGE_SEARCH_WIDTH: f32 = 200.0;

/// Under the toolbar while searching: the result navigator (24 pt down, 32 tall) and 16 pt below.
const SEARCH_BAND: f32 = 48.0;

/// Height of the sidebar's header (traffic lights and toggle) and the search sidebar's width.
const SIDEBAR_HEADER: f32 = 36.0;
/// From the sidebar's edge to the centre of the toggle in its header: 4 pt, then half its 32 pt.
const SIDEBAR_HEADER_TOGGLE_INSET: f32 = 4.0 + 16.0;
const SEARCH_SIDEBAR_WIDTH: f32 = 352.0;

/// The toolbar's leading group while the sidebar is `open` (0 hidden, 1 shown) of its `extent`.
/// With the sidebar out the toggle sits in its header; hidden, in a capsule at `docked` (after the
/// traffic lights). In between it rides the sidebar's edge until it reaches that place, the
/// capsule fading in over the rest of the way, and back and forward ride after it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Leading {
    toggle_x: f32,
    capsule: f32,
    nav_x: f32,
}

impl Leading {
    fn new(open: f32, extent: f32, window_left: f32, docked: f32) -> Self {
        use theme::metrics::{GROUP_GAP, SIDEBAR_TOGGLE_WIDTH};
        let edge = window_left + extent * open;
        let parked = docked + SIDEBAR_TOGGLE_WIDTH / 2.0;
        let dock_at = ((parked + SIDEBAR_HEADER_TOGGLE_INSET - window_left) / extent).max(1e-3);
        Self {
            toggle_x: (edge - SIDEBAR_HEADER_TOGGLE_INSET).max(parked),
            capsule: (1.0 - open / dock_at).clamp(0.0, 1.0),
            nav_x: (edge + GROUP_GAP).max(docked + SIDEBAR_TOGGLE_WIDTH + GROUP_GAP),
        }
    }
}

/// The sidebar toggle centred at `centre_x` on the toolbar's line. In the sidebar's header it is a
/// borderless 32 pt button in secondary ink; after the traffic lights, a 40 pt glass capsule in
/// ink. `capsule` (0–1) blends the two while the sidebar slides.
fn sidebar_toggle(
    ui: &mut Ui,
    p: &theme::Palette,
    centre_x: f32,
    centre_y: f32,
    capsule: f32,
    open: bool,
    actions: &mut Vec<Action>,
) -> egui::Rect {
    use theme::metrics::{CAPSULE_HEIGHT, ICON, SIDEBAR_TOGGLE_WIDTH};
    let width = 32.0 + (SIDEBAR_TOGGLE_WIDTH - 32.0) * capsule;
    let rect = egui::Rect::from_center_size(egui::pos2(centre_x, centre_y), vec2(width, CAPSULE_HEIGHT));
    let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    ui.set_clip_rect(ui.ctx().content_rect());
    let resp = ui.interact(rect, Id::new("sidebar-toggle"), Sense::click());
    let radius = rect.height() / 2.0;
    if capsule > 0.0 {
        let mut glass = ui.painter().clone();
        glass.multiply_opacity(capsule);
        widgets::capsule(&glass, rect, p);
    }
    if let Some(fill) = widgets::hover_fill(&ui, &resp, p, false) {
        ui.painter().rect_filled(rect, radius, fill);
    }
    let icon = egui::Rect::from_center_size(rect.center(), vec2(ICON, ICON));
    crate::icons::sidebar(ui.painter(), icon, p.ink_2.lerp_to_gamma(p.ink, capsule));
    let resp = resp.on_hover_text(if open { "Hide sidebar (⌘B)" } else { "Show sidebar (⌘B)" });
    if resp.clicked() {
        actions.push(Action::ToggleSidebar);
    }
    rect
}

fn translation_menu(ui: &mut Ui, library: &Library, settings: &Settings, actions: &mut Vec<Action>) {
    let p = settings.theme.palette();
    for (i, e) in library.translations().enumerate() {
        let m = &e.meta;
        let current = m.id == settings.translation;
        let shortcut = if i < 9 { format!("⌘{}", i + 1) } else { String::new() };
        let resp = ui.add(
            egui::Button::selectable(current, egui::RichText::new(format!("{}  {}", m.abbrev, m.language)))
                .shortcut_text(shortcut),
        );
        if resp.clicked() {
            actions.push(Action::SetTranslation(m.id.clone()));
            ui.close();
        }
    }
    crate::menu::separator(ui);
    crate::menu::header(ui, p, "Also under each ayah");
    for e in library.translations() {
        let m = &e.meta;
        if m.id == settings.translation {
            continue;
        }
        let on = settings.parallel_on && settings.parallel.contains(&m.id);
        let mut checked = on;
        if crate::menu::checkbox(ui, &mut checked, format!("{} — {}", m.abbrev, m.language)).changed() {
            actions.push(if checked {
                Action::AddParallel(m.id.clone())
            } else {
                Action::RemoveParallel(m.id.clone())
            });
        }
    }
    let mut translit = settings.typo.transliteration;
    if crate::menu::checkbox(ui, &mut translit, "Transliteration").changed() {
        actions.push(Action::ToggleTransliteration);
    }
    crate::menu::separator(ui);
    if ui.button("Get more translations…").clicked() {
        actions.push(Action::OpenPage(crate::pages::Page::Translations));
        ui.close();
    }
}

impl QuranApp {
    /// The end of every frame: window drag areas, actions, persistence and startup bookkeeping.
    fn end_frame(&mut self, ctx: &egui::Context, frame: &eframe::Frame, actions: Vec<Action>) {
        #[cfg(not(target_os = "macos"))]
        let _ = frame;
        #[cfg(target_os = "macos")]
        {
            // Popups over the title bar aren't drag handles either.
            let bar = egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                vec2(ctx.content_rect().width(), self.chrome.height),
            );
            let mut rects = self.toolbar_items.clone();
            ctx.memory(|m| {
                for layer in m.areas().visible_layer_ids() {
                    if layer.order != egui::Order::Background
                        && let Some(r) = m.area_rect(layer.id)
                        && r.intersects(bar)
                    {
                        rects.push(r.intersect(bar));
                    }
                }
            });
            rects.sort_by(|a, b| a.left().total_cmp(&b.left()));
            if !self.sidebar_motion.moving() {
                self.no_drag.set(frame, &rects, ctx.zoom_factor());
            }
        }

        let had_actions = !actions.is_empty();
        self.apply(ctx, actions);
        if had_actions {
            ctx.request_repaint();
        }
        self.persist(ctx, false);

        if self.startup.frames < 2 {
            crate::trace(self.startup.t0, &format!("frame {} built", self.startup.frames));
        }
        self.startup.frames += 1;
        let Some(first_ui) = self.startup.first_ui else {
            self.startup.first_ui = Some(self.startup.t0.elapsed());
            ctx.request_repaint();
            return;
        };
        if !self.warmed {
            self.warmed = true;
            let mut ids = self.columns();
            ids.push(quran_core::library::TRANSLITERATION_ID.into());
            self.library.warm(&ids);
            if self.startup.probe {
                let presented = self.startup.t0.elapsed();
                println!(
                    "{{\"init_ms\":{:.2},\"first_frame_built_ms\":{:.2},\"first_frame_presented_ms\":{:.2}}}",
                    self.startup.init.as_secs_f64() * 1000.0,
                    first_ui.as_secs_f64() * 1000.0,
                    presented.as_secs_f64() * 1000.0,
                );
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }
}

impl eframe::App for QuranApp {
    fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        if self.startup.frames < 2 {
            crate::trace(self.startup.t0, &format!("frame {} start", self.startup.frames));
        }
        let ctx = ui.ctx().clone();
        let atlas = ctx.fonts(|f| (*f.options(), f.font_atlas_fill_ratio()));
        let rebuilt = self.atlas_seen.is_some_and(|(o, fill)| o != atlas.0 || atlas.1 < fill);
        if std::mem::take(&mut self.fonts_changed) || rebuilt {
            self.reader.invalidate();
        }
        self.poll_tasks(&ctx);
        let mut actions = Vec::new();
        if self.onboarding.is_none() {
            self.shortcuts(&ctx, &mut actions);
        }
        #[cfg(target_os = "macos")]
        if let Some(menu) = &self.menu {
            menu.poll(&mut actions);
        }
        let focused = ctx.input(|i| i.focused)
            || std::env::var_os("QURAN_RS_BACKGROUND").is_some()
            || self.startup.t0.elapsed() > Duration::from_secs(4);
        if self.startup.frames > 2
            && focused
            && let Some(step) = self.demo.pop_front()
        {
            self.demo_step(&ctx, &step, &mut actions);
            ctx.request_repaint();
        }
        if self.settings.theme_system {
            let t = if ctx.system_theme() == Some(egui::Theme::Dark) { Theme::Dark } else { Theme::Light };
            if t != self.settings.theme {
                actions.push(Action::SetTheme(t));
            }
        }
        let p = self.settings.theme.palette();

        self.chrome = chrome(frame, &ctx);
        if let Some(onboarding) = &mut self.onboarding {
            let done = egui::CentralPanel::default()
                .frame(Frame::NONE)
                .show(ui, |ui| onboarding.show(ui, &self.library, &mut self.settings, &mut actions))
                .inner;
            if done {
                self.onboarding = None;
                self.settings.onboarded = true;
                open_reader_window(&ctx);
            }
            self.end_frame(&ctx, frame, actions);
            return;
        }
        #[cfg(target_os = "macos")]
        self.toolbar_items.clear();
        let open = self.sidebar_open_fraction(&ctx, frame);
        if open > 0.0 {
            let extent = self.sidebar_extent;
            let attached = self.settings.sidebar_attached;
            let inset = if attached { 0.0 } else { theme::metrics::SIDEBAR_INSET };
            egui::Panel::left("sidebar")
                .resizable(false)
                .exact_size(extent * open)
                .frame(Frame::NONE.fill(p.paper))
                .show_separator_line(false)
                .show(ui, |ui| {
                    let area = ui.max_rect();
                    let full =
                        egui::Rect::from_min_max(egui::pos2(area.right() - extent, area.top()), area.max);
                    let panel = egui::Rect::from_min_max(
                        full.min + vec2(inset, inset),
                        egui::pos2(full.right(), full.bottom() - inset),
                    );
                    if attached {
                        ui.painter().rect_filled(panel, 0.0, p.sidebar);
                        ui.painter().vline(
                            panel.right() - 0.5,
                            panel.y_range(),
                            Stroke::new(1.0, p.separator),
                        );
                    } else {
                        let radius = theme::radius::LG;
                        p.shadows.panel.paint(ui.painter(), panel, radius);
                        ui.painter().rect_filled(panel, radius, p.sidebar);
                    }
                    let content = egui::Rect::from_min_max(panel.min + vec2(0.0, SIDEBAR_HEADER), panel.max);
                    let mut child = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(content)
                            .layout(egui::Layout::top_down(egui::Align::Min)),
                    );
                    child.set_clip_rect(content);
                    let env = crate::sidebar::SidebarEnv {
                        library: &self.library,
                        study: &self.study,
                        translation: &self.settings.translation,
                        theme: self.settings.theme,
                        location: self.reader.location,
                        recent: std::iter::once(self.reader.location)
                            .chain(self.history.recent().copied())
                            .fold(Vec::new(), |mut v: Vec<AyahRef>, here| {
                                if !v.iter().any(|r| r.surah == here.surah) {
                                    v.push(here);
                                }
                                v
                            }),
                        selection: self.reader.selected_refs().first().copied(),
                        page: self.pages.page,
                    };
                    self.sidebar.show(
                        &mut child,
                        &env,
                        &mut self.settings.sidebar_tab,
                        &mut self.settings.whole_words,
                        &mut actions,
                    );
                });
        }
        let first = self.startup.frames == 0;
        egui::CentralPanel::default().frame(Frame::NONE.fill(p.paper)).show(ui, |ui| {
            let full = ui.max_rect();
            self.main_area = full;
            if !self.pages.reader() {
                let query = self.pages.query.clone();
                let env = crate::pages::PageEnv {
                    library: &self.library,
                    study: &self.study,
                    settings: &self.settings,
                    query: &query,
                    toolbar: self.chrome.height,
                    catalog: self.catalog.as_ref(),
                    downloading: &self.downloading,
                };
                self.pages.show(ui, full, &env, &mut actions);
                let material = ctx.animate_bool_with_time(
                    Id::new("toolbar-material"),
                    self.pages.scrolled,
                    theme::motion::FAST_MS / 1000.0,
                );
                self.toolbar(ui, full, material, &mut actions);
                return;
            }
            let reader_rect = full;
            let cols = if self.settings.typo.mode == Mode::Reading {
                vec![self.settings.translation.clone()]
            } else {
                self.columns()
            };
            let searching = self.sidebar.result_position().is_some();
            let band = if searching { SEARCH_BAND } else { 0.0 };
            let keyboard = !ctx.egui_wants_keyboard_input() && self.palette.is_none();
            // Focus mode: two points larger, a wider measure, a quiet head under where the toolbar
            // would be.
            let focus_typo;
            let (typo, top) = if self.focus {
                let mut t = self.settings.typo.clone();
                t.size = (t.size + 2.0).round();
                t.arabic_size = (t.arabic_size + 3.0).round();
                t.measure = 680.0 / t.size;
                t.quiet_head = true;
                focus_typo = t;
                (&focus_typo, 52.0 + 12.0 - crate::reader::layout::PAGE_TOP)
            } else {
                (&self.settings.typo, self.chrome.height + band)
            };
            let env = Env {
                library: &self.library,
                study: &self.study,
                typo,
                theme: self.settings.theme,
                columns: &cols,
                keyboard,
                toolbar: top,
                resizing: self.sidebar_motion.moving(),
            };
            let mut reader_ui = ui.new_child(egui::UiBuilder::new().max_rect(reader_rect));
            self.reader.marker_for = self.note_popover.as_ref().map(|n| n.verse);
            self.reader.show(&mut reader_ui, &env, &mut actions);
            if first {
                crate::trace(self.startup.t0, "reader laid out and painted");
            }
            if self.startup.frames > 2 {
                self.reading.tick(
                    ctx.input(|i| i.time),
                    ctx.input(|i| i.focused),
                    self.reader.location,
                    self.reader.furthest_in_view,
                    &self.settings.translation,
                    &self.study,
                );
            }
            if band > 0.0 {
                // egui can't blur what scrolls beneath, so the band is solid paper.
                let r = egui::Rect::from_min_size(
                    reader_rect.min,
                    vec2(reader_rect.width(), self.chrome.height + band),
                );
                ui.painter().rect_filled(r, 0.0, p.paper);
            }
            self.result_navigator(ui, reader_rect, &mut actions);
            let material = ctx.animate_bool_with_time(
                Id::new("toolbar-material"),
                self.reader.under_toolbar && band == 0.0,
                theme::motion::FAST_MS / 1000.0,
            );
            if self.focus {
                self.focus_chrome(ui, full, &mut actions);
            } else {
                self.toolbar(ui, full, material, &mut actions);
            }
        });

        if let Some(note) = &mut self.note_popover {
            let marker = self
                .reader
                .marker_rect
                .unwrap_or_else(|| crate::note_popover::fallback_anchor(ctx.content_rect()));
            if note.show(
                &ctx,
                p,
                &self.library,
                &mut self.study,
                &self.settings.translation,
                marker,
                &mut actions,
            ) {
                self.note_popover = None;
            }
        }
        if let Some(c) = &mut self.compare {
            let selected = self.reader.selected_refs();
            if !selected.contains(&c.verse) || !self.pages.reader() {
                self.compare = None;
            } else {
                let anchor = self
                    .reader
                    .selection_rect
                    .unwrap_or_else(|| crate::note_popover::fallback_anchor(ctx.content_rect()));
                if c.show(&ctx, p, &self.library, &self.settings, anchor, &mut actions) {
                    self.compare = None;
                }
            }
        }
        if let Some(menu) = &mut self.verse_menu {
            let refs = self.reader.selected_refs();
            if menu.show(&ctx, p, &self.study, &refs, &self.settings, &mut actions) {
                self.verse_menu = None;
            }
        }
        if let Some(pal) = &mut self.palette {
            let close = pal.show(&ctx, &self.library, &self.study, &self.settings, &mut actions);
            if close {
                self.palette = None;
            }
        }
        if let Some(s) = &mut self.settings_ui {
            let open = s.show(
                &ctx,
                &self.library,
                &mut self.settings,
                &self.study,
                self.catalog.as_ref(),
                &self.downloading,
                &mut actions,
            );
            if !open {
                self.settings_ui = None;
            }
        }
        // A text file dropped on the window imports as a translation.
        if let Some(path) = ctx.input(|i| i.raw.dropped_files.first().map(|f| f.path().to_path_buf())) {
            match crate::cli::import_file(&path, None, None) {
                Ok(t) => {
                    let name = t.meta.name.clone();
                    match self.library.install(t) {
                        Ok(id) => {
                            self.toast(&ctx, format!("Imported {name}"));
                            self.settings.translation = id;
                            self.reader.invalidate();
                        }
                        Err(e) => self.toast(&ctx, format!("Couldn't save the import: {e:#}")),
                    }
                }
                Err(e) => self.toast(&ctx, format!("Import failed: {e:#}")),
            }
        }
        if let Some(sheet) = &mut self.share_sheet {
            let t = self.library.load(&self.settings.translation).ok();
            match sheet.show(&ctx, p, t.as_ref(), &mut self.settings.share) {
                crate::share_sheet::Outcome::Open => {
                    if sheet.waiting() {
                        ctx.request_repaint();
                    }
                }
                crate::share_sheet::Outcome::Close => self.share_sheet = None,
                crate::share_sheet::Outcome::Toast(msg) => self.toast(&ctx, msg),
                crate::share_sheet::Outcome::Share { path, anchor } => {
                    #[cfg(target_os = "macos")]
                    crate::macos_window::share_file(frame, &path, anchor, ctx.zoom_factor());
                    #[cfg(not(target_os = "macos"))]
                    {
                        let _ = anchor;
                        self.toast(&ctx, format!("Saved the image to {}", path.display()));
                    }
                }
            }
        }
        self.shortcuts_help(&ctx, &mut actions);
        self.plan_reminder(&ctx);
        self.auto_backup(&ctx);
        self.toasts(&ctx);
        self.atlas_seen = Some(ctx.fonts(|f| (*f.options(), f.font_atlas_fill_ratio())));
        self.end_frame(&ctx, frame, actions);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.reading.finish(&self.study);
        self.settings.surah = self.reader.location.surah;
        self.settings.ayah = self.reader.location.ayah;
        self.settings.save(&self.study);
    }
}

/// After onboarding: the reader's 1280 × 800 window, centred on the screen.
fn open_reader_window(ctx: &egui::Context) {
    let size = vec2(1280.0, 800.0);
    ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
    if let Some(monitor) = ctx.input(|i| i.viewport().monitor_size) {
        let pos = ((monitor - size) / 2.0).max(egui::Vec2::ZERO);
        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos.to_pos2()));
    }
}

/// Where automatic backups go.
pub fn backups_dir(data_dir: &std::path::Path) -> PathBuf {
    data_dir.join("backups")
}

/// The newest backup: when (Unix seconds) and its size in bytes.
pub fn last_backup(data_dir: &std::path::Path) -> Option<(i64, u64)> {
    std::fs::read_dir(backups_dir(data_dir))
        .ok()?
        .filter_map(|e| e.ok()?.metadata().ok())
        .filter(|m| m.is_file())
        .filter_map(|m| {
            let at = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64;
            Some((at, m.len()))
        })
        .max_by_key(|(at, _)| *at)
}

/// Opens a folder in the system's file browser.
pub fn reveal(path: &std::path::Path) {
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    let _ = std::process::Command::new(opener).arg(path).spawn();
}

/// Opens a web page in the browser.
pub fn open_url(url: &str) {
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    let _ = std::process::Command::new(opener).arg(url).spawn();
}

/// A surah's label for lists: "2 · Al-Baqarah".
pub fn surah_label(s: SurahId) -> String {
    format!("{} · {}", s.0, s.name())
}

#[cfg(test)]
mod tests {
    use super::Leading;

    const EXTENT: f32 = 264.0;
    const DOCKED: f32 = 96.0;

    #[test]
    fn the_leading_group_ends_where_the_static_layouts_put_it() {
        let shown = Leading::new(1.0, EXTENT, 0.0, DOCKED);
        assert_eq!(shown, Leading { toggle_x: 244.0, capsule: 0.0, nav_x: 272.0 });
        let hidden = Leading::new(0.0, EXTENT, 0.0, DOCKED);
        assert_eq!(hidden, Leading { toggle_x: 116.0, capsule: 1.0, nav_x: 144.0 });
    }
}
