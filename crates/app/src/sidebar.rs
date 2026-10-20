//! Left sidebar: a floating source list — study collections, the library, then the surahs or the
//! thirty juz — that turns into search results while a search is active.
//!
//! Ported from bible-rs (MIT).

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use eframe::egui::text::{LayoutJob, TextFormat, TextWrapping};
use eframe::egui::{
    self, Align, Align2, Color32, FontId, Key, Painter, Rect, Response, ScrollArea, Sense, Ui, pos2, vec2,
};
use quran_core::library::Library;
use quran_core::meta::{AyahRef, Juz, SurahId};
use quran_core::search::{Query, Scope};
use quran_core::study::{HighlightColor, StudyStore};

use crate::actions::{Action, ExportKind, SidebarTab};
use crate::theme::{self, Palette, Theme, text};
use crate::widgets;

/// Source-list geometry (points): rows inset from the panel's edges, 32 pt rows on 8 pt padding.
const LIST_INSET: f32 = 8.0;
const ROW_HEIGHT: f32 = 32.0;
const ROW_PAD: f32 = 8.0;
const ICON_LANE: f32 = 16.0;
const GAP: f32 = 8.0;
/// Surah and juz rows: two lines beside a numbered badge.
const SURAH_ROW: f32 = 48.0;
/// An open collection lists its most recent items, then "Show all".
const RECENT_ITEMS: usize = 5;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum StudyKind {
    Bookmarks,
    Highlights,
    Notes,
}

impl StudyKind {
    const ALL: [StudyKind; 3] = [StudyKind::Bookmarks, StudyKind::Highlights, StudyKind::Notes];
}

/// What the sidebar needs from the rest of the app each frame.
pub struct SidebarEnv<'a> {
    pub library: &'a Library,
    pub study: &'a StudyStore,
    pub translation: &'a str,
    pub theme: Theme,
    pub location: AyahRef,
    /// Recently visited places, newest first.
    pub recent: Vec<AyahRef>,
    /// The ayah selected in the reader (its list item shows as selected).
    pub selection: Option<AyahRef>,
    /// The page in the main area: its row is selected, and the list follows the reader only
    /// while it is shown.
    pub page: crate::pages::Page,
}

pub struct Sidebar {
    /// Which study collections are expanded, and the one last opened (drawn selected).
    study_open: [bool; 3],
    study_selected: Option<StudyKind>,
    /// Collections showing every item rather than the most recent few.
    study_all: [bool; 3],
    /// Highlights shown: all, or one colour.
    highlight_filter: Option<HighlightColor>,
    /// Type-to-filter: the filter field replaces the study rows and recent places appear.
    filtering: bool,
    filter: String,
    filter_focus: bool,
    /// Frames left to hold the source list at its top.
    scroll_top: u8,
    pub query: String,
    /// Whether the toolbar's search field has focus (arrow keys then move through results).
    pub field_focused: bool,
    focus: bool,
    scope: Scope,
    results: Arc<Vec<u16>>,
    entries: Arc<Vec<Entry>>,
    key: Option<(String, String, Scope, bool)>,
    /// Results ordered with the current surah first, and the surah they were ordered for.
    current_first: bool,
    order_key: (bool, SurahId),
    /// Steps through the results asked for by the reader's navigator.
    pending_step: i32,
    elapsed: Duration,
    selected: usize,
    scroll_to_selected: bool,
    /// The surah (or juz) last scrolled into view in the list.
    revealed: Option<(SidebarTab, u8)>,
    /// Surahs with highlights or notes, keyed by the study store's generation.
    activity: (u64, HashSet<u8>),
    /// For a search that found nothing: a spelling that finds ayahs (query, count, first).
    suggestion: Option<(String, usize, u16)>,
}

impl Sidebar {
    pub fn new(_tab: SidebarTab) -> Self {
        Self {
            study_open: [false; 3],
            study_selected: None,
            study_all: [false; 3],
            highlight_filter: None,
            filtering: false,
            filter: String::new(),
            filter_focus: false,
            scroll_top: 0,
            query: String::new(),
            field_focused: false,
            focus: false,
            scope: Scope::All,
            results: Arc::new(Vec::new()),
            entries: Arc::new(Vec::new()),
            key: None,
            current_first: false,
            order_key: (false, SurahId(1)),
            pending_step: 0,
            elapsed: Duration::ZERO,
            selected: 0,
            scroll_to_selected: false,
            revealed: None,
            activity: (0, HashSet::new()),
            suggestion: None,
        }
    }

    pub fn focus_search(&mut self, q: Option<String>) {
        self.focus = true;
        if let Some(q) = q {
            self.query = q;
        }
    }

    /// Whether the toolbar's search field should take focus this frame.
    pub fn take_focus(&mut self) -> bool {
        std::mem::take(&mut self.focus)
    }

    /// Expands a study collection by name (`bookmarks`, `highlights`, `notes`), or starts
    /// filtering surahs (`filter`).
    pub fn open_study(&mut self, name: &str) {
        let kind = match name {
            "bookmarks" => StudyKind::Bookmarks,
            "highlights" => StudyKind::Highlights,
            "notes" => StudyKind::Notes,
            "filter" => {
                self.start_filter(String::new());
                return;
            }
            _ => return,
        };
        self.toggle_study(kind);
    }

    fn toggle_study(&mut self, kind: StudyKind) {
        let i = StudyKind::ALL.iter().position(|k| *k == kind).unwrap_or(0);
        self.study_open[i] = !self.study_open[i];
        self.study_selected = if self.study_open[i] {
            Some(kind)
        } else {
            StudyKind::ALL.iter().zip(self.study_open).rev().find(|(_, o)| *o).map(|(k, _)| *k)
        };
    }

    fn start_filter(&mut self, text: String) {
        self.filtering = true;
        self.filter = text;
        self.filter_focus = true;
        self.scroll_top = 3;
    }

