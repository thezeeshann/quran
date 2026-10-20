//! The reading view: one juz at a time, scrolled section by section (a ruku each, cut where a
//! juz begins). A juz's page opens with its heading and ends with "End of Juz" and a button on
//! to the next; jumping anywhere opens the juz it lies in.
//!
//! The scroll position is an *anchor* — a section plus a pixel offset into it — rather than a
//! position in one giant document, so only the sections on screen are ever laid out and
//! scrolling from Al-Fatihah to An-Nas costs the same per frame as scrolling one surah. Layouts
//! are cached per section and invalidated when typography, width or the translations shown
//! change; an ayah anchor keeps the reading position stable across those changes.
//!
//! Ported from bible-rs (MIT).

pub mod layout;

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use eframe::egui::{self, CornerRadius, CursorIcon, Id, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use eframe::emath::GuiRounding as _;
use quran_core::library::Library;
use quran_core::meta::AyahRef;
use quran_core::study::StudyStore;
use quran_core::translation::Translation;

use self::layout::{Ctx, Deco, SectionLayout, Typography, juz_sections, section_juz, section_of, sections};
use crate::actions::Action;
use crate::icons;
use crate::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Anchor {
    pub section: u16,
    /// Distance from the section's top to the top of the viewport.
    pub offset: f32,
}

pub struct Env<'a> {
    pub library: &'a Library,
    pub study: &'a StudyStore,
    pub typo: &'a Typography,
    pub theme: Theme,
    /// Translation ids shown under each ayah (the reading one first).
    pub columns: &'a [String],
    /// Whether arrow/page keys should scroll the reader (no text field has focus).
    pub keyboard: bool,
    /// Height of the toolbar floating over the reader's top edge.
    pub toolbar: f32,
    /// The reader's width is changing frame by frame (the sidebar sliding): typeset only what is
    /// on screen, not the neighbouring sections, which would be laid out again next frame.
    pub resizing: bool,
}

pub struct Reader {
    pub anchor: Anchor,
    cache: HashMap<u16, Arc<SectionLayout>>,
    key: u64,
    /// The column width the cached sections were typeset at.
    col_w: f32,
    resize_frames: u32,
    em: f32,
    /// Inclusive ayah range (within one surah).
    pub selection: Option<(AyahRef, AyahRef)>,
    flash: Option<(AyahRef, f64)>,
    pending_jump: Option<(AyahRef, bool)>,
    reanchor: Option<(AyahRef, f32)>,
    scroll_remaining: f32,
    viewport_h: f32,
    top_pad: f32,
    toolbar: f32,
    /// Whether text is scrolled beneath the toolbar (it then takes the toolbar material).
    pub under_toolbar: bool,
    scrolled_at: f64,
    /// The ayah at the reading line (for the toolbar and the saved position).
    pub location: AyahRef,
    /// The furthest ayah of the location's surah that is in view (reading progress).
    pub furthest_in_view: u16,
    /// The ayah whose note popover is open, and where its marker was drawn this frame.
    pub marker_for: Option<AyahRef>,
    pub marker_rect: Option<Rect>,
    /// Where the selection is on screen this frame, for popovers.
    pub selection_rect: Option<Rect>,
    frames: u64,
    /// Sections typeset so far.
    pub layouts: u64,
    /// The column's left edge and width this frame.
    pub column_x: (f32, f32),
}

impl Reader {
    pub fn new(at: AyahRef) -> Self {
        let at = if at.is_valid() { at } else { AyahRef::FIRST };
        let mut r = Self {
            anchor: Anchor { section: section_of(at), offset: 0.0 },
            cache: HashMap::new(),
            key: 0,
            col_w: 0.0,
            resize_frames: 0,
            em: 18.0,
            selection: None,
            flash: None,
            pending_jump: None,
            reanchor: None,
            scroll_remaining: 0.0,
            viewport_h: 800.0,
            top_pad: 76.0,
            toolbar: 52.0,
            under_toolbar: false,
            scrolled_at: f64::NEG_INFINITY,
            location: at,
            furthest_in_view: at.ayah,
            marker_for: None,
            marker_rect: None,
            selection_rect: None,
            frames: 0,
            layouts: 0,
            column_x: (0.0, 0.0),
        };
        if at.ayah > 1 || sections()[section_of(at) as usize].from != 1 {
            r.pending_jump = Some((at, false));
        }
        r
    }

