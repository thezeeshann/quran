//! The Settings window: Reading, Appearance, Translations, Shortcuts, Data and About, as icon
//! tabs under a 52 pt title bar. It is a window of its own; when the app renders offscreen
//! (screenshots) it fills the main window instead.
//!
//! Ported from bible-rs (MIT).

use eframe::egui::text::{LayoutJob, TextFormat, TextWrapping};
use eframe::egui::{
    self, Align2, Color32, FontId, Painter, Rect, RichText, ScrollArea, Sense, Stroke, Ui, pos2, vec2,
};
use quran_core::catalog::CatalogEntry;
use quran_core::library::Library;
use quran_core::study::StudyStore;

use crate::actions::{Action, ExportKind, SettingsTab};
use crate::app::Settings;
use crate::icons;
use crate::reader::layout::{Mode, Typography};
use crate::theme::{self, Palette, Theme, text};
use crate::widgets::{self, Button, Kind};

/// The window's width, its title bar and its tab strip (icon tabs 76 × 52, 12 pt below).
const WIDTH: f32 = 720.0;
const TITLE_BAR: f32 = 52.0;
const HEADER: f32 = TITLE_BAR + 52.0 + 12.0;
const MARGIN: f32 = 32.0;
const RAISE_FRAMES: u8 = 3;

type IconFn = fn(&Painter, Rect, Color32);

const TABS: [(SettingsTab, &str, IconFn); 6] = [
    (SettingsTab::Reading, "Reading", icons::text_size),
    (SettingsTab::Appearance, "Appearance", icons::sun),
    (SettingsTab::Translations, "Translations", icons::book),
    (SettingsTab::Shortcuts, "Shortcuts", icons::keyboard),
    (SettingsTab::Data, "Data", icons::tray),
    (SettingsTab::About, "About", icons::info),
];

pub struct SettingsUi {
    tab: SettingsTab,
    /// Shortcuts: the filter, and the row waiting for a new chord.
    shortcut_filter: String,
    recording: Option<&'static str>,
    /// Data: Erase… was pressed and waits for the second click.
    confirm_erase: bool,
    /// Frames left in which to bring the window to the front: AppKit can open it behind the main
    /// window (or leave it there when Settings is chosen again), so it is raised until it shows.
    raise: u8,
}

impl SettingsUi {
    pub fn new(tab: SettingsTab) -> Self {
        Self {
            tab,
            shortcut_filter: String::new(),
            recording: None,
            confirm_erase: false,
            raise: RAISE_FRAMES,
        }
    }

    /// Settings was chosen while the window is open: show `tab` and bring the window forward.
    pub fn reopen(&mut self, tab: SettingsTab) {
        self.tab = tab;
        self.raise = RAISE_FRAMES;
    }

    /// Shortcuts (frame 26): every shortcut by group; a row that does one thing takes a new
    /// chord when its keys are clicked.
    fn shortcuts(&mut self, ui: &mut Ui, settings: &mut Settings) {
        use crate::keymap::{self, Chord};
        let p = settings.theme.palette();
        // A chord pressed while recording becomes the row's (taken from any row that had it).
        if let Some(id) = self.recording {
            let pressed = ui.input(|i| {
                i.events.iter().find_map(|e| match e {
                    egui::Event::Key { key, pressed: true, modifiers, .. } => Some((*key, *modifiers)),
                    _ => None,
                })
            });
            if let Some((key, mods)) = pressed {
                if key == egui::Key::Escape {
                    self.recording = None;
                } else if let Some(chord) = Chord::from_press(key, mods) {
                    for other in keymap::ALL.iter().filter(|s| s.id != id) {
                        if keymap::chord(other, &settings.shortcuts) == Some(chord) {
                            settings.shortcuts.insert(other.id.to_string(), String::new());
                        }
                    }
                    let default = keymap::ALL.iter().find(|s| s.id == id).and_then(|s| s.default);
                    if default == Some(chord) {
                        settings.shortcuts.remove(id);
                    } else {
                        settings.shortcuts.insert(id.to_string(), chord.spec());
                    }
                    self.recording = None;
                }
                ui.input_mut(|i| i.events.clear());
            }
        }

        // Tools: filter, restore.
        let (tools, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
        let field = Rect::from_min_size(tools.min, vec2(320.0, 32.0));
        p.shadows.control.paint(ui.painter(), field, 8.0);
        ui.painter().rect_filled(field, 8.0, p.glass);
        let glass = Rect::from_min_size(pos2(field.left() + 10.0, field.center().y - 8.0), vec2(16.0, 16.0));
        icons::heavy(|| icons::search(ui.painter(), glass, p.ink_2));
        let edit = Rect::from_min_max(
            pos2(glass.right() + 6.0, field.top()),
            pos2(field.right() - 8.0, field.bottom()),
        );
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(edit));
        child.add_sized(
            edit.size(),
            egui::TextEdit::singleline(&mut self.shortcut_filter)
                .hint_text(RichText::new("Filter shortcuts").color(p.ink_3).font(text::UI.font()))
                .font(text::UI.font())
                .text_color(p.ink)
                .frame(egui::Frame::NONE)
                .margin(egui::Margin::ZERO)
                .vertical_align(egui::Align::Center),
        );
        let (_, resp) = Button::new(Kind::Secondary, "Restore Defaults").show_right(
            ui,
            p,
            tools.right(),
            tools.center().y,
            ui.id().with("restore-shortcuts"),
        );
        if resp.clicked() {
            settings.shortcuts.clear();
            self.recording = None;
        }
        ui.add_space(24.0);

        let q = self.shortcut_filter.trim().to_lowercase();
        for group in keymap::GROUPS {
            let rows: Vec<&keymap::Shortcut> = keymap::ALL
                .iter()
                .filter(|s| s.group == group && (q.is_empty() || s.label.to_lowercase().contains(&q)))
                .collect();
            if rows.is_empty() {
                continue;
            }
            group_title(ui, p, group);
            let heights = vec![40.0; rows.len()];
            rows_card(ui, p, &heights, |ui, i, r| {
                let s = rows[i];
                row_label(ui, p, r, s.label, None);
                let right = r.right() - 16.0;
                let cy = r.center().y;
                if self.recording == Some(s.id) {
                    let cancel = Rect::from_center_size(pos2(right - 12.0, cy), vec2(24.0, 24.0));
                    if widgets::round_button(
                        ui,
                        p,
                        cancel,
                        icons::close,
                        ui.id().with(("cancel-record", s.id)),
                    )
                    .clicked()
                    {
                        self.recording = None;
                    }
                    let field =
                        Rect::from_min_size(pos2(cancel.left() - 6.0 - 150.0, cy - 14.0), vec2(150.0, 28.0));
                    let painter = ui.painter();
                    painter.rect_filled(field.expand(3.0), 11.0, p.focus);
                    painter.rect_filled(field, 8.0, p.field);
                    painter.rect_stroke(field, 8.0, Stroke::new(1.0, p.accent), egui::StrokeKind::Inside);
                    painter.rect_filled(
                        Rect::from_min_size(pos2(field.left() + 10.0, cy - 7.0), vec2(1.5, 14.0)),
                        0.0,
                        p.accent,
                    );
                    painter.text(
                        pos2(field.left() + 17.5, cy),
                        Align2::LEFT_CENTER,
                        "Press new shortcut…",
                        theme::ui_medium(12.0),
                        p.accent,
                    );
                    return;
                }
                let caps = keymap::keycaps(s, &settings.shortcuts);
                let widths: Vec<f32> = caps.iter().map(|c| widgets::keycap_width(ui.painter(), c)).collect();
                let total = widths.iter().sum::<f32>() + 4.0 * (widths.len().max(1) as f32 - 1.0);
                let area = Rect::from_min_size(
                    pos2(right - total.max(40.0), cy - 12.0),
                    vec2(total.max(40.0), 24.0),
                );
                if s.default.is_some() {
                    let resp = ui.interact(area.expand(4.0), ui.id().with(("record", s.id)), Sense::click());
                    if resp.hovered() {
                        ui.painter().rect_filled(area.expand(4.0), 8.0, p.fill);
                    }
                    if resp.on_hover_text("Click to change").clicked() {
                        self.recording = Some(s.id);
                    }
                }
                if caps.is_empty() {
                    ui.painter().text(
                        pos2(right, cy),
                        Align2::RIGHT_CENTER,
                        "None",
                        text::SMALL.font(),
                        p.ink_3,
                    );
                }
                let mut x = right - total;
                for (c, w) in caps.iter().zip(widths) {
                    widgets::keycap(ui.painter(), p, x, cy, c);
                    x += w + 4.0;
                }
            });
            ui.add_space(24.0);
        }
    }