    pub fn invalidate_search(&mut self) {
        self.key = None;
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn searching(&self) -> bool {
        !self.query.trim().is_empty()
    }

    /// The selected result and how many there are, while a search has results.
    pub fn result_position(&self) -> Option<(usize, usize)> {
        (self.searching() && !self.results.is_empty()).then(|| (self.selected + 1, self.results.len()))
    }

    /// Moves to the next (`1`) or previous (`-1`) result.
    pub fn step_result(&mut self, delta: i32) {
        self.pending_step += delta;
    }

    pub fn show(
        &mut self,
        ui: &mut Ui,
        env: &SidebarEnv<'_>,
        tab: &mut SidebarTab,
        whole_words: &mut bool,
        actions: &mut Vec<Action>,
    ) {
        if self.searching() {
            self.search(ui, env, whole_words, actions);
        } else {
            // Typing over the list (with nothing else taking keys) starts filtering the surahs.
            if !self.filtering && ui.ui_contains_pointer() && !ui.ctx().egui_wants_keyboard_input() {
                let typed: String = ui.input(|i| {
                    i.events
                        .iter()
                        .filter_map(|e| match e {
                            egui::Event::Text(t) if i.modifiers.is_none() || i.modifiers.shift_only() => {
                                Some(t.clone())
                            }
                            _ => None,
                        })
                        .collect()
                });
                if typed.chars().any(char::is_alphanumeric) {
                    self.start_filter(typed);
                }
            }
            if !matches!(tab, SidebarTab::Surahs | SidebarTab::Juz) {
                *tab = SidebarTab::Surahs;
            }
            self.source_list(ui, env, tab, actions);
        }
    }

    fn source_list(
        &mut self,
        ui: &mut Ui,
        env: &SidebarEnv<'_>,
        tab: &mut SidebarTab,
        actions: &mut Vec<Action>,
    ) {
        let p = env.theme.palette();
        let study = env.study;
        let abbrev = env.library.get(env.translation).map(|e| e.meta.abbrev.clone()).unwrap_or_default();
        let here = env.location;
        let reading = env.page == crate::pages::Page::Reader;
        let current = match tab {
            SidebarTab::Juz => (SidebarTab::Juz, here.juz()),
            _ => (SidebarTab::Surahs, here.surah),
        };
        if !reading {
            self.revealed = None;
        }
        let reveal = reading && self.revealed != Some(current);
        if reveal {
            self.revealed = Some(current);
        }
        if self.activity.0 != study.generation() {
            let mut set = HashSet::new();
            set.extend(study.highlights().into_iter().map(|(v, _)| v.surah));
            set.extend(study.notes().into_iter().map(|(v, _)| v.surah));
            self.activity = (study.generation(), set);
        }
        let filter = self.filter.trim().to_string();
        let matching: Option<Vec<SurahId>> = (!filter.is_empty()).then(|| {
            let n: Option<u32> = filter.parse().ok();
            match n.and_then(SurahId::new) {
                Some(s) => vec![s],
                None => quran_core::reference::surah_candidates(&filter, 12),
            }
        });
        let (nh, nn, nb) = study.counts();
        let mut list = ScrollArea::vertical().auto_shrink(false).id_salt("source-list");
        if self.scroll_top > 0 {
            self.scroll_top -= 1;
            list = list.vertical_scroll_offset(0.0);
        }
        list.show(ui, |ui| {
            egui::Frame::NONE.inner_margin(egui::Margin { left: 8, right: 8, top: 4, bottom: 8 }).show(
                ui,
                |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    if self.filtering {
                        if filter_field(ui, p, &mut self.filter, std::mem::take(&mut self.filter_focus)) {
                            self.filtering = false;
                            self.filter.clear();
                        }
                        if !env.recent.is_empty() {
                            section(ui, p, "Recent", true);
                            for (i, r) in env.recent.iter().take(3).enumerate() {
                                // Places are surahs, except an ayah selected where the reader is now.
                                let ayah = env.selection.filter(|s| i == 0 && s.surah == r.surah);
                                let label = match ayah {
                                    Some(s) => s.long(),
                                    None => format!("{} {}:{}", r.surah_id().name(), r.surah, r.ayah),
                                };
                                let resp = row(
                                    ui,
                                    p,
                                    &Row {
                                        icon: Some(crate::icons::clock),
                                        icon_color: Some(p.ink_3),
                                        label: &label,
                                        trailing: Some(abbrev.clone()),
                                        ..Row::default()
                                    },
                                );
                                if resp.clicked() {
                                    actions.push(Action::Jump { to: *r, flash: r.ayah > 1 });
                                }
                            }
                        }
                    } else {
                        section(ui, p, "Study", true);
                        use crate::pages::Page;
                        for (i, (kind, label, n, icon, page)) in [
                            (StudyKind::Bookmarks, "Bookmarks", nb, icon_bookmark as IconFn, Page::Bookmarks),
                            (
                                StudyKind::Highlights,
                                "Highlights",
                                nh,
                                crate::icons::highlighter,
                                Page::Highlights,
                            ),
                            (StudyKind::Notes, "Notes", nn, crate::icons::note, Page::Notes),
                        ]
                        .into_iter()
                        .enumerate()
                        {
                            let open = self.study_open[i];
                            // Rows open their page; the chevron (shown on hover, or while open) lists
                            // the most recent items in place.
                            let next =
                                Rect::from_min_size(ui.cursor().min, vec2(ui.available_width(), ROW_HEIGHT));
                            let hovered = ui.rect_contains_pointer(next);
                            let r = row(
                                ui,
                                p,
                                &Row {
                                    icon: Some(icon),
                                    label,
                                    count: (n > 0).then(|| n.to_string()),
                                    disclosure: (hovered || open).then_some(open),
                                    selected: env.page == page,
                                    ..Row::default()
                                },
                            );
                            let on_chevron = r
                                .interact_pointer_pos()
                                .is_some_and(|pos| pos.x > r.rect.right() - ROW_PAD - ICON_LANE - 4.0);
                            if r.clicked() {
                                if on_chevron {
                                    self.toggle_study(kind)
                                } else {
                                    actions.push(Action::OpenPage(page))
                                }
                            }
                            if open {
                                self.study_items(ui, env, kind, actions);
                            }
                        }
                        let r = row(
                            ui,
                            p,
                            &Row {
                                icon: Some(crate::icons::history),
                                label: "History",
                                selected: env.page == Page::History,
                                ..Row::default()
                            },
                        );
                        if r.clicked() {
                            actions.push(Action::OpenPage(Page::History));
                        }
                        let active = study.plans().iter().filter(|p| p.finished.is_none()).count();
                        let r = row(
                            ui,
                            p,
                            &Row {
                                icon: Some(crate::icons::checklist),
                                label: "Reading Plans",
                                count: (active > 0).then(|| active.to_string()),
                                selected: env.page == Page::Plans,
                                ..Row::default()
                            },
                        );
                        if r.clicked() {
                            actions.push(Action::OpenPage(Page::Plans));
                        }
                        section(ui, p, "Library", false);
                        let r = row(
                            ui,
                            p,
                            &Row {
                                icon: Some(crate::icons::globe),
                                label: "Translations",
                                count: Some(env.library.translations().count().to_string()),
                                selected: env.page == Page::Translations,
                                ..Row::default()
                            },
                        );
                        if r.clicked() {
                            actions.push(Action::OpenPage(Page::Translations));
                        }
                    }

                    // Surahs or juz.
                    ui.add_space(16.0);
                    if matching.is_none() {
                        let idx = usize::from(*tab == SidebarTab::Juz);
                        if let Some(i) = widgets::segmented(ui, p, &["Surahs", "Juz"], idx) {
                            *tab = [SidebarTab::Surahs, SidebarTab::Juz][i];
                        }
                        ui.add_space(8.0);
                    }
                    if *tab == SidebarTab::Juz && matching.is_none() {
                        let juz_here = here.juz();
                        for j in Juz::all() {
                            let sel = reading && j.0 == juz_here;
                            let start = j.start();
                            let sub = format!(
                                "Juz {} · {} {}:{}",
                                j.0,
                                start.surah_id().name(),
                                start.surah,
                                start.ayah
                            );
                            let resp = surah_row(ui, p, j.0, j.name(), &sub, j.arabic(), sel, false);
                            if sel && reveal && !self.filtering {
                                resp.scroll_to_me_animation(
                                    Some(Align::Center),
                                    egui::style::ScrollAnimation::none(),
                                );
                            }
                            if resp.clicked() {
                                actions.push(Action::Jump { to: start, flash: false });
                            }
                        }
                    } else {
                        let list: Vec<SurahId> = matching.clone().unwrap_or_else(|| SurahId::all().collect());
                        if list.is_empty() {
                            egui::Frame::NONE.inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
                                ui.label(
                                    egui::RichText::new("No surah by that name")
                                        .font(text::SMALL.font())
                                        .color(p.ink_2),
                                );
                            });
                        }
                        for s in list {
                            let sel = reading && s.0 == here.surah;
                            let sub = format!("{} · {} ayahs", s.english(), s.ayahs());
                            let resp = surah_row(
                                ui,
                                p,
                                s.0,
                                s.name(),
                                &sub,
                                s.arabic(),
                                sel,
                                self.activity.1.contains(&s.0),
                            );
                            if sel && reveal && !self.filtering {
                                resp.scroll_to_me_animation(
                                    Some(Align::Center),
                                    egui::style::ScrollAnimation::none(),
                                );
                            }
                            if resp.clicked() {
                                actions.push(Action::Jump { to: s.first_ayah(), flash: false });
                                if self.filtering {
                                    self.filtering = false;
                                    self.filter.clear();
                                }
                            }
                        }
                    }
                },
            );
        });
    }

    /// A study collection's items, newest first.
    fn study_items(&mut self, ui: &mut Ui, env: &SidebarEnv<'_>, kind: StudyKind, actions: &mut Vec<Action>) {
        let p = env.theme.palette();
        let study = env.study;
        let t = env.library.load(env.translation).ok();
        let ayah_text = |v: AyahRef| t.as_ref().map(|t| t.get(v).to_string()).unwrap_or_default();
        let items: Vec<(AyahRef, Option<Color32>, Option<String>)> = match kind {
            StudyKind::Bookmarks => {
                let mut b = study.bookmarks();
                b.sort_by_key(|(v, b)| (std::cmp::Reverse(b.created), *v));
                b.into_iter()
                    .map(|(v, b)| (v, None, (!b.label.is_empty()).then(|| b.label.clone())))
                    .collect()
            }
            StudyKind::Highlights => {
                colour_filter(ui, p, &mut self.highlight_filter);
                study
                    .highlights_dated()
                    .into_iter()
                    .filter(|(_, c, _)| self.highlight_filter.is_none_or(|f| f == *c))
                    .map(|(v, c, _)| (v, Some(p.swatch(c)), None))
                    .collect()
            }
            StudyKind::Notes => {
                let mut n = study.notes();
                n.sort_by_key(|(v, n)| (std::cmp::Reverse(n.updated), *v));
                n.into_iter().map(|(v, n)| (v, None, Some(n.body.replace('\n', " ")))).collect()
            }
        };
        if items.is_empty() {
            let hint = match kind {
                StudyKind::Bookmarks => "No bookmarks yet. Select an ayah and press B (or ⌘D).",
                StudyKind::Highlights => "No highlights yet. Select an ayah and press 1–6.",
                StudyKind::Notes => "No notes yet. Select an ayah and press N.",
            };
            egui::Frame::NONE.inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
                ui.label(egui::RichText::new(hint).font(text::SMALL.font()).color(p.ink_2));
            });
            return;
        }
        let abbrev = t.as_ref().map(|t| t.meta.abbrev.clone()).unwrap_or_default();
        let k = StudyKind::ALL.iter().position(|x| *x == kind).unwrap_or(0);
        let total = items.len();
        let shown = if self.study_all[k] { total } else { total.min(RECENT_ITEMS) };
        for (v, bar, note) in items.into_iter().take(shown) {
            let item = ListItem {
                reference: v.long(),
                trailing: abbrev.clone(),
                bar,
                note,
                snippet: ayah_text(v),
                selected: env.selection == Some(v),
            };
            let resp = list_item(ui, p, &item);
            if resp.clicked() {
                actions.push(Action::Jump { to: v, flash: true });
            }
            resp.context_menu(|ui| {
                let (label, a) = match kind {
                    StudyKind::Bookmarks => ("Remove bookmark", Action::RemoveBookmark(v)),
                    StudyKind::Highlights => ("Remove highlight", Action::RemoveHighlight(v)),
                    StudyKind::Notes => ("Delete note", Action::RemoveNote(v)),
                };
                if ui.button(label).clicked() {
                    actions.push(a);
                    ui.close();
                }
                ui.separator();
                if ui.button("Export as Markdown…").clicked() {
                    actions.push(Action::Export(ExportKind::Markdown));
                    ui.close();
                }
                if ui.button("Export as JSON…").clicked() {
                    actions.push(Action::Export(ExportKind::Json));
                    ui.close();
                }
                if ui.button("Import…").clicked() {
                    actions.push(Action::ImportStudy);
                    ui.close();
                }
            });
        }
        if shown < total {
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
            if let Some(fill) = widgets::hover_fill(ui, &resp, p, false) {
                ui.painter().rect_filled(rect, theme::radius::SM, fill);
            }
            ui.painter().text(
                rect.left_center() + vec2(ITEM_PAD.x, 0.0),
                Align2::LEFT_CENTER,
                format!("Show all {total}"),
                theme::ui_medium(text::SMALL.size),
                p.accent,
            );
            if resp.clicked() {
                self.study_all[k] = true;
            }
        }
    }

    /// A search that found nothing: why, ways to widen it, and a spelling that finds something,
    /// optically centred in the panel under the count line.
    fn no_results(&mut self, ui: &mut Ui, p: &Palette, whole_words: &mut bool) {
        let filtered = self.scope != Scope::All || *whole_words;
        let several = self.query.split_whitespace().count() > 1;
        let body = if several {
            "Every word has to appear in the same ayah.\nTry fewer words or search the whole Quran."
        } else {
            "Check the spelling, or try another word.\nArabic searches the Arabic text; other\nwords search the translation."
        };
        let title = format!("No results for “{}”", self.query.trim());
        let scoped = self.scope != Scope::All;
        let mut buttons = Vec::new();
        if scoped {
            buttons.push(widgets::Button::new(widgets::Kind::Secondary, "Search Whole Quran"));
        }
        if filtered {
            buttons.push(widgets::Button::new(widgets::Kind::Ghost, "Clear filters"));
        }
        let empty = crate::pages::Empty {
            glyph: crate::icons::search_thin,
            tone: crate::pages::Tone::Quiet,
            title: &title,
            body,
            buttons: &buttons,
            hints: &[],
            width: 320.0,
        };
        ScrollArea::vertical().id_salt("no-results").auto_shrink(false).show(ui, |ui| {
            let area = ui.max_rect();
            let cx = area.center().x;
            let painter = ui.painter().clone();
            let room = area.width();
            let state_h = empty.height(&painter, room);
            let h = state_h + if self.suggestion.is_some() { 20.0 + 60.0 } else { 0.0 };
            let top = crate::pages::optical_top(area.y_range(), h, area.top() + 24.0);
            let responses = empty.show(ui, p, cx, top, room);
            let mut responses = responses.iter();
            if scoped && responses.next().is_some_and(|r| r.clicked()) {
                self.scope = Scope::All;
            }
            if filtered && responses.next().is_some_and(|r| r.clicked()) {
                self.scope = Scope::All;
                *whole_words = false;
            }
            let mut y = top + state_h;
            if let Some((q, n, first)) = self.suggestion.clone() {
                y += 20.0;
                let suggestion = format!("“{q}”");
                let reference = AyahRef::from_index(first).map(|r| r.long()).unwrap_or_default();
                let detail = if n == 1 {
                    format!("1 ayah · {reference}")
                } else {
                    format!("{} ayahs · first {reference}", group(n))
                };
                let sw = painter
                    .layout_no_wrap(suggestion.clone(), theme::ui_semibold(13.0), Color32::PLACEHOLDER)
                    .size()
                    .x;
                let dw =
                    painter.layout_no_wrap(detail.clone(), text::SMALL.font(), Color32::PLACEHOLDER).size().x;
                let inner = (sw + 8.0 + dw).max(80.0);
                let card = Rect::from_min_size(pos2(cx - inner / 2.0 - 16.0, y), vec2(inner + 32.0, 60.0));
                let resp = ui.interact(card, ui.id().with("did-you-mean"), Sense::click());
                painter.rect_filled(card, 12.0, if resp.hovered() { p.fill_strong } else { p.fill });
                painter.text(
                    pos2(cx, y + 12.0 + 8.0),
                    Align2::CENTER_CENTER,
                    "Did you mean",
                    theme::ui_medium(12.0),
                    p.ink_3,
                );
                let x = cx - inner / 2.0;
                painter.text(
                    pos2(x, y + 32.0 + 8.0),
                    Align2::LEFT_CENTER,
                    &suggestion,
                    theme::ui_semibold(13.0),
                    p.accent,
                );
                painter.text(
                    pos2(x + sw + 8.0, y + 32.0 + 8.0),
                    Align2::LEFT_CENTER,
                    detail,
                    text::SMALL.font(),
                    p.ink_3,
                );
                if resp.clicked() {
                    self.query = q;
                }
            }
            ui.allocate_rect(
                Rect::from_min_max(area.min, pos2(area.right(), top + h + 16.0)),
                Sense::hover(),
            );
        });
    }

    /// Search results: scope and whole-word filters, a count, then the hits grouped by surah with
    /// the matched words marked. Arabic queries search the Arabic text; others the translation.
    fn search(
        &mut self,
        ui: &mut Ui,
        env: &SidebarEnv<'_>,
        whole_words: &mut bool,
        actions: &mut Vec<Action>,
    ) {
        let p = env.theme.palette();
        let arabic = Query::is_arabic(&self.query);
        let Ok(t) = env.library.load(env.translation) else {
            ui.label("The translation couldn't be loaded.");
            return;
        };
        let index = if arabic { None } else { env.library.get(env.translation).and_then(|e| e.index().ok()) };
        let mut jump_to: Option<u16> = None;
        let mut step = std::mem::take(&mut self.pending_step);
        if self.field_focused {
            ui.input_mut(|i| {
                if i.consume_key(egui::Modifiers::NONE, Key::ArrowDown) {
                    step += 1;
                }
                if i.consume_key(egui::Modifiers::NONE, Key::ArrowUp) {
                    step -= 1;
                }
                if i.key_pressed(Key::Enter) {
                    jump_to = self.results.get(self.selected).copied();
                }
            });
        }
        if step != 0 && !self.results.is_empty() {
            self.selected =
                (self.selected as i64 + step as i64).clamp(0, self.results.len() as i64 - 1) as usize;
            self.scroll_to_selected = true;
            jump_to = self.results.get(self.selected).copied();
        }
        let surah = env.location.surah_id();
        let juz = Juz(env.location.juz());
        let q = quran_core::text::quran();
        let snippet_text =
            |r: AyahRef| -> String { if arabic { q.arabic(r).to_string() } else { t.get(r).to_string() } };

        egui::Frame::NONE.inner_margin(egui::Margin { left: 8, right: 8, top: 8, bottom: 4 }).show(
            ui,
            |ui| {
                ui.spacing_mut().item_spacing.y = 12.0;
                let idx = match self.scope {
                    Scope::All => 0,
                    Scope::Surah(_) => 1,
                    Scope::Juz(_) => 2,
                };
                if let Some(i) = widgets::segmented(ui, p, &["Quran", "This Surah", "This Juz"], idx) {
                    self.scope = [Scope::All, Scope::Surah(surah), Scope::Juz(juz)][i];
                }
                let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
                let select = Rect::from_min_size(row.min, vec2(168.0, 32.0));
                let label = match self.scope {
                    Scope::Surah(s) => s.name().to_string(),
                    Scope::Juz(j) => format!("Juz {}", j.0),
                    Scope::All => "All surahs".into(),
                };
                let resp = select_field(ui, p, select, &label, "scope-surah");
                crate::menu::popup(&resp, p).show(|ui| {
                    ui.set_min_width(220.0);
                    if ui.selectable_label(self.scope == Scope::All, "All surahs").clicked() {
                        self.scope = Scope::All;
                        ui.close();
                    }
                    crate::menu::separator(ui);
                    ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                        for s in SurahId::all() {
                            if ui
                                .selectable_label(
                                    self.scope == Scope::Surah(s),
                                    format!("{}  {}", s.0, s.name()),
                                )
                                .clicked()
                            {
                                self.scope = Scope::Surah(s);
                                ui.close();
                            }
                        }
                        crate::menu::separator(ui);
                        for j in Juz::all() {
                            if ui
                                .selectable_label(self.scope == Scope::Juz(j), format!("Juz {}", j.0))
                                .clicked()
                            {
                                self.scope = Scope::Juz(j);
                                ui.close();
                            }
                        }
                    });
                });
                let check =
                    Rect::from_min_size(pos2(select.right() + 16.0, row.center().y - 8.0), vec2(16.0, 16.0));
                if widgets::checkbox(ui, p, check, *whole_words, "Whole words").clicked() {
                    *whole_words = !*whole_words;
                }

                // Run the search when its inputs change (well under a millisecond).
                let key = (
                    self.query.clone(),
                    if arabic { "ar".to_string() } else { env.translation.to_string() },
                    self.scope,
                    *whole_words,
                );
                if self.key.as_ref() != Some(&key) || self.order_key != (self.current_first, surah) {
                    let idx_ref: &quran_core::search::SearchIndex = match &index {
                        Some(i) => i,
                        None => quran_core::search::arabic_index(),
                    };
                    let out = quran_core::search::run(idx_ref, &self.query, self.scope, *whole_words);
                    let mut ayahs = out.ayahs;
                    if self.current_first {
                        ayahs.sort_by_key(|i| AyahRef::from_index(*i).is_none_or(|r| r.surah != surah.0));
                    }
                    self.results = Arc::new(ayahs);
                    self.elapsed = out.elapsed;
                    self.key = Some(key);
                    self.order_key = (self.current_first, surah);
                    self.selected = 0;
                    let query = Query::parse(&self.query, *whole_words);
                    self.entries = Arc::new(result_entries(ui, &snippet_text, arabic, &self.results, &query));
                    self.suggestion = if self.results.is_empty() {
                        quran_core::search::suggest(idx_ref, &self.query, self.scope, *whole_words)
                    } else {
                        None
                    };
                }
                if self.results.is_empty() {
                    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::hover());
                    ui.painter().text(
                        pos2(row.left() + 4.0, row.top() + 4.0 + 8.0),
                        Align2::LEFT_CENTER,
                        "No ayahs match",
                        theme::ui_medium(text::SMALL.size),
                        p.ink_2,
                    );
                    return;
                }
                let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::hover());
                let n = self.results.len();
                let surahs = self.entries.iter().filter(|e| matches!(e.kind, EntryKind::Header(..))).count();
                let summary = format!(
                    "{} {} in {} {}",
                    group(n),
                    if n == 1 { "ayah" } else { "ayahs" },
                    surahs,
                    if surahs == 1 { "surah" } else { "surahs" }
                );
                let font = theme::ui_medium(text::SMALL.size);
                let cy = row.top() + 4.0 + 8.0;
                let r = ui.painter().text(
                    pos2(row.left() + 4.0, cy),
                    Align2::LEFT_CENTER,
                    summary,
                    font.clone(),
                    p.ink_2,
                );
                ui.interact(r, ui.id().with("summary"), Sense::hover()).on_hover_text(format!(
                    "{:.2} ms · {}",
                    self.elapsed.as_secs_f64() * 1000.0,
                    if arabic { "Arabic text" } else { t.meta.abbrev.as_str() }
                ));
                let order = if self.current_first { "This surah first" } else { "In order" };
                let ow = ui.painter().layout_no_wrap(order.into(), font.clone(), p.ink_2).size().x;
                let chevron = Rect::from_center_size(pos2(row.right() - 4.0 - 6.0, cy), vec2(12.0, 12.0));
                let order_rect = Rect::from_min_max(
                    pos2(chevron.left() - 2.0 - ow, cy - 8.0),
                    pos2(chevron.right(), cy + 8.0),
                );
                let resp = ui.interact(order_rect, ui.id().with("order"), Sense::click());
                ui.painter().text(pos2(chevron.left() - 2.0, cy), Align2::RIGHT_CENTER, order, font, p.ink_2);
                crate::icons::chevron_down(ui.painter(), chevron, p.ink_3);
                crate::menu::popup(&resp, p).show(|ui| {
                    if ui.selectable_label(!self.current_first, "In order").clicked() {
                        self.current_first = false;
                        ui.close();
                    }
                    if ui.selectable_label(self.current_first, "This surah first").clicked() {
                        self.current_first = true;
                        ui.close();
                    }
                });
            },
        );

        if self.results.is_empty() {
            self.no_results(ui, p, whole_words);
            return;
        }
        let query = Query::parse(&self.query, *whole_words);
        let entries = self.entries.clone();
        let total = entries.last().map_or(0.0, |e| e.y + e.height);
        let mut sa = ScrollArea::vertical().auto_shrink(false).id_salt("results");
        if std::mem::take(&mut self.scroll_to_selected)
            && let Some(e) =
                entries.iter().find(|e| matches!(e.kind, EntryKind::Item(i, _) if i == self.selected))
        {
            let avail = ui.available_height();
            let cur = ui.ctx().data(|d| d.get_temp::<f32>(egui::Id::new("results-offset"))).unwrap_or(0.0);
            if e.y < cur || e.y + e.height > cur + avail {
                sa = sa.vertical_scroll_offset((e.y - avail / 3.0).max(0.0));
            }
        }
        let abbrev = if arabic { "Arabic".to_string() } else { t.meta.abbrev.clone() };
        let out = sa.show_viewport(ui, |ui, viewport| {
            let width = ui.available_width();
            ui.set_min_height(total + 8.0);
            let origin = ui.min_rect().min + vec2(LIST_INSET, 0.0);
            let first = entries.partition_point(|e| e.y + e.height < viewport.top());
            for e in &entries[first..] {
                if e.y > viewport.bottom() {
                    break;
                }
                let r =
                    Rect::from_min_size(origin + vec2(0.0, e.y), vec2(width - 2.0 * LIST_INSET, e.height));
                match e.kind {
                    EntryKind::Header(s, count) => {
                        let cy = r.top() + 8.0 + 10.0;
                        ui.painter().text(
                            pos2(r.left() + 8.0, cy),
                            Align2::LEFT_CENTER,
                            format!("{} · {}", s.0, s.name().to_uppercase()),
                            text::CAPTION.font(),
                            p.ink_3,
                        );
                        ui.painter().text(
                            pos2(r.right() - 8.0, cy),
                            Align2::RIGHT_CENTER,
                            count.to_string(),
                            text::SMALL.font(),
                            p.ink_3,
                        );
                    }
                    EntryKind::Item(i, rows) => {
                        let gi = self.results[i];
                        let Some(v) = AyahRef::from_index(gi) else { continue };
                        let resp = ui.interact(r, ui.id().with(("hit", gi)), Sense::click());
                        let selected = i == self.selected;
                        if selected {
                            ui.painter().rect_filled(r, 10.0, p.accent_soft);
                        } else if let Some(fill) = widgets::hover_fill(ui, &resp, p, false) {
                            ui.painter().rect_filled(r, 10.0, fill);
                        }
                        let inner = r.shrink2(ITEM_PAD);
                        let cy = inner.top() + 8.0;
                        ui.painter().text(
                            pos2(inner.left(), cy),
                            Align2::LEFT_CENTER,
                            v.long(),
                            text::UI_STRONG.font(),
                            if selected { p.accent } else { p.ink },
                        );
                        ui.painter().text(
                            pos2(inner.right(), cy),
                            Align2::RIGHT_CENTER,
                            &abbrev,
                            theme::ui_medium(11.0),
                            p.ink_3,
                        );
                        if arabic {
                            let line = Rect::from_min_max(
                                pos2(inner.left(), inner.top() + 22.0),
                                pos2(inner.right(), inner.bottom() + 2.0),
                            );
                            paint_arabic_snippet(ui.painter(), p, line, q.arabic(v), &query);
                        } else {
                            let (job, hits) =
                                snippet_job(t.get(v), &query, inner.width(), rows as usize, env.theme);
                            paint_snippet(
                                ui.painter(),
                                p,
                                pos2(inner.left(), inner.top() + 20.0),
                                job,
                                &hits,
                            );
                        }
                        if resp.clicked() {
                            self.selected = i;
                            jump_to = Some(gi);
                        }
                    }
                }
            }
        });
        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("results-offset"), out.state.offset.y));
        if let Some(gi) = jump_to
            && let Some(v) = AyahRef::from_index(gi)
        {
            actions.push(Action::Jump { to: v, flash: true });
        }
    }
}

