//! Library › Translations: every installed translation, a row each, with an inspector for the
//! selected one — and the catalogue of the Tanzil Project's translations to add, grouped by
//! language.
//!
//! Ported from bible-rs (MIT).

use std::sync::Arc;

use eframe::egui::{self, FontId, Rect, Sense, Ui, pos2, vec2};
use quran_core::AyahRef;
use quran_core::library::Entry;

use super::{Page, PageEnv, elide, group_label, text};
use crate::actions::Action;
use crate::icons;
use crate::theme::{self, Palette};
use crate::widgets::{self, BadgeKind, Button, Kind, css_baseline, flow, paint_runs};

const ROW: f32 = 60.0;
const ROW_GAP: f32 = 2.0;
const INSPECTOR: f32 = 300.0;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Filter {
    #[default]
    Installed,
    Bundled,
    Added,
    /// The catalogue.
    More,
}

#[derive(Default)]
pub struct State {
    filter: Filter,
    /// The row shown in the inspector (the reading translation until another is picked).
    pub selected: Option<String>,
}

fn description(e: &Entry) -> String {
    let m = &e.meta;
    let mut parts = vec![m.language.clone()];
    if !m.translator.is_empty() && m.translator != m.name {
        parts.push(m.translator.clone());
    }
    parts.push(if e.is_bundled() { "built in".into() } else { m.source.clone() });
    parts.join(" · ")
}

impl State {
    /// Draws the page from `content`'s top-left; returns the bottom of what was drawn.
    pub fn show(&mut self, ui: &mut Ui, content: Rect, env: &PageEnv<'_>, actions: &mut Vec<Action>) -> f32 {
        let p = env.settings.theme.palette();
        let painter = ui.painter().clone();
        let (left, right, top) = (content.left(), content.right(), content.top());
        let entries: Vec<Arc<Entry>> = env.library.entries().to_vec();
        let nb = entries.iter().filter(|e| e.is_bundled()).count();
        let na = entries.len() - nb;
        let available = match env.catalog {
            Some(Ok(list)) => Some(list.len()),
            _ => None,
        };

        let subtitle = format!("{} installed · {nb} built in, {na} added", entries.len());
        super::title(&painter, p, left, top, "Translations", &subtitle);
        let cy = top + 26.0 + 16.0;
        let (import, resp) = Button::new(Kind::Secondary, "Import File…").icon(icons::upload).show_right(
            ui,
            p,
            right,
            cy,
            ui.id().with("import"),
        );
        if resp.clicked() {
            actions.push(Action::ImportTranslation);
        }
        let (_, resp) = Button::new(Kind::Primary, "Get More").icon(icons::download).show_right(
            ui,
            p,
            import.left() - 8.0,
            cy,
            ui.id().with("get-more"),
        );
        if resp.clicked() {
            self.filter = Filter::More;
        }

        // Tabs.
        let y = top + 58.0 + 20.0;
        let mut x = left;
        let more_count = available.map(|n| n.to_string());
        for (f, label, n) in [
            (Filter::Installed, "Installed", Some(entries.len().to_string())),
            (Filter::Bundled, "Built in", Some(nb.to_string())),
            (Filter::Added, "Added", Some(na.to_string())),
            (Filter::More, "Get More", more_count),
        ] {
            let on = self.filter == f;
            let w = widgets::pill_tab_width(&painter, label, n.as_deref(), on, false);
            let r = Rect::from_min_size(pos2(x, y), vec2(w, 28.0));
            if widgets::pill_tab(ui, p, r, label, n.as_deref(), None, on, ui.id().with(("tab", label)))
                .clicked()
            {
                self.filter = f;
            }
            x = r.right() + 4.0;
        }
        let body = y + 28.0 + 20.0;
        if self.filter == Filter::More {
            return self.catalog(
                ui,
                p,
                Rect::from_min_max(pos2(left, body), pos2(right, body)),
                env,
                actions,
            );
        }

        // The list.
        let q = env.query.trim().to_lowercase();
        let shown: Vec<Arc<Entry>> = entries
            .iter()
            .filter(|e| match self.filter {
                Filter::Bundled => e.is_bundled(),
                Filter::Added => !e.is_bundled(),
                _ => true,
            })
            .filter(|e| {
                q.is_empty()
                    || e.meta.abbrev.to_lowercase().contains(&q)
                    || e.meta.name.to_lowercase().contains(&q)
                    || description(e).to_lowercase().contains(&q)
            })
            .cloned()
            .collect();
        let selected = self.selected.clone().unwrap_or_else(|| env.settings.translation.clone());
        let list_w = (right - left - 24.0 - INSPECTOR).max(320.0);
        let mut y = body;
        for e in &shown {
            let row = Rect::from_min_size(pos2(left, y), vec2(list_w, ROW));
            self.row(ui, p, row, e, env, &selected, actions);
            y += ROW + ROW_GAP;
        }
        if shown.is_empty() {
            let msg = if self.filter == Filter::Added {
                "Nothing added yet. Get More lists over a hundred translations in forty languages."
            } else {
                "No translations match."
            };
            text(&painter, left + 12.0, body + 20.0, 20.0, msg, theme::ui_font(13.0), p.ink_3, false);
            y = body + 60.0;
        }
        let list_bottom = y;
        let inspector_bottom = match entries.iter().find(|e| e.meta.id == selected) {
            Some(e) => self.inspector(ui, p, pos2(left + list_w + 24.0, body), e, env, actions),
            None => body,
        };
        list_bottom.max(inspector_bottom)
    }