    /// Data (frame 27): what's in the library and where, exports, backups, and erasing.
    fn data(
        &mut self,
        ui: &mut Ui,
        library: &Library,
        settings: &mut Settings,
        study: &StudyStore,
        actions: &mut Vec<Action>,
    ) {
        let p = settings.theme.palette();
        let data_dir = library.data_dir().to_path_buf();

        group_title(ui, p, "Your library");
        let (h, n, b) = study.counts();
        let stats = [(h, "highlights"), (b, "bookmarks"), (n, "notes"), (study.plans().len(), "plans")];
        rows_card(ui, p, &[72.0, 44.0], |ui, i, r| {
            let painter = ui.painter().clone();
            if i == 0 {
                // Four columns sized to their content, sharing what's left.
                let serif = FontId::new(22.0, theme::family(theme::SERIF));
                let widths: Vec<f32> = stats
                    .iter()
                    .map(|(n, l)| {
                        let a = painter
                            .layout_no_wrap(n.to_string(), serif.clone(), Color32::PLACEHOLDER)
                            .size()
                            .x;
                        let b = painter
                            .layout_no_wrap((*l).into(), theme::ui_medium(11.0), Color32::PLACEHOLDER)
                            .size()
                            .x;
                        a.max(b)
                    })
                    .collect();
                let spare = ((r.width() - 32.0) - widths.iter().sum::<f32>() - 48.0).max(0.0) / 4.0;
                let mut x = r.left() + 16.0;
                for ((n, l), w) in stats.iter().zip(widths) {
                    crate::pages::text(
                        &painter,
                        x,
                        r.top() + 14.0,
                        28.0,
                        &n.to_string(),
                        serif.clone(),
                        p.ink,
                        false,
                    );
                    crate::pages::text(
                        &painter,
                        x,
                        r.top() + 42.0,
                        16.0,
                        l,
                        theme::ui_medium(11.0),
                        p.ink_3,
                        false,
                    );
                    x += w + spare + 16.0;
                }
            } else {
                row_label(ui, p, r, "Stored in", None);
                let (b, resp) = Button::new(Kind::Secondary, "Show in Finder").small().show_right(
                    ui,
                    p,
                    r.right() - 16.0,
                    r.center().y,
                    ui.id().with("reveal"),
                );
                if resp.clicked() {
                    actions.push(Action::RevealDataDir);
                }
                let home = dirs::home_dir().map(|h| h.to_string_lossy().into_owned()).unwrap_or_default();
                let mut path = data_dir.to_string_lossy().into_owned();
                if !home.is_empty() && path.starts_with(&home) {
                    path = format!("~{}", &path[home.len()..]);
                }
                let path = crate::pages::elide(
                    &painter,
                    &path,
                    &FontId::monospace(12.0),
                    b.left() - 10.0 - (r.left() + 140.0),
                );
                painter.text(
                    pos2(b.left() - 10.0, r.center().y),
                    Align2::RIGHT_CENTER,
                    path,
                    FontId::monospace(12.0),
                    p.ink_2,
                );
            }
        });
        ui.add_space(24.0);

        group_title(ui, p, "Export");
        rows_card(ui, p, &[56.0, 56.0, 44.0], |ui, i, r| {
            let right = r.right() - 16.0;
            let cy = r.center().y;
            match i {
                0 | 1 => {
                    let (label, detail, kind) = if i == 0 {
                        ("Markdown", "Readable notes, highlights and bookmarks · ⌘E", ExportKind::Markdown)
                    } else {
                        ("JSON backup", "Everything, re-importable · ⇧⌘E", ExportKind::Json)
                    };
                    row_label(ui, p, r, label, Some(detail));
                    let (_, resp) = Button::new(Kind::Secondary, "Export…")
                        .small()
                        .icon(icons::upload)
                        .show_right(ui, p, right, cy, ui.id().with(("export", i)));
                    if resp.clicked() {
                        actions.push(Action::Export(kind));
                    }
                }
                _ => {
                    row_label(ui, p, r, "Include", None);
                    let labels = ["Highlights", "Notes", "Bookmarks", "History"];
                    let widths: Vec<f32> = labels
                        .iter()
                        .map(|l| {
                            16.0 + 8.0
                                + ui.painter()
                                    .layout_no_wrap((*l).into(), text::UI.font(), Color32::PLACEHOLDER)
                                    .size()
                                    .x
                        })
                        .collect();
                    let mut x = right - widths.iter().sum::<f32>() - 16.0 * 3.0;
                    for (k, (l, w)) in labels.iter().zip(widths).enumerate() {
                        let b = Rect::from_min_size(pos2(x, cy - 8.0), vec2(16.0, 16.0));
                        if widgets::checkbox(ui, p, b, settings.export_include[k], l).clicked() {
                            settings.export_include[k] = !settings.export_include[k];
                        }
                        x += w + 16.0;
                    }
                }
            }
        });
        ui.add_space(24.0);

        group_title(ui, p, "Backup");
        rows_card(ui, p, &[44.0, 44.0, 44.0, 44.0], |ui, i, r| {
            let right = r.right() - 16.0;
            let cy = r.center().y;
            match i {
                0 => {
                    row_label(ui, p, r, "Back up automatically", None);
                    widgets::toggle(
                        ui,
                        p,
                        pos2(right, cy),
                        ui.id().with("backup-auto"),
                        &mut settings.backup_auto,
                    );
                }
                1 => {
                    row_label(ui, p, r, "Frequency", None);
                    let field = Rect::from_min_size(pos2(right - 160.0, cy - 16.0), vec2(160.0, 32.0));
                    let value = if settings.backup_weekly { "Weekly" } else { "Daily" };
                    let resp =
                        widgets::select_field(ui, p, field, value, None, ui.id().with("backup-frequency"));
                    crate::menu::popup(&resp, p).show(|ui| {
                        ui.set_min_width(160.0);
                        if ui.selectable_label(!settings.backup_weekly, "Daily").clicked() {
                            settings.backup_weekly = false;
                        }
                        if ui.selectable_label(settings.backup_weekly, "Weekly").clicked() {
                            settings.backup_weekly = true;
                        }
                    });
                }
                2 => {
                    row_label(ui, p, r, "Last backup", None);
                    let value = match crate::app::last_backup(&data_dir) {
                        Some((at, bytes)) => {
                            let now = quran_core::study::now();
                            let day = crate::time::day_start(at);
                            let when = if day == crate::time::day_start(now) {
                                "Today".to_string()
                            } else if day == crate::time::day_start(now) - 86_400 {
                                "Yesterday".to_string()
                            } else {
                                crate::time::short_date(at)
                            };
                            format!("{when} at {} · {} KB", crate::time::clock(at), bytes.div_ceil(1024))
                        }
                        None => "Never".into(),
                    };
                    ui.painter().text(pos2(right, cy), Align2::RIGHT_CENTER, value, text::UI.font(), p.ink_2);
                }
                _ => {
                    row_label(ui, p, r, "Restore from a JSON backup", None);
                    let (_, resp) = Button::new(Kind::Secondary, "Import Backup…")
                        .small()
                        .icon(icons::download)
                        .show_right(ui, p, right, cy, ui.id().with("import-backup"));
                    if resp.clicked() {
                        actions.push(Action::ImportStudy);
                    }
                }
            }
        });
        ui.add_space(24.0);

        rows_card(ui, p, &[56.0], |ui, _, r| {
            let right = r.right() - 16.0;
            let cy = r.center().y;
            if self.confirm_erase {
                row_label(
                    ui,
                    p,
                    r,
                    "Erase everything?",
                    Some("A backup is saved first; otherwise this can't be undone."),
                );
                let (b, resp) = Button::new(Kind::Destructive, "Erase Everything").small().show_right(
                    ui,
                    p,
                    right,
                    cy,
                    ui.id().with("erase-confirm"),
                );
                if resp.clicked() {
                    actions.push(Action::EraseStudy);
                    self.confirm_erase = false;
                }
                let (_, cancel) = Button::new(Kind::Plain, "Cancel").small().show_right(
                    ui,
                    p,
                    b.left() - 8.0,
                    cy,
                    ui.id().with("erase-cancel"),
                );
                if cancel.clicked() {
                    self.confirm_erase = false;
                }
            } else {
                row_label(
                    ui,
                    p,
                    r,
                    "Erase all study data",
                    Some("Highlights, notes, bookmarks, plans and history. Translations stay."),
                );
                let (_, resp) = Button::new(Kind::Destructive, "Erase…").small().show_right(
                    ui,
                    p,
                    right,
                    cy,
                    ui.id().with("erase"),
                );
                if resp.clicked() {
                    self.confirm_erase = true;
                }
            }
        });
    }