/// A surah (or juz) in the list: its number in an eight-pointed star, its name over a line of
/// detail, its Arabic name at the end. A dot marks surahs with highlights or notes.
#[allow(clippy::too_many_arguments)]
fn surah_row(
    ui: &mut Ui,
    p: &Palette,
    n: u8,
    name: &str,
    sub: &str,
    arabic: &str,
    selected: bool,
    dot: bool,
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), SURAH_ROW), Sense::click());
    if selected {
        ui.painter().rect_filled(rect, theme::radius::SM, p.accent_soft);
    } else if let Some(fill) = widgets::hover_fill(ui, &resp, p, false) {
        ui.painter().rect_filled(rect, theme::radius::SM, fill);
    }
    let painter = ui.painter();
    let cy = rect.center().y;
    let badge = pos2(rect.left() + ROW_PAD + 14.0, cy);
    widgets::star_badge(painter, p, badge, 14.0, n, selected);
    let x = badge.x + 14.0 + 10.0;
    let ar = painter.layout_no_wrap(arabic.to_string(), theme::arabic_font(17.0), Color32::PLACEHOLDER);
    let right = rect.right() - ROW_PAD;
    let room = (right - ar.size().x - 8.0 - x).max(40.0);
    let (font, color) =
        if selected { (text::UI_STRONG.font(), p.accent) } else { (theme::ui_medium(text::UI.size), p.ink) };
    let name = crate::pages::elide(painter, name, &font, room);
    painter.text(pos2(x, cy - 8.0), Align2::LEFT_CENTER, name, font, color);
    let sub_font = theme::ui_font(11.0);
    let sub = crate::pages::elide(painter, sub, &sub_font, room);
    painter.text(pos2(x, cy + 9.0), Align2::LEFT_CENTER, sub, sub_font, p.ink_3);
    let ah = ar.size().y;
    painter.galley(
        pos2(right - ar.size().x, cy - ah / 2.0 - 1.0),
        ar,
        if selected { p.accent } else { p.ink_2 },
    );
    if dot && !selected {
        painter.circle_filled(pos2(badge.x, badge.y + 14.0 + 3.0), 2.0, p.accent);
    }
    resp
}