    pub fn jump(&mut self, at: AyahRef, flash: bool) {
        self.scroll_remaining = 0.0;
        self.pending_jump = Some((at, flash));
        self.location = at;
    }

    pub fn scroll_by(&mut self, dy: f32) {
        self.scroll_remaining += dy;
    }

    pub fn page(&self) -> f32 {
        (self.viewport_h - self.toolbar - self.em * 3.0).max(self.em * 4.0)
    }

    pub fn invalidate(&mut self) {
        self.cache.clear();
    }

    pub fn selected_refs(&self) -> Vec<AyahRef> {
        let Some((a, b)) = self.selection else { return Vec::new() };
        if a.surah != b.surah {
            return vec![a];
        }
        (a.ayah..=b.ayah).map(|n| AyahRef::new(a.surah, n)).collect()
    }

    fn select(&mut self, v: AyahRef, extend: bool) {
        match (self.selection, extend) {
            (Some((a, _)), true) if a.surah == v.surah => {
                self.selection = Some(if v < a { (v, a) } else { (a, v) });
            }
            (Some((a, b)), false) if a == v && b == v => self.selection = None,
            _ => self.selection = Some((v, v)),
        }
    }

    pub fn set_selection(&mut self, v: Option<AyahRef>) {
        self.selection = v.map(|v| (v, v));
    }

    /// Moves a single-ayah selection by `delta` ayahs (across surahs at the ends).
    pub fn step_selection(&mut self, delta: i32) {
        let current = self.selection.map(|(a, b)| if delta > 0 { b } else { a });
        let target = match current {
            None => Some(self.location),
            Some(c) if delta > 0 => c.next(),
            Some(c) => c.prev(),
        };
        if let Some(r) = target {
            self.selection = Some((r, r));
            self.ensure_visible(r);
        }
    }

    fn ensure_visible(&mut self, r: AyahRef) {
        let s = section_of(r);
        let Some(l) = self.cache.get(&s) else {
            self.pending_jump = Some((r, false));
            return;
        };
        let Some(a) = l.anchor(r.ayah) else { return };
        let mut y = a.top - self.anchor.offset;
        if s != self.anchor.section {
            match self.cache.get(&self.anchor.section) {
                Some(cur) if s == self.anchor.section + 1 => y += cur.height,
                _ => {
                    self.pending_jump = Some((r, false));
                    return;
                }
            }
        }
        let margin = self.em * 3.0;
        let h = (a.bottom - a.top).min(self.viewport_h * 0.6);
        if y < self.toolbar + margin {
            self.scroll_remaining = y - self.toolbar - margin;
        } else if y + h > self.viewport_h - margin {
            self.scroll_remaining = y + h - self.viewport_h + margin;
        }
    }

    fn layout_key(env: &Env<'_>, width: f32) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        width.to_bits().hash(&mut h);
        serde_json::to_string(env.typo).unwrap_or_default().hash(&mut h);
        env.columns.hash(&mut h);
        h.finish()
    }

    fn section_layout(&mut self, ui: &Ui, env: &Env<'_>, geom: &Geometry, s: u16) -> Arc<SectionLayout> {
        if let Some(l) = self.cache.get(&s) {
            return l.clone();
        }
        let translations: Vec<Arc<Translation>> =
            env.columns.iter().filter_map(|id| env.library.load(id).ok()).collect();
        let translit = env
            .typo
            .transliteration
            .then(|| env.library.load(quran_core::library::TRANSLITERATION_ID).ok())
            .flatten();
        let cx = Ctx {
            typo: env.typo,
            width: geom.col_w,
            translations: &translations,
            transliteration: translit.as_ref(),
        };
        self.layouts += 1;
        let layout = Arc::new(ui.ctx().fonts_mut(|fonts| layout::layout_section(fonts, &cx, s)));
        self.cache.insert(s, layout.clone());
        if self.cache.len() > 64 {
            // Keep the neighbourhood of the reading position.
            let here = self.anchor.section as i32;
            self.cache.retain(|k, _| (*k as i32 - here).abs() <= 16);
        }
        layout
    }