    /// Puts a shortcut row into recording (demo steps).
    pub fn record(&mut self, id: &str) {
        self.recording = crate::keymap::ALL.iter().find(|s| s.id == id).map(|s| s.id);
    }

    fn title(&self) -> &'static str {
        TABS.iter().find(|(t, _, _)| *t == self.tab).map_or("Settings", |(_, l, _)| l)
    }

    /// Shows the window; returns `false` once it is closed.
    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        library: &Library,
        settings: &mut Settings,
        study: &StudyStore,
        catalog: Option<&Result<Vec<CatalogEntry>, String>>,
        downloading: &[String],
        actions: &mut Vec<Action>,
    ) -> bool {
        if crate::offscreen() {
            // A screenshot of the window: fill the main window with it.
            let screen = ctx.content_rect();
            egui::Area::new(egui::Id::new("settings-offscreen"))
                .order(egui::Order::Foreground)
                .fixed_pos(screen.min)
                .show(ctx, |ui| {
                    ui.set_min_size(screen.size());
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(screen));
                    self.window(&mut child, library, settings, study, catalog, downloading, actions);
                });
            return true;
        }
        let mut open = true;
        let mut builder = egui::ViewportBuilder::default()
            .with_title("Settings")
            .with_inner_size([WIDTH, 860.0])
            .with_min_inner_size([WIDTH, 420.0])
            .with_max_inner_size([WIDTH, 4000.0]);
        if cfg!(target_os = "macos") {
            builder =
                builder.with_fullsize_content_view(true).with_titlebar_shown(false).with_title_shown(false);
        }
        ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("settings"), builder, |ui, _class| {
            #[cfg(target_os = "macos")]
            crate::macos_window::unify_window_titled("Settings");
            if self.raise > 0 {
                self.raise -= 1;
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Focus);
                ui.ctx().request_repaint();
            }
            if ui.ctx().input(|i| i.viewport().close_requested() || i.key_pressed(egui::Key::Escape))
                && !ui.ctx().egui_wants_keyboard_input()
            {
                open = false;
            }
            if ui.ctx().input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::W)) {
                open = false;
            }
            let rect = ui.max_rect();
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
            self.window(&mut child, library, settings, study, catalog, downloading, actions);
        });
        open
    }

    /// The window's content: title bar, tabs, then the tab's settings.
    #[allow(clippy::too_many_arguments)]
    fn window(
        &mut self,
        ui: &mut Ui,
        library: &Library,
        settings: &mut Settings,
        study: &StudyStore,
        catalog: Option<&Result<Vec<CatalogEntry>, String>>,
        downloading: &[String],
        actions: &mut Vec<Action>,
    ) {
        let p = settings.theme.palette();
        let rect = ui.max_rect();
        ui.painter().rect_filled(rect, 0.0, p.sidebar);
        // Header: the title over the tabs, in the toolbar material with a hairline.
        let header = Rect::from_min_size(rect.min, vec2(rect.width(), HEADER));
        ui.painter().rect_filled(header, 0.0, p.paper);
        ui.painter().rect_filled(header, 0.0, p.toolbar);
        ui.painter().hline(header.x_range(), header.bottom() - 0.5, Stroke::new(1.0, p.separator));
        ui.painter().text(
            pos2(header.center().x, rect.top() + TITLE_BAR / 2.0),
            Align2::CENTER_CENTER,
            self.title(),
            text::UI_STRONG.font(),
            p.ink,
        );
        let tabs_w = TABS.len() as f32 * 76.0 + (TABS.len() - 1) as f32 * 4.0;
        let mut x = header.center().x - tabs_w / 2.0;
        for (tab, label, icon) in TABS {
            let r = Rect::from_min_size(pos2(x, rect.top() + TITLE_BAR), vec2(76.0, 52.0));
            let resp = ui.interact(r, ui.id().with(("settings-tab", label)), Sense::click());
            let on = self.tab == tab;
            if on {
                ui.painter().rect_filled(r, 10.0, p.accent_soft);
            } else if let Some(fill) = widgets::hover_fill(ui, &resp, p, false) {
                ui.painter().rect_filled(r, 10.0, fill);
            }
            let top = r.top() + (52.0 - 38.0) / 2.0;
            icon(
                ui.painter(),
                Rect::from_min_size(pos2(r.center().x - 10.0, top), vec2(20.0, 20.0)),
                if on { p.accent } else { p.ink_2 },
            );
            let (font, color) =
                if on { (theme::ui_semibold(11.0), p.accent) } else { (theme::ui_medium(11.0), p.ink_2) };
            ui.painter().text(
                pos2(r.center().x, top + 20.0 + 2.0 + 8.0),
                Align2::CENTER_CENTER,
                label,
                font,
                color,
            );
            if resp.clicked() {
                self.tab = tab;
            }
            x += 76.0 + 4.0;
        }
        #[cfg(target_os = "macos")]
        {
            // The title bar moves the window; the tabs don't (see `macos_window::NoDragAreas`).
        }

        let body = Rect::from_min_max(pos2(rect.left(), header.bottom()), rect.max);
        let mut child = ui.new_child(
            egui::UiBuilder::new().max_rect(body).layout(egui::Layout::top_down(egui::Align::Min)),
        );
        child.set_clip_rect(body);
        ScrollArea::vertical().auto_shrink(false).id_salt(("settings", self.title())).show(
            &mut child,
            |ui| {
                egui::Frame::NONE
                    .inner_margin(egui::Margin {
                        left: MARGIN as i8,
                        right: MARGIN as i8,
                        top: 24,
                        bottom: 32,
                    })
                    .show(ui, |ui| {
                        ui.set_width(body.width() - 2.0 * MARGIN);
                        ui.spacing_mut().item_spacing.y = 0.0;
                        match self.tab {
                            SettingsTab::Reading => reading(ui, library, settings, actions),
                            SettingsTab::Appearance => appearance(ui, settings, actions),
                            SettingsTab::Translations => {
                                translations(ui, library, settings, catalog, downloading, actions)
                            }
                            SettingsTab::Shortcuts => self.shortcuts(ui, settings),
                            SettingsTab::Data => self.data(ui, library, settings, study, actions),
                            SettingsTab::About => about(ui, p),
                        }
                    });
            },
        );
    }
}

