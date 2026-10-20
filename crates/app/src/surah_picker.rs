//! The surah picker under the toolbar's location button: a Surahs | Juz switch over a grid of
//! menu cells — the 114 surahs in three columns, or the thirty juz — and, for a surah, the grid
//! of its ayahs. The surah (or juz) being read is selected.
//!
//! Arrow keys move a highlight through the grid, return chooses (a surah opens at its start;
//! ⌥-return or the arrow at a cell's end shows its ayahs), backspace goes back and escape closes.

use eframe::egui::{
    self, Align2, Id, Key, Modifiers, Rect, Response, ScrollArea, Sense, Ui, Vec2, pos2, vec2,
};
use quran_core::meta::{AyahRef, Juz, SurahId};

use crate::actions::Action;
use crate::menu::{self, Cell, ITEM_HEIGHT, SEPARATOR_HEIGHT};
use crate::theme::{self, Palette, text};

const POPUP: &str = "surah-picker";
const SURAH_COLUMNS: usize = 3;
const JUZ_COLUMNS: usize = 3;
const AYAH_COLUMNS: usize = 10;
const CELL_W: f32 = 196.0;
const MAX_HEIGHT: f32 = 460.0;

#[derive(Clone, Copy, Default, PartialEq)]
enum Grid {
    #[default]
    Surahs,
    Juz,
    /// A surah's ayahs.
    Ayahs(u8),
}

#[derive(Clone, Copy, Default)]
struct State {
    grid: Grid,
    cursor: Option<usize>,
    /// Scroll the highlighted cell into view (after a key).
    reveal: bool,
}

#[derive(Default)]
struct Keys {
    dx: isize,
    dy: isize,
    enter: bool,
    alt_enter: bool,
    back: bool,
}

/// Whether the picker is open (it takes the arrow keys, return and escape then).
pub fn is_open(ctx: &egui::Context) -> bool {
    egui::Popup::is_id_open(ctx, Id::new(POPUP))
}

/// The picker, opening from `anchor`; `here` is the ayah being read.
pub fn show(anchor: &Response, p: &Palette, here: AyahRef, actions: &mut Vec<Action>) {
    let state_id = Id::new(POPUP).with("state");
    let shown = menu::popup(anchor, p)
        .id(Id::new(POPUP))
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            let mut state: State = ui.data(|d| d.get_temp(state_id)).unwrap_or_default();
            let keys = ui.input_mut(|i| Keys {
                dx: i.consume_key(Modifiers::NONE, Key::ArrowRight) as isize
                    - i.consume_key(Modifiers::NONE, Key::ArrowLeft) as isize,
                dy: i.consume_key(Modifiers::NONE, Key::ArrowDown) as isize
                    - i.consume_key(Modifiers::NONE, Key::ArrowUp) as isize,
                alt_enter: i.consume_key(Modifiers::ALT, Key::Enter),
                enter: i.consume_key(Modifiers::NONE, Key::Enter),
                back: i.consume_key(Modifiers::NONE, Key::Backspace),
            });
            let close = grid(ui, p, here, &keys, &mut state, actions);
            ui.data_mut(|d| d.insert_temp(state_id, state));
            if close {
                ui.close();
            }
        });
    if shown.is_none() {
        anchor.ctx.data_mut(|d| d.remove::<State>(state_id));
    }
}