    #[allow(clippy::too_many_arguments)]
    fn row(
        &mut self,
        ui: &mut Ui,
        p: &Palette,
        row: Rect,
        e: &Entry,
        env: &PageEnv<'_>,
        selected: &str,
        actions: &mut Vec<Action>,
    ) {
        let m = &e.meta;
        let id = ui.id().with(("translation-row", &m.id));
        let resp = ui.interact(row, id, Sense::click());
        let more = Rect::from_min_size(pos2(row.right() - 12.0 - 28.0, row.top() + 16.0), vec2(28.0, 28.0));
        let more_resp = ui.interact(more, id.with("more"), Sense::click());
        let is_selected = m.id == selected;
        let is_default = m.id == env.settings.translation;
        let painter = ui.painter().clone();
        if is_selected {
            painter.rect_filled(row, 12.0, p.accent_soft);
        } else if let Some(fill) = widgets::hover_fill(ui, &resp, p, more_resp.hovered()) {
            painter.rect_filled(row, 12.0, fill);
        }
        // Tile: the language's code.
        let tile = Rect::from_min_size(pos2(row.left() + 12.0, row.top() + 10.0), vec2(48.0, 40.0));
        painter.rect_filled(tile, 10.0, if is_selected { p.raised } else { p.fill });
        let code = m.id.split('.').next().unwrap_or("").to_uppercase();
        let ink = if is_selected || is_default { p.accent } else { p.ink_2 };
        let g = widgets::line(&painter, &code, theme::ui_bold(12.0), ink, 0.0);
        let x = tile.center().x - g.size().x / 2.0;
        widgets::paint_on_baseline(&painter, x, css_baseline(tile.top() + 12.0, 16.0, 12.0, false), g, ink);

        let mut badges: Vec<(BadgeKind, &str)> = vec![if e.is_bundled() {
            (BadgeKind::Neutral, "Built in")
        } else {
            (BadgeKind::Accent, "Added")
        }];
        if m.rtl() {
            badges.push((BadgeKind::Outline, "Right to left"));
        }
        if is_default {
            badges.push((BadgeKind::Solid, "Reading"));
        } else if env.settings.parallel_on && env.settings.parallel.contains(&m.id) {
            badges.push((BadgeKind::Success, "Shown"));
        }
        let mut bx = more.left() - 14.0;
        for (kind, label) in badges.iter().rev() {
            let w = widgets::badge_width(&painter, *kind, label);
            widgets::badge(&painter, p, bx - w, row.top() + 30.0, *kind, label);
            bx -= w + 6.0;
        }
        let tx = tile.right() + 14.0;
        let width = bx - 14.0 - tx;
        let title = format!("{} — {}", m.abbrev, m.name);
        let name = elide(&painter, &title, &theme::ui_semibold(13.0), width);
        text(&painter, tx, row.top() + 12.0, 20.0, &name, theme::ui_semibold(13.0), p.ink, false);
        let desc = elide(&painter, &description(e), &theme::ui_font(12.0), width);
        text(&painter, tx, row.top() + 32.0, 16.0, &desc, theme::ui_font(12.0), p.ink_2, false);

        if more_resp.hovered() || more_resp.is_pointer_button_down_on() {
            let fill = if more_resp.is_pointer_button_down_on() { p.fill_strong } else { p.fill };
            painter.circle_filled(more.center(), 14.0, fill);
        }
        icons::heavy(|| {
            icons::ellipsis(&painter, Rect::from_center_size(more.center(), vec2(16.0, 16.0)), p.ink_2)
        });
        crate::menu::popup(&more_resp, p).show(|ui| {
            ui.set_min_width(200.0);
            if ui.button("Read").clicked() {
                actions.push(Action::SetTranslation(m.id.clone()));
                actions.push(Action::OpenPage(Page::Reader));
            }
            if !m.is_transliteration()
                && ui.add_enabled(!is_default, egui::Button::new("Show Under Each Ayah")).clicked()
            {
                actions.push(Action::AddParallel(m.id.clone()));
                actions.push(Action::OpenPage(Page::Reader));
            }
            if !e.is_bundled() {
                crate::menu::separator(ui);
                if ui.button("Remove").clicked() {
                    actions.push(Action::RemoveTranslation(m.id.clone()));
                }
            }
        });

        if resp.clicked() {
            self.selected = Some(m.id.clone());
        }
        if resp.double_clicked() {
            actions.push(Action::SetTranslation(m.id.clone()));
            actions.push(Action::OpenPage(Page::Reader));
        }
    }