/// Translations: which is read and which show under each ayah, the transliteration, adding
/// more from the catalogue or a file.
fn translations(
    ui: &mut Ui,
    library: &Library,
    settings: &mut Settings,
    catalog: Option<&Result<Vec<CatalogEntry>, String>>,
    downloading: &[String],
    actions: &mut Vec<Action>,
) {
    let p = settings.theme.palette();
    group_title(ui, p, "Reading");
    let entries: Vec<_> = library.translations().cloned().collect();
    let current = library.get(&settings.translation).map(|e| e.meta.label()).unwrap_or_default();
    rows_card(ui, p, &[48.0, 48.0], |ui, i, r| {
        let right = r.right() - 16.0;
        let cy = r.center().y;
        match i {
            0 => {
                row_label(ui, p, r, "Translation", Some("⌘1…⌘9 switch"));
                let field = Rect::from_min_size(pos2(right - 300.0, cy - 16.0), vec2(300.0, 32.0));
                let resp =
                    widgets::select_field(ui, p, field, &current, Some(icons::book), ui.id().with("reading"));
                crate::menu::popup(&resp, p).show(|ui| {
                    ui.set_min_width(300.0);
                    for e in &entries {
                        if ui.selectable_label(e.meta.id == settings.translation, e.meta.label()).clicked() {
                            actions.push(Action::SetTranslation(e.meta.id.clone()));
                            ui.close();
                        }
                    }
                });
            }
            _ => {
                row_label(ui, p, r, "Transliteration", Some("The Arabic in Latin letters, under each ayah"));
                let mut on = settings.typo.transliteration;
                if widgets::toggle(ui, p, pos2(right, cy), ui.id().with("translit"), &mut on).clicked() {
                    actions.push(Action::ToggleTransliteration);
                }
            }
        }
    });
    ui.add_space(24.0);

    group_title(ui, p, "Also under each ayah");
    let others: Vec<_> = entries.iter().filter(|e| e.meta.id != settings.translation).cloned().collect();
    let rows: Vec<f32> = std::iter::once(48.0).chain(others.iter().map(|_| 44.0)).collect();
    rows_card(ui, p, &rows, |ui, i, r| {
        let right = pos2(r.right() - 16.0, r.center().y);
        if i == 0 {
            row_label(ui, p, r, "Show more than one translation", Some("⇧⌘P"));
            let mut on = settings.parallel_on;
            if widgets::toggle(ui, p, right, ui.id().with("parallel"), &mut on).clicked() {
                actions.push(Action::ToggleParallel);
            }
            return;
        }
        let e = &others[i - 1];
        row_label(ui, p, r, &e.meta.label(), Some(&e.meta.language));
        let mut on = settings.parallel.contains(&e.meta.id);
        if widgets::toggle(ui, p, right, ui.id().with(("par", &e.meta.id)), &mut on).clicked() {
            actions.push(if on {
                Action::AddParallel(e.meta.id.clone())
            } else {
                Action::RemoveParallel(e.meta.id.clone())
            });
        }
    });
    ui.add_space(24.0);

    group_title(ui, p, "More translations");
    let listed = match catalog {
        Some(Ok(l)) => format!("{} translations in {} languages from the Tanzil Project", l.len(), {
            let mut langs: Vec<&str> = l.iter().map(|c| c.language_name()).collect();
            langs.sort_unstable();
            langs.dedup();
            langs.len()
        }),
        Some(Err(e)) if e == "Loading…" => "Loading the catalogue…".to_string(),
        Some(Err(e)) => format!("Couldn't load the catalogue: {e}"),
        None => "Over a hundred translations from the Tanzil Project".to_string(),
    };
    rows_card(ui, p, &[56.0, 56.0], |ui, i, r| {
        let cy = r.center().y;
        match i {
            0 => {
                row_label(ui, p, r, "Download from the catalogue", Some(&listed));
                let (_, resp) = Button::new(Kind::Primary, "Browse…")
                    .small()
                    .icon(icons::download)
                    .show_right(ui, p, r.right() - 16.0, cy, ui.id().with("browse"));
                if resp.clicked() {
                    actions.push(Action::OpenPage(crate::pages::Page::Translations));
                }
            }
            _ => {
                let busy = if downloading.is_empty() {
                    None
                } else {
                    Some(format!("Downloading {}…", downloading.join(", ")))
                };
                row_label(
                    ui,
                    p,
                    r,
                    "Import a file",
                    Some(
                        busy.as_deref()
                            .unwrap_or("Tanzil's sura|aya|text format, or one line per ayah (6,236 lines)"),
                    ),
                );
                let (_, resp) = Button::new(Kind::Secondary, "Import…")
                    .small()
                    .icon(icons::upload)
                    .show_right(ui, p, r.right() - 16.0, cy, ui.id().with("import"));
                if resp.clicked() {
                    actions.push(Action::ImportTranslation);
                }
            }
        }
    });
    ui.add_space(8.0);
    footnote(
        ui,
        p,
        "Downloaded and imported translations are kept in the data folder and work offline. Remove them on the Translations page.",
    );
}

fn group_title(ui: &mut Ui, p: &Palette, title: &str) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::hover());
    ui.painter().text(r.left_center(), Align2::LEFT_CENTER, title, text::UI_STRONG.font(), p.ink);
    ui.add_space(8.0);
}