/// The switch, then the grid. Returns whether a choice was made (the popup then closes).
fn grid(
    ui: &mut Ui,
    p: &Palette,
    here: AyahRef,
    keys: &Keys,
    state: &mut State,
    actions: &mut Vec<Action>,
) -> bool {
    let width = SURAH_COLUMNS as f32 * CELL_W;
    ui.set_width(width);
    let (n, columns, current) = match state.grid {
        Grid::Surahs => (114usize, SURAH_COLUMNS, here.surah as usize - 1),
        Grid::Juz => (30, JUZ_COLUMNS, here.juz() as usize - 1),
        Grid::Ayahs(s) => (
            SurahId(s).ayahs() as usize,
            AYAH_COLUMNS,
            if s == here.surah { here.ayah as usize - 1 } else { 0 },
        ),
    };
    // The header: the switch, or "‹ Surahs" over a surah's ayahs.
    match state.grid {
        Grid::Ayahs(s) => {
            let (r, resp) = ui.allocate_exact_size(vec2(width, ITEM_HEIGHT), Sense::click());
            if let Some(fill) = crate::widgets::hover_fill(ui, &resp, p, false) {
                ui.painter().rect_filled(r, theme::menu::ITEM_RADIUS, fill);
            }
            let x = menu::text_left(r);
            crate::icons::chevron_left(
                ui.painter(),
                Rect::from_min_size(pos2(x, r.center().y - 8.0), vec2(16.0, 16.0)),
                p.ink_2,
            );
            let sid = SurahId(s);
            ui.painter().text(
                pos2(x + 20.0, r.center().y),
                Align2::LEFT_CENTER,
                format!("{} · {}", sid.0, sid.name()),
                text::UI_STRONG.font(),
                p.ink,
            );
            ui.painter().text(
                pos2(r.right() - theme::menu::ITEM_PADDING, r.center().y),
                Align2::RIGHT_CENTER,
                sid.arabic(),
                theme::arabic_font(16.0),
                p.ink_2,
            );
            if resp.clicked() || keys.back {
                *state = State { grid: Grid::Surahs, cursor: Some(s as usize - 1), reveal: true };
                return false;
            }
        }
        _ => {
            let idx = usize::from(state.grid == Grid::Juz);
            if let Some(i) = crate::widgets::segmented(ui, p, &["Surahs", "Juz"], idx) {
                *state = State { grid: [Grid::Surahs, Grid::Juz][i], cursor: None, reveal: true };
                return false;
            }
        }
    }
    let (sep, _) = ui.allocate_exact_size(vec2(width, SEPARATOR_HEIGHT), Sense::hover());
    menu::paint_separator(ui.painter(), p, sep.left(), sep.right(), sep.top());

    // Keys move the highlight, starting from the current place.
    if keys.dx != 0 || keys.dy != 0 {
        let from = state.cursor.unwrap_or(current) as isize;
        let to = (from + keys.dx + keys.dy * columns as isize).clamp(0, n as isize - 1);
        state.cursor = Some(to as usize);
        state.reveal = true;
    }
    let mut chosen: Option<Action> = None;
    let mut open_ayahs: Option<u8> = None;
    if let Some(c) = state.cursor.or(Some(current)) {
        if keys.enter {
            chosen = Some(choose(state.grid, c));
        } else if keys.alt_enter && state.grid == Grid::Surahs {
            open_ayahs = Some(c as u8 + 1);
        }
    }
    let cell_w = width / columns as f32;
    let cell_h = if matches!(state.grid, Grid::Ayahs(_)) { 32.0 } else { 40.0 };
    let rows = n.div_ceil(columns);
    let reveal = std::mem::take(&mut state.reveal);
    let moved = ui.input(|i| i.pointer.delta() != Vec2::ZERO);
    ScrollArea::vertical().max_height(MAX_HEIGHT).id_salt(("picker", matches!(state.grid, Grid::Juz))).show(
        ui,
        |ui| {
            let (area, _) = ui.allocate_exact_size(vec2(width, rows as f32 * cell_h), Sense::hover());
            let mut hovered = None;
            for i in 0..n {
                let (row, col) = (i / columns, i % columns);
                let r = Rect::from_min_size(
                    pos2(area.left() + col as f32 * cell_w, area.top() + row as f32 * cell_h),
                    vec2(cell_w, cell_h),
                );
                if !(ui.is_rect_visible(r) || reveal && state.cursor == Some(i)) {
                    continue;
                }
                let resp = ui.interact(r, ui.id().with(("cell", i)), Sense::click());
                if resp.hovered() {
                    hovered = Some(i);
                }
                let lit = state.cursor == Some(i);
                let selected = i == current && (state.grid != Grid::Ayahs(0));
                match state.grid {
                    Grid::Ayahs(_) => {
                        let label = (i + 1).to_string();
                        menu::paint_cell(
                            ui.painter(),
                            p,
                            r.shrink2(vec2(1.0, 2.0)),
                            Cell {
                                icon: None,
                                label: &label,
                                centred: true,
                                selected,
                                lit,
                                pressed: resp.is_pointer_button_down_on(),
                            },
                        );
                    }
                    Grid::Surahs | Grid::Juz => {
                        let cell = r.shrink2(vec2(1.0, 2.0));
                        let fill = if selected {
                            p.selection
                        } else if resp.is_pointer_button_down_on() {
                            p.fill_strong
                        } else if lit {
                            p.fill
                        } else {
                            egui::Color32::TRANSPARENT
                        };
                        ui.painter().rect_filled(cell, theme::menu::ITEM_RADIUS, fill);
                        let n1 = i as u8 + 1;
                        let badge = pos2(cell.left() + 8.0 + 11.0, cell.center().y);
                        crate::widgets::star_badge(ui.painter(), p, badge, 11.0, n1, false);
                        let x = badge.x + 11.0 + 9.0;
                        let (name, sub) = if state.grid == Grid::Juz {
                            let j = Juz(n1);
                            (format!("Juz {}", j.0), j.name().to_string())
                        } else {
                            let s = SurahId(n1);
                            (s.name().to_string(), format!("{} · {}", s.english(), s.ayahs()))
                        };
                        let room = cell.right() - x - 22.0;
                        let font = theme::ui_medium(12.5);
                        let name = crate::pages::elide(ui.painter(), &name, &font, room);
                        ui.painter().text(
                            pos2(x, cell.center().y - 7.0),
                            Align2::LEFT_CENTER,
                            name,
                            font,
                            p.ink,
                        );
                        let sf = theme::ui_font(10.5);
                        let sub = crate::pages::elide(ui.painter(), &sub, &sf, room);
                        ui.painter().text(
                            pos2(x, cell.center().y + 8.0),
                            Align2::LEFT_CENTER,
                            sub,
                            sf,
                            p.ink_3,
                        );
                        // A surah's ayahs, from the arrow at its end.
                        if state.grid == Grid::Surahs && (resp.hovered() || lit) {
                            let arrow = Rect::from_center_size(
                                pos2(cell.right() - 12.0, cell.center().y),
                                vec2(18.0, 18.0),
                            );
                            let aresp = ui
                                .interact(arrow, ui.id().with(("ayahs", i)), Sense::click())
                                .on_hover_text("Ayahs (⌥↩)");
                            if aresp.hovered() {
                                ui.painter().rect_filled(arrow, 5.0, p.fill_strong);
                            }
                            crate::icons::disclosure(
                                ui.painter(),
                                Rect::from_center_size(arrow.center(), vec2(12.0, 12.0)),
                                p.ink_2,
                            );
                            if aresp.clicked() {
                                open_ayahs = Some(n1);
                                continue;
                            }
                        }
                    }
                }
                if lit && reveal {
                    ui.scroll_to_rect(r, Some(egui::Align::Center));
                }
                if resp.clicked() && open_ayahs.is_none() {
                    chosen = Some(choose(state.grid, i));
                }
            }
            // The current place in view when the picker opens.
            if state.cursor.is_none()
                && !reveal
                && ui.ctx().data(|d| d.get_temp::<bool>(Id::new(POPUP).with("opened"))).is_none()
            {
                let (row, col) = (current / columns, current % columns);
                let r = Rect::from_min_size(
                    pos2(area.left() + col as f32 * cell_w, area.top() + row as f32 * cell_h),
                    vec2(cell_w, cell_h),
                );
                ui.scroll_to_rect(r, Some(egui::Align::Center));
                ui.ctx().data_mut(|d| d.insert_temp(Id::new(POPUP).with("opened"), true));
            }
            if moved {
                state.cursor = hovered.or(state.cursor);
            }
        },
    );
    if let Some(s) = open_ayahs {
        *state = State { grid: Grid::Ayahs(s), cursor: None, reveal: true };
        return false;
    }
    if let Some(a) = chosen {
        ui.ctx().data_mut(|d| d.remove::<bool>(Id::new(POPUP).with("opened")));
        actions.push(a);
        return true;
    }
    false
}

fn choose(grid: Grid, i: usize) -> Action {
    match grid {
        Grid::Surahs => Action::Jump { to: SurahId(i as u8 + 1).first_ayah(), flash: false },
        Grid::Juz => Action::Jump { to: Juz(i as u8 + 1).start(), flash: false },
        Grid::Ayahs(s) => Action::Jump { to: AyahRef::new(s, i as u16 + 1), flash: i > 0 },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices() {
        assert_eq!(choose(Grid::Surahs, 1), Action::Jump { to: AyahRef::new(2, 1), flash: false });
        assert_eq!(choose(Grid::Juz, 29), Action::Jump { to: AyahRef::new(78, 1), flash: false });
        assert_eq!(choose(Grid::Ayahs(2), 254), Action::Jump { to: AyahRef::new(2, 255), flash: true });
    }
}