    /// The selected translation's card: tile, name, a sample, facts, its notice and actions.
    fn inspector(
        &mut self,
        ui: &mut Ui,
        p: &Palette,
        at: egui::Pos2,
        e: &Entry,
        env: &PageEnv<'_>,
        actions: &mut Vec<Action>,
    ) -> f32 {
        let m = &e.meta;
        let painter = ui.painter().clone();
        let x = at.x + 20.0;
        let inner = INSPECTOR - 40.0;
        let sample_ref = AyahRef::new(1, 2);
        let sample = env.library.load(&m.id).ok().map(|t| t.get(sample_ref).to_string()).unwrap_or_default();
        let serif = FontId::new(14.0, theme::family(theme::SERIF));
        let naskh = FontId::new(15.0, theme::family(theme::NASKH));
        let (lines, rtl_lines) = if m.rtl() {
            (Vec::new(), widgets::rtl_flow(&painter, &sample, &naskh, inner, 3))
        } else {
            (flow(&painter, &sample, &serif, 10.0, inner, 3), Vec::new())
        };
        let sample_h = lines.len() as f32 * 20.0 + rtl_lines.len() as f32 * 28.0 + 16.0;
        let notice: String =
            m.notice.lines().filter(|l| !l.starts_with("ID:")).collect::<Vec<_>>().join(" · ");
        let notice_lines = flow(&painter, &notice, &theme::ui_font(11.0), 10.0, inner, 6);
        let notice_h = if notice.is_empty() { 0.0 } else { notice_lines.len() as f32 * 16.0 + 12.0 };
        let height = 20.0 + 48.0 + 16.0 + 18.0 + sample_h + 96.0 + notice_h + 16.0 + 24.0 + 20.0;
        let card = Rect::from_min_size(at, vec2(INSPECTOR, height));
        p.shadows.panel.paint(&painter, card, 16.0);
        painter.rect_filled(card, 16.0, p.sidebar);

        let mut y = at.y + 20.0;
        let tile = Rect::from_min_size(pos2(x, y), vec2(56.0, 48.0));
        p.shadows.control.paint(&painter, tile, 12.0);
        painter.rect_filled(tile, 12.0, p.raised);
        let code = m.id.split('.').next().unwrap_or("").to_uppercase();
        let g = widgets::line(&painter, &code, theme::ui_bold(15.0), p.accent, 0.0);
        let gx = tile.center().x - g.size().x / 2.0;
        widgets::paint_on_baseline(
            &painter,
            gx,
            css_baseline(tile.top() + 14.0, 20.0, 15.0, false),
            g,
            p.accent,
        );
        let tx = tile.right() + 12.0;
        let name = elide(&painter, &m.abbrev, &theme::ui_semibold(15.0), card.right() - 20.0 - tx);
        text(&painter, tx, y + 5.0, 20.0, &name, theme::ui_semibold(15.0), p.ink, false);
        let sub = elide(&painter, &m.name, &theme::ui_font(12.0), card.right() - 20.0 - tx);
        text(&painter, tx, y + 27.0, 16.0, &sub, theme::ui_font(12.0), p.ink_2, false);
        y += 48.0 + 16.0;

        text(
            &painter,
            x,
            y,
            16.0,
            &format!("AL-FATIHAH {sample_ref}"),
            theme::ui_semibold(10.5),
            p.ink_3,
            false,
        );
        y += 18.0;
        for (k, l) in lines.iter().enumerate() {
            paint_runs(&painter, x, css_baseline(y + 20.0 * k as f32, 20.0, 14.0, true), l, p.ink);
        }
        widgets::paint_rtl(&painter, &rtl_lines, x + inner, y, 28.0, p.ink);
        y += sample_h;

        let value_w = card.right() - 20.0 - (x + 80.0);
        let facts = [
            ("Language", m.language.clone()),
            ("Translator", m.translator.clone()),
            ("Source", if e.is_bundled() { "Tanzil.net (built in)".into() } else { m.source.clone() }),
            ("Direction", if m.rtl() { "Right to left".into() } else { "Left to right".into() }),
        ];
        for (k, (label, value)) in facts.into_iter().enumerate() {
            let ry = y + 24.0 * k as f32;
            text(&painter, x, ry, 16.0, label, theme::ui_font(12.0), p.ink_3, false);
            let v = elide(&painter, &value, &theme::ui_medium(12.0), value_w);
            text(&painter, x + 80.0, ry, 16.0, &v, theme::ui_medium(12.0), p.ink, false);
        }
        y += 96.0;
        if !notice.is_empty() {
            for (k, l) in notice_lines.iter().enumerate() {
                paint_runs(&painter, x, css_baseline(y + 16.0 * k as f32, 16.0, 11.0, false), l, p.ink_3);
            }
            y += notice_h;
        }
        y += 16.0;

        let is_default = m.id == env.settings.translation;
        let label = if is_default { "Reading" } else { "Read This" };
        let (r, resp) = Button::new(Kind::Secondary, label)
            .small()
            .enabled(!is_default && !m.is_transliteration())
            .show_left(ui, p, x, y + 12.0, ui.id().with("make-default"));
        if resp.clicked() {
            actions.push(Action::SetTranslation(m.id.clone()));
        }
        let (_, resp) = Button::new(Kind::Plain, "Compare")
            .small()
            .icon(icons::compare)
            .enabled(!is_default && !m.is_transliteration())
            .show_left(ui, p, r.right() + 8.0, y + 12.0, ui.id().with("compare"));
        if resp.clicked() {
            actions.push(Action::AddParallel(m.id.clone()));
            actions.push(Action::OpenPage(Page::Reader));
        }
        card.bottom()
    }