/// A card of rows (`ds-14` form group): the surface colour with a hairline ring, radius 12; rows
/// are at least 44 pt, separated by hairlines inset 16 pt.
fn rows_card(ui: &mut Ui, p: &Palette, rows: &[f32], mut row: impl FnMut(&mut Ui, usize, Rect)) {
    let width = ui.available_width();
    let height: f32 = rows.iter().sum::<f32>() + (rows.len().saturating_sub(1)) as f32;
    let (card, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    ui.painter().rect_filled(card, 12.0, p.surface);
    ui.painter().rect_stroke(card, 12.0, Stroke::new(0.5, p.border), egui::StrokeKind::Outside);
    let mut y = card.top();
    for (i, h) in rows.iter().enumerate() {
        if i > 0 {
            ui.painter().hline(card.left() + 16.0..=card.right(), y + 0.5, Stroke::new(1.0, p.separator));
            y += 1.0;
        }
        row(ui, i, Rect::from_min_size(pos2(card.left(), y), vec2(width, *h)));
        y += h;
    }
}

/// A row's label (13 pt) and optional detail (12 pt tertiary) at the row's left.
fn row_label(ui: &Ui, p: &Palette, r: Rect, label: &str, detail: Option<&str>) {
    let x = r.left() + 16.0;
    match detail {
        None => {
            ui.painter().text(pos2(x, r.center().y), Align2::LEFT_CENTER, label, text::UI.font(), p.ink);
        }
        Some(d) => {
            ui.painter().text(
                pos2(x, r.center().y - 8.0),
                Align2::LEFT_CENTER,
                label,
                text::UI.font(),
                p.ink,
            );
            ui.painter().text(
                pos2(x, r.center().y + 10.0),
                Align2::LEFT_CENTER,
                d,
                text::SMALL.font(),
                p.ink_3,
            );
        }
    }
}

/// Line length shown in characters; stored as the measure in ems (66 characters ≈ 32 em).
const CHARS_PER_EM: f32 = 66.0 / 32.0;

fn reading(ui: &mut Ui, library: &Library, settings: &mut Settings, actions: &mut Vec<Action>) {
    let p = settings.theme.palette();

    // Preview: Al-Fatihah 1:2 as the reader would set it.
    let typo = settings.typo.clone();
    let width = ui.available_width();
    let inner = width - 48.0;
    let at = quran_core::AyahRef::new(1, 2);
    let translation =
        library.load(&settings.translation).ok().map(|t| t.get(at).to_string()).unwrap_or_default();
    let measure = inner.min(typo.size * typo.measure);
    let size = typo.arabic_size.min(40.0);
    let arabic_lh = (size * typo.arabic_line).round();
    let lines = widgets::quran_lines(ui.painter(), &[at], size, measure);
    let lh = (typo.size * typo.line_height).round();
    let mut job = LayoutJob::single_section(
        translation,
        TextFormat {
            font_id: FontId::new(typo.size, theme::family(theme::SERIF)),
            color: p.ink,
            line_height: Some(lh),
            ..Default::default()
        },
    );
    job.wrap = TextWrapping { max_width: measure, max_rows: 3, ..Default::default() };
    let galley = ui.painter().layout_job(job);
    let show_tr = typo.show_translation && typo.mode == Mode::Translation;
    let card_h = 16.0
        + 16.0
        + 8.0
        + arabic_lh * lines.len() as f32
        + if show_tr { 8.0 + galley.size().y } else { 0.0 }
        + 20.0;
    let (card, _) = ui.allocate_exact_size(vec2(width, card_h), Sense::hover());
    ui.painter().rect_filled(card, 12.0, p.paper);
    ui.painter().rect_stroke(card, 12.0, Stroke::new(0.5, p.border), egui::StrokeKind::Outside);
    let mut label = LayoutJob::single_section(
        "AL-FATIHAH 1:2".into(),
        TextFormat {
            font_id: theme::ui_semibold(text::LABEL.size),
            color: p.accent,
            extra_letter_spacing: 0.12 * 11.0,
            ..Default::default()
        },
    );
    label.wrap.max_width = f32::INFINITY;
    let lg = ui.painter().layout_job(label);
    ui.painter().galley(
        pos2(card.left() + 24.0, card.top() + 16.0 + (16.0 - lg.size().y) / 2.0),
        lg,
        p.accent,
    );
    let x = card.left() + 24.0;
    let mut y = card.top() + 16.0 + 16.0 + 8.0;
    widgets::paint_quran_lines(
        ui.painter(),
        &lines,
        x + measure,
        measure,
        y,
        arabic_lh,
        size,
        p.ink,
        p.verse,
        false,
    );
    y += arabic_lh * lines.len() as f32 + 8.0;
    if show_tr {
        ui.painter().galley(pos2(x, y), galley, p.ink);
    }
    ui.add_space(8.0);
    let (r, _) = ui.allocate_exact_size(vec2(width, 16.0), Sense::hover());
    ui.painter().text(
        pos2(r.left() + 8.0, r.center().y),
        Align2::LEFT_CENTER,
        "Preview updates as you change settings.",
        text::SMALL.font(),
        p.ink_3,
    );
    ui.add_space(24.0);

    group_title(ui, p, "Theme");
    theme_cards(ui, settings, actions);
    ui.add_space(24.0);

    group_title(ui, p, "Text");
    let typo = &mut settings.typo;
    rows_card(ui, p, &[44.0, 44.0, 44.0, 44.0, 44.0], |ui, i, r| {
        let right = r.right() - 16.0;
        let cy = r.center().y;
        let slider = |w: f32| Rect::from_min_size(pos2(right - 44.0 - 12.0 - w, cy - 12.0), vec2(w, 24.0));
        let value = |ui: &Ui, s: String| {
            ui.painter().text(
                pos2(right, cy),
                Align2::RIGHT_CENTER,
                s,
                theme::ui_medium(text::UI.size),
                p.ink_2,
            );
        };
        match i {
            0 => {
                row_label(ui, p, r, "Arabic size", None);
                value(ui, format!("{:.0} pt", typo.arabic_size));
                widgets::slider(
                    ui,
                    p,
                    slider(256.0),
                    ui.id().with("arabic-size"),
                    &mut typo.arabic_size,
                    20.0..=56.0,
                    1.0,
                    0,
                );
            }
            1 => {
                row_label(ui, p, r, "Translation size", None);
                value(ui, format!("{:.0} pt", typo.size));
                widgets::slider(
                    ui,
                    p,
                    slider(256.0),
                    ui.id().with("size"),
                    &mut typo.size,
                    12.0..=32.0,
                    1.0,
                    0,
                );
            }
            2 => {
                row_label(ui, p, r, "Arabic line spacing", None);
                value(ui, format!("{}", (typo.arabic_line * 100.0).round() / 100.0));
                widgets::slider(
                    ui,
                    p,
                    slider(256.0),
                    ui.id().with("arabic-spacing"),
                    &mut typo.arabic_line,
                    1.6..=2.8,
                    0.05,
                    0,
                );
            }
            3 => {
                row_label(ui, p, r, "Translation line spacing", None);
                value(ui, format!("{}", (typo.line_height * 100.0).round() / 100.0));
                widgets::slider(
                    ui,
                    p,
                    slider(256.0),
                    ui.id().with("spacing"),
                    &mut typo.line_height,
                    1.3..=2.0,
                    0.05,
                    0,
                );
            }
            _ => {
                row_label(ui, p, r, "Line length", None);
                let mut chars = (typo.measure * CHARS_PER_EM).round();
                value(ui, format!("{chars:.0} ch"));
                if widgets::slider(
                    ui,
                    p,
                    slider(256.0),
                    ui.id().with("length"),
                    &mut chars,
                    50.0..=100.0,
                    1.0,
                    0,
                ) {
                    typo.measure = chars / CHARS_PER_EM;
                }
            }
        }
    });
    ui.add_space(24.0);

    group_title(ui, p, "Layout");
    rows_card(ui, p, &[44.0, 48.0, 48.0, 44.0, 48.0, 48.0], |ui, i, r| {
        let right = pos2(r.right() - 16.0, r.center().y);
        match i {
            0 => {
                row_label(ui, p, r, "View", None);
                let selected = usize::from(typo.mode == Mode::Reading);
                if let Some(k) = widgets::segmented_fit(
                    ui,
                    p,
                    right,
                    ui.id().with("view"),
                    &["Translation", "Reading"],
                    selected,
                ) {
                    typo.mode = if k == 0 { Mode::Translation } else { Mode::Reading };
                }
            }
            1 => {
                row_label(ui, p, r, "Translation under each ayah", Some("In the translation view"));
                widgets::toggle(ui, p, right, ui.id().with("show-translation"), &mut typo.show_translation);
            }
            2 => {
                row_label(ui, p, r, "Transliteration", Some("⌘R"));
                widgets::toggle(ui, p, right, ui.id().with("translit2"), &mut typo.transliteration);
            }
            3 => {
                row_label(ui, p, r, "Ayah numbers in the margin", None);
                widgets::toggle(ui, p, right, ui.id().with("numbers"), &mut typo.ayah_numbers);
            }
            4 => {
                row_label(ui, p, r, "Surah headings and juz markers", Some("With the Basmala"));
                widgets::toggle(ui, p, right, ui.id().with("headings"), &mut typo.headings);
            }
            _ => {
                row_label(ui, p, r, "Justify the Arabic", Some("Lines fill the column, as on a mushaf page"));
                widgets::toggle(ui, p, right, ui.id().with("justify"), &mut typo.justify);
            }
        }
    });
    ui.add_space(24.0);

    let (r, _) = ui.allocate_exact_size(vec2(width, 32.0), Sense::hover());
    ui.painter().text(
        r.left_center(),
        Align2::LEFT_CENTER,
        "Amiri Quran, Literata and Inter are bundled under the SIL Open Font License.",
        text::SMALL.font(),
        p.ink_3,
    );
    let label = "Reset to Defaults";
    let w = widgets::secondary_button_width(ui, label);
    let b = Rect::from_min_size(pos2(r.right() - w, r.top()), vec2(w, 32.0));
    if widgets::secondary_button(ui, p, b, label, ui.id().with("reset")).clicked() {
        *typo = Typography::default();
    }
}

/// The four theme cards: Light, Sepia, Dark, Match System (`ds-14` theme picker).
fn theme_cards(ui: &mut Ui, settings: &mut Settings, actions: &mut Vec<Action>) {
    let p = settings.theme.palette();
    let width = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(width, 108.0), Sense::hover());
    let gap = (width - 4.0 * 136.0) / 3.0;
    let options: [(Option<Theme>, &str); 4] = [
        (Some(Theme::Light), "Light"),
        (Some(Theme::Sepia), "Sepia"),
        (Some(Theme::Dark), "Dark"),
        (None, "Match System"),
    ];
    for (i, (t, label)) in options.into_iter().enumerate() {
        let card =
            Rect::from_min_size(pos2(row.left() + i as f32 * (136.0 + gap), row.top()), vec2(136.0, 84.0));
        let resp = ui.interact(
            card.union(Rect::from_min_size(card.left_bottom(), vec2(136.0, 24.0))),
            ui.id().with(("theme", i)),
            Sense::click(),
        );
        let on = match t {
            Some(t) => !settings.theme_system && settings.theme == t,
            None => settings.theme_system,
        };
        if on {
            ui.painter().rect_filled(card.expand(5.0), 15.0, p.focus);
            ui.painter().rect_filled(card.expand(2.0), 12.0, p.accent);
        } else if resp.hovered() {
            ui.painter().rect_filled(card.expand(3.0), 13.0, p.fill_strong);
        }
        match t {
            Some(t) => {
                let tp = t.palette();
                ui.painter().rect_filled(card, 10.0, tp.paper);
                let strip = Rect::from_min_size(card.min, vec2(28.0, card.height()));
                ui.painter().rect_filled(
                    strip,
                    egui::CornerRadius { nw: 10, sw: 10, ne: 0, se: 0 },
                    tp.sidebar,
                );
                let x = strip.right() + 12.0;
                ui.painter().text(
                    pos2(x, card.top() + 10.0 + 10.0),
                    Align2::LEFT_CENTER,
                    "Aa",
                    FontId::new(15.0, theme::family(theme::SERIF)),
                    tp.ink,
                );
                for (k, w) in [64.0, 72.0, 56.0].into_iter().enumerate() {
                    let y = card.top() + 10.0 + 20.0 + 6.0 + k as f32 * 10.0;
                    ui.painter().rect_filled(
                        Rect::from_min_size(pos2(x, y), vec2(w, 4.0)),
                        2.0,
                        tp.ink.gamma_multiply(0.55),
                    );
                }
            }
            None => {
                let half = card.width() / 2.0;
                ui.painter().rect_filled(
                    Rect::from_min_size(card.min, vec2(half, card.height())),
                    egui::CornerRadius { nw: 10, sw: 10, ne: 0, se: 0 },
                    crate::theme::LIGHT.paper,
                );
                ui.painter().rect_filled(
                    Rect::from_min_size(pos2(card.left() + half, card.top()), vec2(half, card.height())),
                    egui::CornerRadius { nw: 0, sw: 0, ne: 10, se: 10 },
                    crate::theme::DARK.paper,
                );
            }
        }
        ui.painter().rect_stroke(card, 10.0, Stroke::new(0.5, p.border), egui::StrokeKind::Outside);
        let (font, color) =
            if on { (theme::ui_semibold(text::SMALL.size), p.accent) } else { (text::SMALL.font(), p.ink_2) };
        ui.painter().text(
            pos2(card.center().x, card.bottom() + 8.0 + 8.0),
            Align2::CENTER_CENTER,
            label,
            font,
            color,
        );
        if resp.clicked() {
            match t {
                Some(t) => {
                    settings.theme_system = false;
                    actions.push(Action::SetTheme(t));
                }
                None => settings.theme_system = true,
            }
        }
    }
}
/// Appearance (frame 24): theme, accent colour, the toolbar's translation, the sidebar; names
/// for the highlight colours; motion.
fn appearance(ui: &mut Ui, settings: &mut Settings, actions: &mut Vec<Action>) {
    let p = settings.theme.palette();
    group_title(ui, p, "Appearance");
    rows_card(ui, p, &[44.0, 64.0, 44.0, 44.0], |ui, i, r| match i {
        0 => {
            row_label(ui, p, r, "Theme", None);
            let items: [(IconFn, &str); 4] =
                [(icons::sun, "Light"), (icons::page, "Sepia"), (icons::moon, "Dark"), (icons::auto, "Auto")];
            let selected = if settings.theme_system { 3 } else { settings.theme as usize };
            let right = pos2(r.right() - 16.0, r.center().y);
            if let Some(k) = icon_segmented(ui, p, right, ui.id().with("theme-seg"), &items, selected) {
                if k == 3 {
                    settings.theme_system = true;
                } else {
                    settings.theme_system = false;
                    actions.push(Action::SetTheme(Theme::ALL[k]));
                }
            }
        }
        1 => {
            row_label(ui, p, r, "Accent colour", Some("Tints selection, links and primary buttons"));
            let x0 = r.right() - 16.0 - (5.0 * 52.0 + 4.0 * 4.0);
            for (k, a) in theme::Accent::ALL.into_iter().enumerate() {
                let col = Rect::from_min_size(pos2(x0 + 56.0 * k as f32, r.top() + 10.0), vec2(52.0, 44.0));
                let resp = ui.interact(col, ui.id().with(("accent", k)), Sense::click());
                let c = pos2(col.center().x, col.top() + 11.0);
                let on = settings.accent == a;
                let painter = ui.painter();
                if on {
                    painter.circle_filled(c, 14.5, p.ink);
                    painter.circle_filled(c, 13.0, p.surface);
                } else if resp.hovered() {
                    painter.circle_filled(c, 14.0, p.fill_strong);
                }
                painter.circle_filled(c, 11.0, a.swatch());
                if on {
                    icons::check(painter, Rect::from_center_size(c, vec2(12.0, 12.0)), Color32::WHITE);
                } else {
                    painter.circle_stroke(c, 10.75, Stroke::new(0.5, Color32::from_white_alpha(51)));
                }
                let (font, ink) =
                    if on { (theme::ui_semibold(11.0), p.ink) } else { (theme::ui_medium(11.0), p.ink_3) };
                painter.text(
                    pos2(col.center().x, col.top() + 28.0 + 8.0),
                    Align2::CENTER_CENTER,
                    a.label(),
                    font,
                    ink,
                );
                if resp.clicked() && !on {
                    actions.push(Action::SetAccent(a));
                }
            }
        }
        2 => {
            row_label(ui, p, r, "Show translation in toolbar", None);
            let right = pos2(r.right() - 16.0, r.center().y);
            widgets::toggle(
                ui,
                p,
                right,
                ui.id().with("toolbar-translation"),
                &mut settings.toolbar_translation,
            );
        }
        _ => {
            row_label(ui, p, r, "Sidebar", None);
            let right = pos2(r.right() - 16.0, r.center().y);
            let (_, pick) = widgets::segmented_tight(
                ui,
                p,
                right,
                ui.id().with("sidebar-style"),
                &["Floating", "Attached"],
                usize::from(settings.sidebar_attached),
            );
            if let Some(k) = pick {
                settings.sidebar_attached = k == 1;
            }
        }
    });
    ui.add_space(24.0);

    group_title(ui, p, "Highlight colours");
    settings.highlight_names.resize(6, String::new());
    let colours = quran_core::study::HighlightColor::ALL;
    rows_card(ui, p, &[48.0; 6], |ui, i, r| {
        let c = colours[i];
        let dot = pos2(r.left() + 16.0 + 9.0, r.center().y);
        ui.painter().circle_filled(dot, 9.0, p.swatch(c));
        ui.painter().circle_stroke(dot, 8.75, Stroke::new(0.5, Color32::from_black_alpha(46)));
        let field = Rect::from_min_size(pos2(r.left() + 46.0, r.top() + 8.0), vec2(280.0, 32.0));
        let name = c.name();
        let placeholder = name[..1].to_uppercase() + &name[1..];
        widgets::text_field(
            ui,
            p,
            field,
            &mut settings.highlight_names[i],
            &placeholder,
            None,
            false,
            false,
            egui::Id::new(("highlight-name", i)),
        );
        let key = (i + 1).to_string();
        let kw = widgets::keycap_width(ui.painter(), &key);
        let cap = widgets::keycap(ui.painter(), p, r.right() - 16.0 - kw, r.center().y, &key);
        ui.painter().text(
            pos2(cap.left() - 12.0, r.center().y),
            Align2::RIGHT_CENTER,
            "Key",
            text::SMALL.font(),
            p.ink_3,
        );
    });
    ui.add_space(8.0);
    footnote(ui, p, "Names appear in the highlight menu and the Highlights filter.");
    ui.add_space(24.0);

    group_title(ui, p, "Motion");
    rows_card(ui, p, &[48.0, 44.0], |ui, i, r| {
        let right = pos2(r.right() - 16.0, r.center().y);
        if i == 0 {
            row_label(ui, p, r, "Reduce motion", Some("Follows the system setting by default"));
            let mut on = settings.reduce_motion;
            if widgets::toggle(ui, p, right, ui.id().with("reduce-motion"), &mut on).clicked() {
                actions.push(Action::SetReduceMotion(on));
            }
        } else {
            row_label(ui, p, r, "Flash an ayah after jumping to it", None);
            widgets::toggle(ui, p, right, ui.id().with("flash-jumps"), &mut settings.flash_jumps);
        }
    });
}