    /// The sections of the juz being read.
    fn bounds(&self) -> (u16, u16) {
        let r = juz_sections(section_juz(self.anchor.section));
        (*r.start(), *r.end())
    }

    /// Shows the reader in `ui`, returning actions for the app (selection, notes, …).
    pub fn show(&mut self, ui: &mut Ui, env: &Env<'_>, actions: &mut Vec<Action>) {
        let rect = ui.available_rect_before_wrap();
        let p = env.theme.palette();
        ui.painter().rect_filled(rect, 0.0, p.paper);
        self.toolbar = env.toolbar;
        self.top_pad = env.toolbar + layout::PAGE_TOP;
        self.marker_rect = None;
        self.selection_rect = None;
        let id = Id::new("reader");
        // Text runs under the toolbar, but clicks there belong to the toolbar (and the window).
        let below_toolbar = Rect::from_min_max(pos2(rect.left(), rect.top() + env.toolbar), rect.max);
        let resp = ui.interact(below_toolbar, id, Sense::click_and_drag());
        let layouts_before = self.layouts;
        let mut geom = Geometry::new(rect, env.typo, ui.pixels_per_point());
        let first_resize_frame = env.resizing && self.resize_frames == 0;
        self.resize_frames = if env.resizing { self.resize_frames + 1 } else { 0 };
        if first_resize_frame
            && self.key != 0
            && (geom.col_w - self.col_w).abs() < 2.0 * env.typo.size
            && Self::layout_key(env, self.col_w) == self.key
        {
            geom = Geometry::with_column(rect, ui.pixels_per_point(), self.col_w);
        }
        self.column_x = (geom.x0, geom.col_w);
        self.em = env.typo.size;
        self.viewport_h = rect.height();

        // Re-key the cache when anything that affects layout changes, keeping the position.
        let key = Self::layout_key(env, geom.col_w);
        if key != self.key {
            if self.key != 0
                && self.pending_jump.is_none()
                && let Some(l) = self.cache.get(&self.anchor.section)
                && let Some(a) = l.ayah_at_y(self.anchor.offset)
                && self.anchor.offset >= a.top
            {
                let span = (a.bottom - a.top).max(1.0);
                let sec = sections()[self.anchor.section as usize];
                self.reanchor =
                    Some((AyahRef::new(sec.surah.0, a.ayah), (self.anchor.offset - a.top) / span));
            }
            self.cache.clear();
            self.key = key;
            self.col_w = geom.col_w;
        }
        if let Some((r, frac)) = self.reanchor.take() {
            let s = section_of(r);
            let l = self.section_layout(ui, env, &geom, s);
            if let Some(a) = l.anchor(r.ayah) {
                self.anchor = Anchor { section: s, offset: a.top + frac * (a.bottom - a.top) };
            }
        }
        if let Some((r, flash)) = self.pending_jump.take() {
            let s = section_of(r);
            let l = self.section_layout(ui, env, &geom, s);
            // An ayah shows a little below the top; a surah's or juz's first ayah shows its
            // heading.
            let offset = if r.ayah <= 1 || r == quran_core::meta::Juz(r.juz()).start() {
                0.0
            } else {
                l.anchor(r.ayah).map_or(0.0, |a| (a.top - self.em * 0.8).max(0.0))
            };
            self.anchor = Anchor { section: s, offset };
            if flash {
                self.flash = Some((r, ui.input(|i| i.time)));
            }
        }

        // Scrolling: wheel/trackpad (already smoothed by egui) and animated keyboard steps.
        let mut dy = 0.0;
        if resp.hovered() || resp.contains_pointer() {
            dy -= ui.input(|i| i.smooth_scroll_delta.y);
        }
        if self.scroll_remaining.abs() > 0.5 {
            let step = self.scroll_remaining * 0.28;
            let step = if step.abs() < 0.5 { self.scroll_remaining } else { step };
            dy += step;
            self.scroll_remaining -= step;
            ui.ctx().request_repaint();
        } else {
            self.scroll_remaining = 0.0;
        }
        // Scrollbar.
        let bar = Rect::from_min_max(
            pos2(rect.right() - 12.0, rect.top() + env.toolbar + 4.0),
            pos2(rect.right() - 2.0, rect.bottom() - 4.0),
        );
        let bar_resp = ui.interact(bar, id.with("bar"), Sense::click_and_drag());
        let (lo, hi) = self.bounds();
        let n = (hi - lo + 1) as f32;
        if let Some(pos) =
            bar_resp.interact_pointer_pos().filter(|_| bar_resp.dragged() || bar_resp.clicked())
        {
            let f = ((pos.y - bar.top()) / bar.height()).clamp(0.0, 0.9999) * n;
            let s = lo + f.floor() as u16;
            let l = self.section_layout(ui, env, &geom, s);
            self.anchor = Anchor { section: s, offset: f.fract() * l.height };
            self.scroll_remaining = 0.0;
            dy = 0.0;
        }
        self.anchor.offset += dy;
        if dy != 0.0 {
            self.scrolled_at = ui.input(|i| i.time);
        }
        self.normalize(ui, env, &geom);

        // Paint sections from the anchor down.
        let painter = ui.painter_at(rect);
        let pointer = ui.input(|i| i.pointer.hover_pos());
        let now = ui.input(|i| i.time);
        let flash_alpha = self.flash.map(|(_, t0)| {
            let t = (now - t0) as f32;
            if t > 2.2 { 0.0 } else { (1.0 - (t - 0.8).max(0.0) / 1.4).clamp(0.0, 1.0) }
        });
        if flash_alpha.is_some_and(|a| a > 0.0) {
            ui.ctx().request_repaint();
        } else {
            self.flash = None;
        }
        let selected: Vec<AyahRef> = self.selected_refs();
        let mut y = rect.top() + self.top_pad - self.anchor.offset;
        let mut s = Some(self.anchor.section);
        let mut hit: Option<AyahRef> = None;
        let mut marker_hit: Option<AyahRef> = None;
        let mut first_visible: Option<AyahRef> = None;
        let reading_line = rect.top() + rect.height() * 0.22;
        let toolbar_bottom = rect.top() + env.toolbar;
        let mut under_toolbar = false;
        let mut in_view: Vec<(u8, u16)> = Vec::new();
        let mut button_hit: Option<AyahRef> = None;
        let (first, last) = self.bounds();
        while let Some(si) = s {
            if y > rect.bottom() {
                break;
            }
            let l = self.section_layout(ui, env, &geom, si);
            let sec = sections()[si as usize];
            let origin = vec2(geom.x0, y);
            // Underlays: highlights, selection, flash.
            for a in &l.ayahs {
                let r = AyahRef::new(sec.surah.0, a.ayah);
                if a.bottom + y < rect.top() || a.top + y > rect.bottom() {
                    continue;
                }
                let mut fills = Vec::new();
                if let Some(hc) = env.study.highlight(r) {
                    fills.push(p.highlight(hc));
                }
                let is_selected = selected.contains(&r);
                if is_selected {
                    fills.push(p.selection);
                }
                if let (Some((fv, _)), Some(al)) = (self.flash, flash_alpha)
                    && fv == r
                    && al > 0.0
                {
                    fills.push(p.accent_soft.gamma_multiply(al));
                }
                for rr in &a.rects {
                    let rr = rr.translate(origin);
                    for f in &fills {
                        painter.rect_filled(rr, CornerRadius::same(8), *f);
                    }
                    if is_selected {
                        self.selection_rect = Some(self.selection_rect.map_or(rr, |s| s.union(rr)));
                    }
                }
            }
            for d in &l.decos {
                match d {
                    Deco::Rule { y: ry, x0, x1, role } => {
                        let yy = y + ry;
                        if yy > rect.top() && yy < rect.bottom() {
                            painter.hline(
                                (origin.x + x0)..=(origin.x + x1),
                                yy.round() + 0.5,
                                Stroke::new(1.0, role.color(p)),
                            );
                        }
                    }
                    Deco::Frame { rect: fr, role } => {
                        let fr = fr.translate(origin);
                        if fr.bottom() > rect.top() && fr.top() < rect.bottom() {
                            let c = role.color(p);
                            painter.rect_stroke(
                                fr,
                                CornerRadius::same(18),
                                Stroke::new(1.0, c.gamma_multiply(0.75)),
                                StrokeKind::Inside,
                            );
                            painter.rect_stroke(
                                fr.shrink(5.0),
                                CornerRadius::same(13),
                                Stroke::new(0.5, c.gamma_multiply(0.5)),
                                StrokeKind::Inside,
                            );
                            // Small diamonds at the middle of each side.
                            for (cx, cy) in [(fr.center().x, fr.top()), (fr.center().x, fr.bottom())] {
                                let d = 4.0;
                                let pts = vec![
                                    pos2(cx, cy - d),
                                    pos2(cx + d, cy),
                                    pos2(cx, cy + d),
                                    pos2(cx - d, cy),
                                ];
                                painter.add(egui::Shape::convex_polygon(
                                    pts,
                                    p.paper,
                                    Stroke::new(1.0, c.gamma_multiply(0.75)),
                                ));
                            }
                            under_toolbar |= fr.top() < toolbar_bottom;
                        }
                    }
                    Deco::Button { rect: br, to, primary } => {
                        let br = br.translate(origin);
                        if br.bottom() < rect.top() || br.top() > rect.bottom() {
                            continue;
                        }
                        let hovered = pointer.is_some_and(|pt| pt.y > toolbar_bottom && br.contains(pt));
                        if hovered {
                            button_hit = Some(*to);
                        }
                        let fill = match (primary, hovered) {
                            (true, false) => p.accent_soft,
                            (true, true) => p.accent_soft.gamma_multiply(1.6),
                            (false, false) => egui::Color32::TRANSPARENT,
                            (false, true) => p.fill,
                        };
                        painter.rect_filled(br, CornerRadius::same((br.height() / 2.0) as u8), fill);
                        under_toolbar |= br.top() < toolbar_bottom;
                    }
                }
            }
            for it in &l.items {
                let r = it.rect().translate(origin);
                if r.bottom() < rect.top() || r.top() > rect.bottom() {
                    continue;
                }
                under_toolbar |= r.top() < toolbar_bottom;
                painter.galley(r.min, it.galley.clone(), it.role.color(p));
            }
            if let Some(ptr) = pointer.filter(|pt| pt.y > toolbar_bottom) {
                let local = ptr - origin;
                if local.y >= 0.0
                    && local.y < l.height
                    && let Some(a) = l.ayah_at(local)
                {
                    hit = Some(AyahRef::new(sec.surah.0, a));
                }
            }
            // Margin markers (notes, bookmarks) at each ayah's first line, right of the column.
            let right = geom.x0 + geom.col_w + layout::MARKER_GAP * 0.75;
            for a in &l.ayahs {
                let r = AyahRef::new(sec.surah.0, a.ayah);
                let top = y + a.top;
                if top > rect.bottom() || y + a.bottom < rect.top() {
                    continue;
                }
                if first_visible.is_none() && y + a.bottom > reading_line {
                    first_visible = Some(r);
                }
                if top < rect.bottom() - self.em {
                    match in_view.last_mut() {
                        Some((su, n)) if *su == r.surah => *n = (*n).max(r.ayah),
                        _ => in_view.push((r.surah, r.ayah)),
                    }
                }
                let mut x = right;
                let cy = y + a.first_line.0 + a.first_line.1 / 2.0;
                if env.study.is_bookmarked(r) {
                    let br = Rect::from_center_size(pos2(x + 10.0, cy), vec2(16.0, 16.0));
                    icons::bookmark(&painter, br, p.accent, true);
                    if pointer.is_some_and(|pt| br.expand(4.0).contains(pt)) {
                        marker_hit = Some(r);
                    }
                    x += 24.0;
                }
                let open = self.marker_for == Some(r);
                if env.study.note(r).is_some() || open {
                    let nr = Rect::from_center_size(pos2(x + 10.0, cy), vec2(16.0, 16.0));
                    if open {
                        let box_ = Rect::from_center_size(nr.center(), vec2(20.0, 20.0));
                        painter.rect_filled(box_, CornerRadius::same(6), p.accent_soft);
                        self.marker_rect = Some(box_);
                    }
                    icons::note(&painter, nr, p.accent);
                    if pointer.is_some_and(|pt| nr.expand(4.0).contains(pt)) {
                        marker_hit = Some(r);
                    }
                }
            }
            if first_visible.is_none() && y + l.height > reading_line {
                first_visible = Some(sec.first());
            }
            y += l.height;
            s = (si < last).then_some(si + 1);
        }
        if let Some(fv) = first_visible {
            self.location = fv;
        }
        self.under_toolbar = under_toolbar;
        if let Some((_, n)) = in_view.iter().find(|(su, _)| *su == self.location.surah) {
            self.furthest_in_view = *n;
        }
        let last_visible = s.map_or(last, |n| n.saturating_sub(1));
        // While idle, typeset the neighbouring sections (one per frame), so scrolling into them
        // never has to lay out text first.
        self.frames += 1;
        let idle = self.frames > 2
            && dy == 0.0
            && self.scroll_remaining == 0.0
            && !resp.dragged()
            && !env.resizing
            && self.layouts == layouts_before;
        if idle {
            let wanted = [
                (last_visible < last).then_some(last_visible + 1),
                (self.anchor.section > first).then(|| self.anchor.section - 1),
                (last_visible + 1 < last).then_some(last_visible + 2),
            ];
            if let Some(next) = wanted.into_iter().flatten().find(|c| !self.cache.contains_key(c)) {
                self.section_layout(ui, env, &geom, next);
                ui.ctx().request_repaint();
            }
        }

        // Interaction.
        if marker_hit.is_some() || hit.is_some() || button_hit.is_some() {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }
        if resp.clicked() {
            if let Some(to) = button_hit {
                self.selection = None;
                actions.push(Action::Jump { to, flash: false });
            } else if let Some(v) = marker_hit {
                actions.push(Action::EditNote(v));
            } else if let Some(v) = hit {
                let shift = ui.input(|i| i.modifiers.shift);
                self.select(v, shift);
            } else {
                self.selection = None;
            }
        }
        if resp.double_clicked()
            && let Some(v) = hit
        {
            self.selection = Some((v, v));
            actions.push(Action::EditNote(v));
        }
        if resp.secondary_clicked()
            && let Some(v) = hit
        {
            if !selected.contains(&v) {
                self.selection = Some((v, v));
            }
            if let Some(pos) = resp.interact_pointer_pos() {
                actions.push(Action::OpenVerseMenu(pos));
            }
        }

        self.paint_scrollbar(ui, &bar, &bar_resp, p);
    }