    /// The catalogue: every translation the Tanzil Project offers, by language, with Add.
    fn catalog(
        &mut self,
        ui: &mut Ui,
        p: &Palette,
        at: Rect,
        env: &PageEnv<'_>,
        actions: &mut Vec<Action>,
    ) -> f32 {
        let painter = ui.painter().clone();
        let (left, right) = (at.left(), at.right());
        let mut y = at.top();
        let list = match env.catalog {
            Some(Ok(list)) => list,
            Some(Err(e)) => {
                let msg = if e == "Loading…" {
                    "Loading the catalogue…".to_string()
                } else {
                    format!("Couldn't load the catalogue: {e}")
                };
                text(&painter, left + 4.0, y, 20.0, &msg, theme::ui_font(13.0), p.ink_2, false);
                return y + 40.0;
            }
            None => {
                text(
                    &painter,
                    left + 4.0,
                    y,
                    20.0,
                    "Loading the catalogue…",
                    theme::ui_font(13.0),
                    p.ink_2,
                    false,
                );
                return y + 40.0;
            }
        };
        let note = "From the Tanzil Project (tanzil.net), via alquran.cloud. Each downloads once (about 1 MB) and \
                    then works offline. Translations are for personal, non-commercial use under their own terms.";
        for (k, l) in
            flow(&painter, note, &theme::ui_font(12.0), 10.0, (right - left).min(720.0), 3).iter().enumerate()
        {
            paint_runs(&painter, left, css_baseline(y + 18.0 * k as f32, 18.0, 12.0, false), l, p.ink_2);
        }
        y += 52.0;
        let q = env.query.trim().to_lowercase();
        let col_w = ((right - left - 16.0) / 2.0).max(280.0);
        let columns = if right - left >= 2.0 * 280.0 + 16.0 { 2 } else { 1 };
        let shown: Vec<&quran_core::catalog::CatalogEntry> = list
            .iter()
            .filter(|c| {
                q.is_empty()
                    || c.english_name.to_lowercase().contains(&q)
                    || c.name.to_lowercase().contains(&q)
                    || c.language_name().to_lowercase().contains(&q)
                    || c.identifier.contains(&q)
            })
            .collect();
        // Language groups, each a header then rows of `columns` entries.
        let mut groups: Vec<Vec<&quran_core::catalog::CatalogEntry>> = Vec::new();
        for c in shown {
            match groups.last_mut() {
                Some(g) if g[0].language_name() == c.language_name() => g.push(c),
                _ => groups.push(vec![c]),
            }
        }
        for group in groups {
            group_label(&painter, p, left, y + 4.0, group[0].language_name());
            y += 28.0;
            for row in group.chunks(columns) {
                for (col, c) in row.iter().enumerate() {
                    let r =
                        Rect::from_min_size(pos2(left + col as f32 * (col_w + 16.0), y), vec2(col_w, 52.0));
                    let installed = env.library.contains(&c.identifier);
                    let downloading = env.downloading.contains(&c.identifier);
                    let resp = ui.interact(r, ui.id().with(("cat", &c.identifier)), Sense::hover());
                    if resp.hovered() {
                        painter.rect_filled(r, 10.0, p.fill);
                    }
                    let tx = r.left() + 12.0;
                    let label = if downloading {
                        "Adding…"
                    } else if installed {
                        "Added"
                    } else {
                        "Add"
                    };
                    let (b, bresp) =
                        Button::new(if installed { Kind::Plain } else { Kind::Secondary }, label)
                            .small()
                            .enabled(!installed && !downloading)
                            .show_right(
                                ui,
                                p,
                                r.right() - 10.0,
                                r.center().y,
                                ui.id().with(("add", &c.identifier)),
                            );
                    let w = b.left() - 10.0 - tx;
                    let title = elide(&painter, &c.english_name, &theme::ui_semibold(13.0), w);
                    text(&painter, tx, r.top() + 8.0, 20.0, &title, theme::ui_semibold(13.0), p.ink, false);
                    let sub =
                        elide(&painter, &format!("{} · {}", c.name, c.identifier), &theme::ui_font(12.0), w);
                    text(&painter, tx, r.top() + 28.0, 16.0, &sub, theme::ui_font(12.0), p.ink_3, false);
                    if bresp.clicked() {
                        actions.push(Action::DownloadTranslation(c.identifier.clone()));
                    }
                }
                y += 56.0;
            }
            y += 16.0;
        }
        y
    }
}