/// A note under a group: 12 pt tertiary, aligned with the group's caption.
fn footnote(ui: &mut Ui, p: &Palette, note: &str) {
    let width = ui.available_width() - 4.0;
    let mut job = LayoutJob::simple(note.into(), text::SMALL.font(), p.ink_3, width);
    job.sections[0].format.line_height = Some(16.0);
    let g = ui.painter().layout_job(job);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), g.size().y), Sense::hover());
    ui.painter().galley(pos2(r.left() + 4.0, r.top()), g, p.ink_3);
}

/// A segmented control of icon-and-label segments, drawn with its right edge at `right_center`.
fn icon_segmented(
    ui: &mut Ui,
    p: &Palette,
    right_center: egui::Pos2,
    id: egui::Id,
    items: &[(IconFn, &str)],
    selected: usize,
) -> Option<usize> {
    let widths: Vec<f32> = items
        .iter()
        .enumerate()
        .map(|(i, (_, l))| {
            let font = if i == selected { theme::ui_semibold(13.0) } else { theme::ui_medium(13.0) };
            ui.painter().layout_no_wrap(l.to_string(), font, Color32::PLACEHOLDER).size().x
                + 10.0
                + 16.0
                + 6.0
                + 10.0
        })
        .collect();
    let total = widths.iter().sum::<f32>() + 2.0 * (items.len() as f32 - 1.0) + 4.0;
    let track = Rect::from_min_size(pos2(right_center.x - total, right_center.y - 16.0), vec2(total, 32.0));
    ui.painter().rect_filled(track, 16.0, p.fill);
    let mut x = track.left() + 2.0;
    let mut out = None;
    for (i, ((icon, label), w)) in items.iter().zip(widths).enumerate() {
        let seg = Rect::from_min_size(pos2(x, track.top() + 2.0), vec2(w, 28.0));
        let resp = ui.interact(seg, id.with(i), Sense::click());
        let on = i == selected;
        if on {
            p.shadows.knob.paint(ui.painter(), seg, 14.0);
            ui.painter().rect_filled(seg, 14.0, p.raised);
        } else if let Some(fill) = widgets::hover_fill(ui, &resp, p, false) {
            ui.painter().rect_filled(seg, 14.0, fill);
        }
        let ink = if on { p.ink } else { p.ink_2 };
        let r = Rect::from_min_size(pos2(seg.left() + 10.0, seg.center().y - 8.0), vec2(16.0, 16.0));
        icons::heavy(|| icon(ui.painter(), r, ink));
        let font = if on { theme::ui_semibold(13.0) } else { theme::ui_medium(13.0) };
        ui.painter().text(pos2(r.right() + 6.0, seg.center().y), Align2::LEFT_CENTER, *label, font, ink);
        if resp.clicked() && !on {
            out = Some(i);
        }
        x += w + 2.0;
    }
    out
}