/// One line of an ayah's Arabic, from a few words before its first match (marked "…" when
/// that isn't its start), right-aligned in `line`, the matching words washed in the first
/// highlight colour.
fn paint_arabic_snippet(painter: &Painter, p: &Palette, line: Rect, text: &str, query: &Query) {
    let font = theme::quran_font(19.0);
    let space = 6.0;
    let words = quran_core::text::words(text);
    let hits: Vec<bool> = words.iter().map(|w| !query.match_ranges(w).is_empty()).collect();
    let first = hits.iter().position(|h| *h).unwrap_or(0);
    let start = if first > 4 { first - 2 } else { 0 };
    let mut x = line.right();
    let dots = |painter: &Painter, x: f32| {
        painter.text(pos2(x, line.center().y), Align2::RIGHT_CENTER, "…", text::UI.font(), p.ink_3);
    };
    if start > 0 {
        dots(painter, x);
        x -= 14.0;
    }
    for (w, hit) in words.iter().zip(&hits).skip(start) {
        let g = painter.layout_no_wrap(w.clone(), font.clone(), Color32::PLACEHOLDER);
        let gw = g.size().x;
        if x - gw < line.left() + 14.0 {
            dots(painter, x);
            break;
        }
        x -= gw;
        let top = line.center().y - g.size().y / 2.0;
        if *hit {
            let wash =
                Rect::from_center_size(pos2(x + gw / 2.0, line.center().y + 2.0), vec2(gw + 6.0, 30.0));
            painter.rect_filled(wash, 5.0, p.highlights[0]);
        }
        painter.galley(pos2(x, top), g, if *hit { p.ink } else { p.ink_2 });
        x -= space;
    }
}

