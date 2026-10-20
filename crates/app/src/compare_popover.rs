//! Compare as a popover: the selected ayah in Arabic and in every installed translation, the
//! current one first, hanging from the ayah in the reader.
//!
//! Ported from bible-rs (MIT).

use eframe::egui::{self, FontId, Key, Order, Rect, Sense, Stroke, pos2, vec2};

use quran_core::AyahRef;
use quran_core::library::Library;

use crate::actions::Action;
use crate::app::Settings;
use crate::icons;
use crate::pages::text;
use crate::theme::{self, Palette};
use crate::widgets::{self, Button, Kind, css_baseline, flow, paint_runs};

const WIDTH: f32 = 400.0;
const BEAK: egui::Vec2 = vec2(18.0, 9.0);
/// The text's measure in a row: 400 − 2×8 list padding − 2×8 row padding.
const MEASURE: f32 = 368.0;
/// A row's label line above its text.
const LABEL: f32 = 18.0;

pub struct ComparePopover {
    pub verse: AyahRef,
}

impl ComparePopover {
    pub fn new(verse: AyahRef) -> Self {
        Self { verse }
    }

    /// Draws the popover under `anchor` (the selected verse on screen); returns true to close.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        p: &Palette,
        library: &Library,
        settings: &Settings,
        anchor: Rect,
        actions: &mut Vec<Action>,
    ) -> bool {
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape)) {
            return true;
        }
        let screen = ctx.content_rect();
        let v = self.verse;
        // The Arabic, then the translations, the reading one first; (id, label, text, rtl).
        let mut rows: Vec<(String, String, String, bool)> =
            vec![(String::new(), "ARABIC".into(), quran_core::text::quran().arabic(v).to_string(), true)];
        let mut ids: Vec<&str> = vec![settings.translation.as_str()];
        ids.extend(
            library.entries().iter().map(|e| e.meta.id.as_str()).filter(|id| *id != settings.translation),
        );
        for id in ids {
            let Some(e) = library.get(id) else { continue };
            let Ok(t) = library.load(id) else { continue };
            rows.push((id.to_string(), e.meta.abbrev.clone(), t.get(v).to_string(), e.meta.rtl()));
        }
        let serif = FontId::new(14.0, theme::family(theme::SERIF));
        let arabic = theme::quran_font(20.0);
        let naskh = FontId::new(15.0, theme::family(theme::NASKH));
        let rtl_font = |id: &str| if id.is_empty() { arabic.clone() } else { naskh.clone() };
        let rtl_lh = |id: &str| if id.is_empty() { 40.0 } else { 28.0 };

        // Rows that fit under the ayah (or above it), the rest summarised in the footer.
        let painter =
            ctx.layer_painter(egui::LayerId::new(Order::Foreground, egui::Id::new("compare-measure")));
        let heights: Vec<f32> = rows
            .iter()
            .map(|(id, _, t, rtl)| {
                if *rtl {
                    16.0 + LABEL
                        + rtl_lh(id)
                            * widgets::rtl_flow(&painter, t, &rtl_font(id), MEASURE, 3).len().max(1) as f32
                } else {
                    16.0 + LABEL + 20.0 * flow(&painter, t, &serif, 10.0, MEASURE, 4).len().max(1) as f32
                }
            })
            .collect();
        let chrome = 60.0 + 12.0 + 46.0;
        let below = screen.bottom() - (anchor.bottom() + 3.0 + BEAK.y) - 16.0;
        let above = anchor.top() - 3.0 - BEAK.y - (screen.top() + 52.0) - 8.0;
        // A tall selection leaves little room either side: then the popover may cover it.
        let room = below.max(above).max(screen.height() * 0.7) - chrome;
        let mut shown = 0;
        let mut list_h = 0.0;
        for h in &heights {
            if list_h + h > room && shown > 0 {
                break;
            }
            list_h += h;
            shown += 1;
        }
        let height = 60.0 + 12.0 + list_h + 46.0;
        let downward = below >= height || below >= above;
        let left =
            (anchor.center().x - WIDTH / 2.0).clamp(screen.left() + 12.0, screen.right() - 12.0 - WIDTH);
        let top =
            if downward { anchor.bottom() + 3.0 + BEAK.y } else { anchor.top() - 3.0 - BEAK.y - height };
        let fits = top >= screen.top() + 52.0 && top + height <= screen.bottom() - 8.0;
        let top = top.clamp(screen.top() + 60.0, (screen.bottom() - 16.0 - height).max(screen.top() + 60.0));
        let rect = Rect::from_min_size(pos2(left, top), vec2(WIDTH, height));

        let mut close = false;
        let area = egui::Area::new(egui::Id::new("compare-popover"))
            .order(Order::Foreground)
            .fixed_pos(rect.min)
            .show(ctx, |ui| {
                ui.allocate_rect(Rect::from_min_size(rect.min, rect.size()), Sense::hover());
                let painter = ui.painter().clone();
                p.shadows.popover.paint(&painter, rect, 16.0);
                painter.rect_filled(rect, 16.0, p.surface);
                // The beak, toward the ayah (when the popover sits beside it).
                let tip_x = (rect.left() + 60.0).max(anchor.left() + 24.0).min(rect.right() - 24.0);
                let (edge, dir) =
                    if downward { (rect.top() + 0.5, -1.0) } else { (rect.bottom() - 0.5, 1.0) };
                let tip = pos2(tip_x, edge + dir * BEAK.y);
                let l = pos2(tip_x - BEAK.x / 2.0, edge);
                let r = pos2(tip_x + BEAK.x / 2.0, edge);
                if fits {
                    painter.add(egui::Shape::convex_polygon(vec![l, tip, r], p.surface, Stroke::NONE));
                    painter.add(egui::Shape::line(vec![l, tip, r], Stroke::new(0.75, p.border)));
                }

                // Header.
                let x = rect.left() + 16.0;
                text(&painter, x, rect.top() + 14.0, 20.0, &v.long(), theme::ui_semibold(15.0), p.ink, false);
                let n = rows.len() - 1;
                let sub =
                    format!("{n} {} · current first", if n == 1 { "translation" } else { "translations" });
                text(&painter, x, rect.top() + 34.0, 16.0, &sub, theme::ui_font(12.0), p.ink_2, false);
                let close_r = Rect::from_min_size(
                    pos2(rect.right() - 12.0 - 28.0, rect.top() + 18.0),
                    vec2(28.0, 28.0),
                );
                if widgets::round_button(ui, p, close_r, icons::close, ui.id().with("compare-close"))
                    .clicked()
                {
                    close = true;
                }
                let copy_r = close_r.translate(vec2(-32.0, 0.0));
                if widgets::round_button(ui, p, copy_r, icons::copy, ui.id().with("compare-copy")).clicked() {
                    let all: Vec<String> = rows.iter().map(|(_, a, t, _)| format!("{a}: {t}")).collect();
                    ui.ctx().copy_text(format!("{}\n{}", v.long(), all.join("\n")));
                    actions.push(Action::Toast(format!("Copied {} in {n} translations", v.long())));
                }
                painter.hline(rect.x_range(), rect.top() + 59.5, Stroke::new(1.0, p.separator));

                // Rows.
                let mut y = rect.top() + 60.0 + 6.0;
                for ((id, abbrev, verse, rtl), h) in rows.iter().zip(&heights).take(shown) {
                    let row = Rect::from_min_size(pos2(rect.left() + 8.0, y), vec2(WIDTH - 16.0, *h));
                    let resp = ui.interact(row, ui.id().with(("compare-row", id)), Sense::click());
                    let current = *id == settings.translation;
                    if current {
                        painter.rect_filled(row, 8.0, p.accent_soft);
                    } else if let Some(fill) = widgets::hover_fill(ui, &resp, p, false) {
                        painter.rect_filled(row, 8.0, fill);
                    }
                    let ink = if current { p.accent } else { p.ink_2 };
                    text(
                        &painter,
                        row.left() + 8.0,
                        row.top() + 8.0,
                        14.0,
                        &abbrev.to_uppercase(),
                        theme::ui_bold(10.0),
                        ink,
                        false,
                    );
                    let text_top = row.top() + 8.0 + LABEL;
                    if *rtl {
                        let lines = widgets::rtl_flow(&painter, verse, &rtl_font(id), MEASURE, 3);
                        let lh = rtl_lh(id);
                        widgets::paint_rtl(&painter, &lines, row.left() + 8.0 + MEASURE, text_top, lh, p.ink);
                    } else {
                        for (k, l) in flow(&painter, verse, &serif, 10.0, MEASURE, 4).iter().enumerate() {
                            let base = css_baseline(text_top + 20.0 * k as f32, 20.0, 14.0, true);
                            paint_runs(&painter, row.left() + 8.0, base, l, p.ink);
                        }
                    }
                    if resp.clicked() && !current && !id.is_empty() {
                        actions.push(Action::SetTranslation(id.clone()));
                    }
                    y += h;
                }

                // Footer: what didn't fit, and Open in Parallel.
                let footer = Rect::from_min_max(pos2(rect.left(), rect.bottom() - 46.0), rect.max);
                painter.hline(rect.x_range(), footer.top() + 0.5, Stroke::new(1.0, p.separator));
                let rest: Vec<&str> = rows.iter().skip(shown).map(|(_, a, _, _)| a.as_str()).collect();
                if !rest.is_empty() {
                    let more = crate::pages::elide(
                        &painter,
                        &format!("+ {}", rest.join(", ")),
                        &theme::ui_font(12.0),
                        180.0,
                    );
                    text(&painter, x, footer.top() + 14.0, 16.0, &more, theme::ui_font(12.0), p.ink_3, false);
                }
                let (_, open) = Button::new(Kind::Secondary, "Show Under Each Ayah")
                    .small()
                    .icon(icons::columns)
                    .show_right(
                        ui,
                        p,
                        rect.right() - 12.0,
                        footer.top() + 10.0 + 12.0,
                        ui.id().with("compare-parallel"),
                    );
                if open.clicked() {
                    for (id, _, _, _) in rows.iter().take(shown).skip(2) {
                        actions.push(Action::AddParallel(id.clone()));
                    }
                    if !settings.parallel_on {
                        actions.push(Action::ToggleParallel);
                    }
                    close = true;
                }
            });
        // Clicking elsewhere closes it.
        if ctx.input(|i| i.pointer.any_pressed())
            && let Some(pos) = ctx.input(|i| i.pointer.interact_pos())
            && !area.response.rect.expand(4.0).contains(pos)
            && !anchor.contains(pos)
        {
            close = true;
        }
        close
    }
}