/// About (frame 28): the app, its version, the translations' and fonts' licences.
fn about(ui: &mut Ui, p: &Palette) {
    let width = ui.available_width();
    let (r, _) = ui.allocate_exact_size(vec2(width, 240.0), Sense::hover());
    let painter = ui.painter().clone();
    let cx = r.center().x;
    crate::onboarding::app_icon(
        &painter,
        Rect::from_center_size(pos2(cx, r.top() + 8.0 + 44.0), vec2(88.0, 88.0)),
    );
    let centred = |top: f32, lh: f32, s: &str, font: FontId, ink: Color32, tracking: f32| {
        let size = font.size;
        let reading = matches!(&font.family, egui::FontFamily::Name(n) if n.starts_with("serif"));
        let g = widgets::line(&painter, s, font, ink, tracking);
        let x = cx - g.size().x / 2.0;
        widgets::paint_on_baseline(&painter, x, widgets::css_baseline(top, lh, size, reading), g, ink);
    };
    centred(
        r.top() + 100.0,
        40.0,
        "Quran",
        FontId::new(32.0, theme::family(theme::SERIF_DISPLAY)),
        p.ink,
        -0.015,
    );
    let arch = match std::env::consts::ARCH {
        "aarch64" if cfg!(target_os = "macos") => "Apple Silicon",
        "x86_64" if cfg!(target_os = "macos") => "Intel",
        other => other,
    };
    let version = format!("Version {} · {arch}", env!("CARGO_PKG_VERSION"));
    centred(r.top() + 144.0, 20.0, &version, theme::ui_font(13.0), p.ink_2, 0.0);
    centred(
        r.top() + 168.0,
        20.0,
        "A fast, native Quran reader for the Mac.",
        theme::ui_font(13.0),
        p.ink_3,
        0.0,
    );
    let tanzil = Button::new(Kind::Secondary, "Tanzil Project").trailing_icon(icons::external);
    let wa = tanzil.width(&painter);
    let cy = r.top() + 200.0 + 16.0;
    if tanzil.show_left(ui, p, cx - wa / 2.0, cy, ui.id().with("tanzil")).1.clicked() {
        crate::app::open_url("https://tanzil.net");
    }
    ui.add_space(24.0);

    let licences = |ui: &mut Ui, rows: &[(&str, &str)]| {
        rows_card(ui, p, &vec![40.0; rows.len()], |ui, i, r| {
            let (k, v) = rows[i];
            let painter = ui.painter();
            painter.text(
                pos2(r.left() + 16.0, r.center().y),
                Align2::LEFT_CENTER,
                k,
                theme::ui_bold(12.0),
                p.ink,
            );
            painter.text(
                pos2(r.left() + 166.0, r.center().y),
                Align2::LEFT_CENTER,
                v,
                text::SMALL.font(),
                p.ink_2,
            );
        });
    };
    group_title(ui, p, "Text");
    licences(
        ui,
        &[
            ("Quran text", "Tanzil Uthmani 1.1 · © Tanzil Project · CC BY 3.0, verbatim"),
            ("Metadata", "Tanzil quran-data · surahs, juz, hizb, pages · CC BY 3.0"),
            ("Translations", "Tanzil translations · non-commercial use, verbatim"),
        ],
    );
    ui.add_space(8.0);
    footnote(
        ui,
        p,
        "The Quran text is the Tanzil Project's, unchanged: it can be copied and distributed verbatim, with its source credited and linked (tanzil.net).",
    );
    ui.add_space(24.0);
    group_title(ui, p, "Fonts");
    licences(
        ui,
        &[
            ("Amiri Quran · Amiri", "SIL Open Font License 1.1 (Khaled Hosny)"),
            ("Noto Naskh Arabic", "SIL Open Font License 1.1"),
            ("Literata · Inter", "SIL Open Font License 1.1"),
        ],
    );
    ui.add_space(24.0);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 16.0), Sense::hover());
    ui.painter().text(
        r.center(),
        Align2::CENTER_CENTER,
        "No accounts, no tracking, no network unless you download a translation.",
        text::SMALL.font(),
        p.ink_3,
    );
}