type IconFn = fn(&Painter, Rect, Color32);

fn icon_bookmark(p: &Painter, r: Rect, c: Color32) {
    crate::icons::bookmark(p, r, c, false);
}

/// A source-list row.
#[derive(Default)]
struct Row<'a> {
    icon: Option<IconFn>,
    /// The icon's colour when not the accent (recent places use tertiary ink).
    icon_color: Option<Color32>,
    label: &'a str,
    /// A small label at the end (a translation's abbreviation).
    trailing: Option<String>,
    /// Extra space before the label (book rows have no icon but line up with icon rows).
    indent: f32,
    count: Option<String>,
    /// A disclosure chevron: `Some(open)`.
    disclosure: Option<bool>,
    selected: bool,
}

fn row(ui: &mut Ui, p: &Palette, r: &Row<'_>) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
    if r.selected {
        ui.painter().rect_filled(rect, theme::radius::SM, p.accent_soft);
    } else if let Some(fill) = widgets::hover_fill(ui, &resp, p, false) {
        ui.painter().rect_filled(rect, theme::radius::SM, fill);
    }
    let painter = ui.painter();
    let cy = rect.center().y;
    let mut x = rect.left() + ROW_PAD + r.indent;
    if let Some(icon) = r.icon {
        icon(
            painter,
            Rect::from_min_size(pos2(x, cy - ICON_LANE / 2.0), vec2(ICON_LANE, ICON_LANE)),
            r.icon_color.unwrap_or(p.accent),
        );
        x += ICON_LANE + GAP;
    }
    let (font, color) =
        if r.selected { (text::UI_STRONG.font(), p.accent) } else { (text::UI.font(), p.ink) };
    painter.text(pos2(x, cy), Align2::LEFT_CENTER, r.label, font, color);
    let mut right = rect.right() - ROW_PAD;
    if let Some(open) = r.disclosure {
        let lane =
            Rect::from_min_size(pos2(right - ICON_LANE, cy - ICON_LANE / 2.0), vec2(ICON_LANE, ICON_LANE));
        let chevron = Rect::from_center_size(lane.center(), vec2(12.0, 12.0));
        let color = if r.selected { p.accent } else { p.ink_3 };
        if open {
            crate::icons::chevron_down(painter, chevron, color);
        } else {
            crate::icons::disclosure(painter, chevron, color);
        }
        right = lane.left() - GAP;
    }
    if let Some(count) = &r.count {
        let (font, color) = if r.selected {
            (theme::ui_medium(text::SMALL.size), p.accent)
        } else {
            (text::SMALL.font(), p.ink_3)
        };
        painter.text(pos2(right, cy), Align2::RIGHT_CENTER, count, font, color);
    }
    if let Some(t) = &r.trailing {
        painter.text(pos2(right, cy), Align2::RIGHT_CENTER, t, theme::ui_medium(11.0), p.ink_3);
    }
    resp
}