    /// Keeps the anchor inside its section, moving to neighbours within the juz as needed.
    fn normalize(&mut self, ui: &Ui, env: &Env<'_>, geom: &Geometry) {
        let (first, last) = self.bounds();
        for _ in 0..64 {
            if self.anchor.offset < 0.0 {
                match (self.anchor.section > first).then(|| self.anchor.section - 1) {
                    Some(prev) => {
                        let h = self.section_layout(ui, env, geom, prev).height;
                        self.anchor = Anchor { section: prev, offset: self.anchor.offset + h };
                    }
                    None => {
                        self.anchor.offset = 0.0;
                        self.scroll_remaining = 0.0;
                        break;
                    }
                }
                continue;
            }
            let h = self.section_layout(ui, env, geom, self.anchor.section).height;
            if self.anchor.offset >= h {
                if self.anchor.section < last {
                    self.anchor = Anchor { section: self.anchor.section + 1, offset: self.anchor.offset - h };
                } else {
                    self.anchor.offset = (h - self.viewport_h * 0.5).max(0.0).min(self.anchor.offset);
                    self.scroll_remaining = 0.0;
                    break;
                }
                continue;
            }
            if self.anchor.section == last {
                let max = (h - self.viewport_h * 0.5).max(0.0);
                if self.anchor.offset > max {
                    self.anchor.offset = max;
                    self.scroll_remaining = 0.0;
                }
            }
            break;
        }
    }

