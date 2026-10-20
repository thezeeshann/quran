//! Everything the UI can ask the app to do. Widgets push actions; the app applies them once per
//! frame, so keyboard shortcuts, menus, the palette and buttons all share one code path.

use quran_core::AyahRef;
use quran_core::study::HighlightColor;
use serde::{Deserialize, Serialize};

use crate::theme::Theme;

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Go somewhere, recording the current position in history; `flash` marks (and selects)
    /// the ayah arrived at.
    Jump {
        to: AyahRef,
        flash: bool,
    },
    Back,
    Forward,
    /// The previous / next surah.
    PrevSurah,
    NextSurah,
    /// The previous / next juz.
    PrevJuz,
    NextJuz,
    SetTranslation(String),
    /// Switch to the n-th installed translation (⌘1…⌘9).
    TranslationNumber(usize),
    /// Show more than one translation under each ayah.
    ToggleParallel,
    /// Select an ayah and compare it across translations.
    CompareVerse(AyahRef),
    AddParallel(String),
    RemoveParallel(String),
    Highlight(Option<HighlightColor>),
    EditNote(AyahRef),
    EditSelectedNote,
    ToggleBookmark,
    RemoveBookmark(AyahRef),
    RemoveHighlight(AyahRef),
    RemoveNote(AyahRef),
    /// Save a note's text (an empty text removes it).
    SaveNote(AyahRef, String),
    /// Start (or restart) a reading plan today; stop one.
    StartPlan(String),
    StopPlan(String),
    /// Tick a plan's passage (day, index) read or not.
    SetPlanRead {
        plan: String,
        day: u16,
        passage: u8,
        read: bool,
    },
    SetPlanReminder(bool),
    SetAccent(crate::theme::Accent),
    SetReduceMotion(bool),
    /// Erase every highlight, note, bookmark, plan and the history (after a backup).
    EraseStudy,
    /// Show the data folder in the file browser.
    RevealDataDir,
    /// Focus mode: full screen with the chrome out of the way.
    ToggleFocus,
    /// Open the text & layout popover (⌘T).
    TextFormat,
    /// Forget the reading history from this time (Unix seconds) on.
    ClearReading(i64),
    /// Bookmark an ayah, or remove its bookmark.
    ToggleBookmarkAt(AyahRef),
    /// Copy the selected ayahs' text; `Copy` adds the reference and translation.
    CopyText,
    Copy,
    /// Open the ayah menu at a point (a right-click on the selection).
    OpenVerseMenu(eframe::egui::Pos2),
    /// Make an image of the selected ayahs to share.
    ShareImage,
    Compare,
    ClearSelection,
    SelectNext,
    SelectPrev,
    OpenPalette,
    OpenSearch(Option<String>),
    /// Move to the next (1) or previous (-1) search result.
    SearchStep(i32),
    ClearSearch,
    ToggleSidebar,
    SidebarTab(SidebarTab),
    OpenSettings(SettingsTab),
    FontSize(f32),
    FontReset,
    SetTheme(Theme),
    CycleTheme,
    /// Translation (ayah by ayah) ↔ Reading (the Arabic flowing like a mushaf page).
    ToggleMode,
    ToggleTransliteration,
    ToggleTranslation,
    Export(ExportKind),
    ImportStudy,
    /// Import a translation from a file (Tanzil's text format).
    ImportTranslation,
    /// Download a translation from the catalogue.
    DownloadTranslation(String),
    /// Remove a downloaded or imported translation.
    RemoveTranslation(String),
    /// Show a page in the main area (the reader, the library, a study collection).
    OpenPage(crate::pages::Page),
    ShowShortcuts,
    ScrollLines(f32),
    ScrollPages(f32),
    Toast(String),
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SidebarTab {
    #[default]
    Surahs,
    Juz,
    Search,
    Study,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsTab {
    #[default]
    Reading,
    Appearance,
    Translations,
    Shortcuts,
    Data,
    About,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportKind {
    Json,
    Markdown,
}