/// A bookmark, highlight, note or search hit in a list (`ds-09`): reference and translation,
/// then the note (13/18) and a serif snippet (14/20, two lines, or one under a note).
pub(crate) struct ListItem {
    pub reference: String,
    pub trailing: String,
    /// A highlight's 3 pt colour bar.
    pub bar: Option<Color32>,
    pub note: Option<String>,
    pub snippet: String,
    pub selected: bool,
}

const ITEM_PAD: egui::Vec2 = vec2(12.0, 10.0);

fn wrapped(text: &str, font: FontId, line_height: f32, color: Color32, width: f32, rows: usize) -> LayoutJob {
    let mut job = LayoutJob::single_section(
        text.to_string(),
        TextFormat { font_id: font, color, line_height: Some(line_height), ..Default::default() },
    );
    job.wrap = TextWrapping {
        max_width: width,
        max_rows: rows,
        break_anywhere: false,
        overflow_character: Some('…'),
    };
    job
}

pub(crate) fn list_item(ui: &mut Ui, p: &Palette, item: &ListItem) -> Response {
    let width = ui.available_width();
    let mut text_left = ITEM_PAD.x;
    if item.bar.is_some() {
        text_left += 3.0 + GAP;
    }
    let text_w = width - text_left - ITEM_PAD.x;
    let painter = ui.painter().clone();
    let note =
        item.note.as_ref().map(|n| painter.layout_job(wrapped(n, text::UI.font(), 18.0, p.ink, text_w, 2)));
    let snippet_rows = if note.is_some() { 1 } else { 2 };
    let snippet = painter.layout_job(wrapped(
        &item.snippet,
        FontId::new(text::SNIPPET_SM.size, theme::family(theme::SERIF)),
        text::SNIPPET_SM.line_height,
        p.ink_2,
        text_w,
        snippet_rows,
    ));
    let mut h = ITEM_PAD.y + 16.0 + 4.0;
    if let Some(n) = &note {
        h += n.size().y + 4.0;
    }
    h += snippet.size().y + ITEM_PAD.y;
    let (rect, resp) = ui.allocate_exact_size(vec2(width, h), Sense::click());
    if item.selected {
        painter.rect_filled(rect, 10.0, p.accent_soft);
    } else if let Some(fill) = widgets::hover_fill(ui, &resp, p, false) {
        painter.rect_filled(rect, 10.0, fill);
    }
    let inner = rect.shrink2(ITEM_PAD);
    if let Some(c) = item.bar {
        painter.rect_filled(Rect::from_min_size(inner.min, vec2(3.0, inner.height())), 2.0, c);
    }
    let x = rect.left() + text_left;
    let mut y = inner.top();
    painter.text(
        pos2(x, y + 8.0),
        Align2::LEFT_CENTER,
        &item.reference,
        text::UI_STRONG.font(),
        if item.selected { p.accent } else { p.ink },
    );
    painter.text(
        pos2(inner.right(), y + 8.0),
        Align2::RIGHT_CENTER,
        &item.trailing,
        theme::ui_medium(11.0),
        p.ink_3,
    );
    y += 16.0 + 4.0;
    if let Some(n) = note {
        let h = n.size().y;
        painter.galley(pos2(x, y), n, p.ink);
        y += h + 4.0;
    }
    painter.galley(pos2(x, y), snippet, p.ink_2);
    resp
}