    fn paint_scrollbar(&self, ui: &Ui, bar: &Rect, resp: &egui::Response, p: &crate::theme::Palette) {
        let painter = ui.painter();
        let active = resp.hovered() || resp.dragged();
        // Overlay scroller: visible while scrolling and for a moment after, or under the pointer.
        let since = ui.input(|i| i.time) - self.scrolled_at;
        let shown = ui.ctx().animate_bool_with_time(Id::new("reader-scroller"), active || since < 1.0, 0.2);
        if since < 1.0 {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
        }
        if shown == 0.0 {
            return;
        }
        let (lo, hi) = self.bounds();
        let n = (hi - lo + 1) as f32;
        if active {
            painter.rect_filled(bar.expand(1.0), CornerRadius::same(6), p.fill.gamma_multiply(0.8));
            // Ticks where each surah in the juz begins.
            for si in (lo + 1)..=hi {
                if !sections()[si as usize].starts_surah() {
                    continue;
                }
                let f = (si - lo) as f32 / n;
                let yy = bar.top() + f * bar.height();
                painter.line_segment(
                    [pos2(bar.left() + 2.0, yy), pos2(bar.right() - 2.0, yy)],
                    Stroke::new(1.0, p.separator),
                );
            }
        }
        let h = self.cache.get(&self.anchor.section).map_or(1.0, |c| c.height.max(1.0));
        let frac = ((self.anchor.section - lo) as f32 + (self.anchor.offset / h).clamp(0.0, 1.0)) / n;
        let thumb_h = 36.0;
        let ty = bar.top() + frac * (bar.height() - thumb_h);
        let thumb = Rect::from_min_size(pos2(bar.left() + 2.0, ty), vec2(bar.width() - 4.0, thumb_h));
        painter.rect_filled(
            thumb,
            CornerRadius::same(4),
            if active { p.ink_2 } else { p.ink_3.gamma_multiply(0.7 * shown) },
        );
        if let Some(pos) = resp.hover_pos() {
            let f = ((pos.y - bar.top()) / bar.height()).clamp(0.0, 0.9999) * n;
            let sec = sections()[(lo + f.floor() as u16) as usize];
            let label =
                format!("{} · {}:{} · Juz {}", sec.surah.name(), sec.surah.0, sec.from, sec.first().juz());
            egui::Tooltip::always_open(
                ui.ctx().clone(),
                ui.layer_id(),
                Id::new("bar-tip"),
                egui::PopupAnchor::Pointer,
            )
            .show(|ui| {
                ui.label(label);
            });
        }
    }
}

/// Column placement for the current window width. The column starts on a whole pixel, so text
/// stays crisp.
pub struct Geometry {
    pub x0: f32,
    pub col_w: f32,
}

impl Geometry {
    pub fn new(rect: Rect, typo: &Typography, pixels_per_point: f32) -> Self {
        // Room either side for the ayah numbers (left) and the note/bookmark markers (right).
        let margin = (rect.width() * 0.06).clamp(layout::GUTTER + 24.0, 104.0);
        let avail = (rect.width() - margin * 2.0).max(200.0);
        let col_w = avail.min(typo.size * typo.measure).floor();
        Self::with_column(rect, pixels_per_point, col_w)
    }

    /// A column `col_w` wide, centred in `rect`.
    pub fn with_column(rect: Rect, pixels_per_point: f32, col_w: f32) -> Self {
        let x0 = (rect.center().x - col_w / 2.0).round_to_pixels(pixels_per_point);
        Self { x0, col_w }
    }
}