/// The toolbar's "Aa" popover: the settings people change while reading.
pub fn quick_text_settings(ui: &mut Ui, settings: &mut Settings, actions: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        for t in Theme::ALL {
            let p = t.palette();
            let (r, resp) = ui.allocate_exact_size(vec2(76.0, 44.0), egui::Sense::click());
            ui.painter().rect_filled(r, egui::CornerRadius::same(8), p.paper);
            let stroke = if settings.theme == t {
                egui::Stroke::new(2.0, p.accent)
            } else {
                egui::Stroke::new(1.0, p.separator)
            };
            ui.painter().rect_stroke(r, egui::CornerRadius::same(8), stroke, egui::StrokeKind::Inside);
            ui.painter().text(
                r.center(),
                Align2::CENTER_CENTER,
                t.label(),
                egui::FontId::new(13.0, theme::family(theme::SERIF)),
                p.ink,
            );
            if resp.clicked() {
                actions.push(Action::SetTheme(t));
            }
        }
    });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.button(RichText::new("A−").font(theme::ui_medium(13.0))).on_hover_text("Smaller (⌘−)").clicked()
        {
            actions.push(Action::FontSize(-1.0));
        }
        ui.label(format!("{:.0} / {:.0} pt", settings.typo.arabic_size, settings.typo.size));
        if ui.button(RichText::new("A+").font(theme::ui_medium(15.0))).on_hover_text("Bigger (⌘+)").clicked()
        {
            actions.push(Action::FontSize(1.0));
        }
    });
    ui.add_space(4.0);
    let mut reading = settings.typo.mode == Mode::Reading;
    if ui.checkbox(&mut reading, "Reading view — Arabic only (⌘L)").changed() {
        actions.push(Action::ToggleMode);
    }
    let mut translit = settings.typo.transliteration;
    if ui.checkbox(&mut translit, "Transliteration (⌘R)").changed() {
        actions.push(Action::ToggleTransliteration);
    }
    let mut parallel = settings.parallel_on;
    if ui.checkbox(&mut parallel, "More translations (⇧⌘P)").changed() {
        actions.push(Action::ToggleParallel);
    }
    ui.checkbox(&mut settings.typo.justify, "Justify the Arabic");
    ui.separator();
    if ui.button("More settings…").clicked() {
        actions.push(Action::OpenSettings(SettingsTab::Reading));
        ui.close();
    }
}