/// The type-to-filter field at the top of the source list. Returns `true` when filtering ends
/// (Escape, or leaving the field empty).
fn filter_field(ui: &mut Ui, p: &Palette, value: &mut String, focus: bool) -> bool {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
    let field = Rect::from_min_size(rect.min, vec2(rect.width(), 28.0));
    let id = egui::Id::new("filter-surahs");
    let focused = ui.memory(|m| m.has_focus(id));
    if focused {
        ui.painter().rect_filled(field.expand(3.0), 11.0, p.focus);
        ui.painter().rect_filled(field, 8.0, p.field);
        ui.painter().rect_stroke(field, 8.0, egui::Stroke::new(1.0, p.accent), egui::StrokeKind::Inside);
    } else {
        ui.painter().rect_filled(field, 8.0, p.fill);
    }
    let glass = Rect::from_min_size(pos2(field.left() + 10.0, field.center().y - 7.0), vec2(14.0, 14.0));
    crate::icons::search(ui.painter(), glass, p.ink_3);
    let edit_rect = Rect::from_min_max(
        pos2(glass.right() + 6.0, field.top()),
        pos2(field.right() - 10.0, field.bottom()),
    );
    // A child UI, so the text edit doesn't move this one's cursor (the row is already allocated).
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(edit_rect));
    let resp = child.add_sized(
        edit_rect.size(),
        egui::TextEdit::singleline(value)
            .id(id)
            .hint_text("Filter surahs")
            .font(text::UI.font())
            .frame(egui::Frame::NONE)
            .margin(egui::Margin::ZERO)
            .vertical_align(Align::Center)
            .desired_width(edit_rect.width()),
    );
    if focus {
        resp.request_focus();
        // Keep what was typed to start filtering, with the cursor after it.
        let end = egui::text::CCursor::new(value.chars().count());
        let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
        state.cursor.set_char_range(Some(egui::text::CCursorRange::one(end)));
        state.store(ui.ctx(), id);
    }
    let escape = resp.has_focus() && ui.input(|i| i.key_pressed(Key::Escape));
    // Leaving the field empty ends filtering, but not when the window itself loses focus.
    let left = resp.lost_focus() && ui.input(|i| i.focused);
    escape || (left && value.trim().is_empty())
}

/// Highlight colour chips: "All", then one swatch per colour (`ds-02`).
fn colour_filter(ui: &mut Ui, p: &Palette, filter: &mut Option<HighlightColor>) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
    let cy = rect.top() + 4.0 + 10.0;
    let x = rect.left() + 32.0;
    let all_font = theme::ui_semibold(11.0);
    let all_w = ui.painter().layout_no_wrap("All".into(), all_font.clone(), p.ink).size().x + 16.0;
    let all = Rect::from_min_size(pos2(x, cy - 10.0), vec2(all_w, 20.0));
    let resp = ui.interact(all, ui.id().with("hl-all"), Sense::click());
    let on = filter.is_none();
    let fill = if on { p.accent } else { widgets::hover_fill(ui, &resp, p, false).unwrap_or(p.fill) };
    ui.painter().rect_filled(all, 10.0, fill);
    ui.painter().text(
        all.center(),
        Align2::CENTER_CENTER,
        "All",
        all_font,
        if on { p.on_accent } else { p.ink_2 },
    );
    if resp.clicked() {
        *filter = None;
    }
    let mut x = all.right() + GAP;
    for c in HighlightColor::ALL {
        let r = Rect::from_min_size(pos2(x, cy - 8.0), vec2(16.0, 16.0));
        let resp = ui.interact(r.expand(2.0), ui.id().with(("hl", c.name())), Sense::click());
        let selected = *filter == Some(c);
        if selected {
            ui.painter().circle_stroke(r.center(), 8.0 + 2.75, egui::Stroke::new(1.5, p.ink));
        } else if resp.hovered() {
            ui.painter().circle_filled(r.center(), 8.0 + 3.0, p.fill_strong);
        }
        ui.painter().circle_filled(r.center(), 8.0, p.swatch(c));
        ui.painter().circle_stroke(r.center(), 7.75, egui::Stroke::new(0.5, Color32::from_black_alpha(46)));
        if resp.on_hover_text(format!("Only {} highlights", c.name())).clicked() {
            *filter = if selected { None } else { Some(c) };
        }
        x = r.right() + GAP;
    }
}

/// A section header: 11 pt semibold tertiary text, 8 pt below the previous row (16 for later
/// sections).
fn section(ui: &mut Ui, p: &Palette, label: &str, first: bool) {
    let (top, h) = if first { (8.0, 28.0) } else { (16.0, 36.0) };
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
    let y = rect.top() + top + (h - top) / 2.0;
    ui.painter().text(
        pos2(rect.left() + ROW_PAD, y),
        Align2::LEFT_CENTER,
        label,
        text::CAPTION.font(),
        p.ink_3,
    );
}

fn group(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A search hit's snippet: Literata 14/20 in secondary ink, starting a few words before the first
/// hit (marked with "…"), the matched phrase in ink at medium weight. Returns the job and the
/// char ranges of the hits within it.
pub fn snippet_job(
    text: &str,
    query: &Query,
    width: f32,
    rows: usize,
    theme: Theme,
) -> (LayoutJob, Vec<std::ops::Range<usize>>) {
    let p = theme.palette();
    let base = TextFormat {
        font_id: FontId::new(text::SNIPPET_SM.size, theme::family(theme::SERIF)),
        color: p.ink_2,
        line_height: Some(text::SNIPPET_SM.line_height),
        ..Default::default()
    };
    let hit = TextFormat {
        font_id: FontId::new(text::SNIPPET_SM.size, theme::family(theme::SERIF_MEDIUM)),
        color: p.ink,
        ..base.clone()
    };
    let ranges = hit_ranges(text, query);
    let start = kwic_start(text, &ranges);
    let mut job = LayoutJob::default();
    let mut chars = 0usize;
    let mut out = Vec::new();
    // A hair of space either side of a hit, for its rounded mark.
    let mut after_hit = false;
    let mut push = |job: &mut LayoutJob, s: &str, f: TextFormat, is_hit: bool| {
        let n = s.chars().count();
        if is_hit {
            out.push(chars..chars + n);
        }
        chars += n;
        job.append(s, if is_hit || after_hit { 2.0 } else { 0.0 }, f);
        after_hit = is_hit;
    };
    if start > 0 {
        push(&mut job, "…", base.clone(), false);
    }
    let mut pos = start;
    for r in ranges.iter().filter(|r| r.start >= start) {
        if r.start > pos {
            push(&mut job, &text[pos..r.start], base.clone(), false);
        }
        push(&mut job, &text[r.start..r.end], hit.clone(), true);
        pos = r.end;
    }
    if pos < text.len() {
        push(&mut job, &text[pos..], base, false);
    }
    job.wrap = TextWrapping {
        max_width: width,
        max_rows: rows,
        break_anywhere: false,
        overflow_character: Some('…'),
    };
    job.halign = Align::LEFT;
    (job, out)
}

/// Matches of the query in the text, with hits separated only by spaces merged ("love one another").
fn hit_ranges(text: &str, query: &Query) -> Vec<std::ops::Range<usize>> {
    let mut ranges: Vec<std::ops::Range<usize>> = Vec::new();
    for r in query.match_ranges(text) {
        match ranges.last_mut() {
            Some(last) if text[last.end..r.start].chars().all(|c| c == ' ') => last.end = r.end,
            _ => ranges.push(r),
        }
    }
    ranges
}

/// Where a snippet starts: five words before the first hit, or at the verse's start when the hit
/// is within its first seven words.
fn kwic_start(text: &str, ranges: &[std::ops::Range<usize>]) -> usize {
    let Some(hit) = ranges.first() else { return 0 };
    let starts: Vec<usize> = text[..hit.start]
        .char_indices()
        .filter(|&(i, c)| !c.is_whitespace() && (i == 0 || text[..i].ends_with(char::is_whitespace)))
        .map(|(i, _)| i)
        .collect();
    if starts.len() <= 7 { 0 } else { starts[starts.len() - 5] }
}

/// Lays out a snippet and paints it with its hits marked (`hl-yellow`, radius 3).
fn paint_snippet(
    painter: &Painter,
    p: &Palette,
    pos: egui::Pos2,
    job: LayoutJob,
    hits: &[std::ops::Range<usize>],
) {
    let galley = painter.layout_job(job);
    for range in hits {
        let mut base = 0usize;
        for row in &galley.rows {
            let n = row.glyphs.len();
            let (s, e) = (range.start.max(base), range.end.min(base + n));
            if s < e {
                let (g0, g1) = (&row.glyphs[s - base], &row.glyphs[e - base - 1]);
                let r = Rect::from_min_max(
                    pos + vec2(row.pos.x + g0.pos.x - 2.0, row.pos.y),
                    pos + vec2(row.pos.x + g1.pos.x + g1.advance_width + 2.0, row.pos.y + row.height()),
                );
                painter.rect_filled(r, 3.0, p.highlights[0]);
            }
            base += n + usize::from(row.ends_with_newline);
        }
    }
    painter.galley(pos, galley, p.ink_2);
}

/// One line of the result list: a surah's header or a hit (its index in the results and how
/// many snippet lines it shows), at a precomputed position so the list can be virtualized.
#[derive(Clone, Copy)]
enum EntryKind {
    Header(SurahId, usize),
    Item(usize, u8),
}

#[derive(Clone, Copy)]
struct Entry {
    kind: EntryKind,
    y: f32,
    height: f32,
}

/// Lays the result list out: headers 28 pt, hits 60 (one snippet line) or 80 (two). A snippet's
/// line count comes from its glyph advances (no shaping), for the first couple of thousand hits;
/// Arabic snippets are one line.
fn result_entries(
    ui: &Ui,
    text: &dyn Fn(AyahRef) -> String,
    arabic: bool,
    results: &[u16],
    query: &Query,
) -> Vec<Entry> {
    let width = ui.available_width() - 2.0 * LIST_INSET - 2.0 * ITEM_PAD.x;
    let font = FontId::new(text::SNIPPET_SM.size, theme::family(theme::SERIF));
    let mut out = Vec::with_capacity(results.len() + 64);
    let mut y = 0.0;
    let mut i = 0;
    let at = |k: u16| AyahRef::from_index(k).unwrap_or(AyahRef::FIRST);
    while i < results.len() {
        let surah = at(results[i]).surah_id();
        let count = results[i..].iter().take_while(|k| at(**k).surah_id() == surah).count();
        out.push(Entry { kind: EntryKind::Header(surah, count), y, height: 28.0 });
        y += 28.0;
        for (k, idx) in results.iter().enumerate().skip(i).take(count) {
            let rows = if arabic {
                1
            } else if k < 2000 {
                let t = text(at(*idx));
                let start = kwic_start(&t, &hit_ranges(&t, query));
                let w: f32 = ui.fonts_mut(|f| t[start..].chars().map(|c| f.glyph_width(&font, c)).sum());
                if w + if start > 0 { 12.0 } else { 0.0 } <= width { 1 } else { 2 }
            } else {
                2
            };
            let height = if arabic { 74.0 } else { 40.0 + 20.0 * rows as f32 };
            out.push(Entry { kind: EntryKind::Item(k, rows), y, height });
            y += height;
        }
        i += count;
    }
    out
}

/// A pop-up button (`ds-14` select): field colour, hairline and soft drop, book glyph, value,
/// up-down chevrons.
fn select_field(ui: &mut Ui, p: &Palette, r: Rect, value: &str, id: &str) -> Response {
    let resp = ui.interact(r, ui.id().with(id), Sense::click());
    let painter = ui.painter();
    painter.add(
        eframe::epaint::RectShape::filled(r.translate(vec2(0.0, 1.0)), 8.0, Color32::from_black_alpha(15))
            .with_blur_width(2.0),
    );
    painter.rect_filled(r, 8.0, if resp.hovered() { p.raised } else { p.field });
    painter.rect_stroke(r, 8.0, egui::Stroke::new(0.5, p.border), egui::StrokeKind::Outside);
    let cy = r.center().y;
    crate::icons::heavy(|| {
        crate::icons::book(
            painter,
            Rect::from_center_size(pos2(r.left() + 12.0 + 8.0, cy), vec2(16.0, 16.0)),
            p.ink_2,
        )
    });
    painter.text(pos2(r.left() + 12.0 + 16.0 + 8.0, cy), Align2::LEFT_CENTER, value, text::UI.font(), p.ink);
    crate::icons::heavy(|| {
        crate::icons::chevrons_up_down(
            painter,
            Rect::from_center_size(pos2(r.right() - 8.0 - 8.0, cy), vec2(16.0, 16.0)),
            p.ink_2,
        )
    });
    resp
}
