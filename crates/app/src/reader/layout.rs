//! Section typesetting: turns a section of the Quran (one ruku, cut where a juz begins) into
//! positioned galleys.
//!
//! The Arabic is set word by word. egui shapes each run with HarfRust, which joins the letters,
//! places the harakat and Quranic marks and puts a right-to-left word's glyphs in visual order —
//! but it wraps paragraphs left to right, so lines of Arabic are broken and filled here: words
//! go from the right edge leftward, the next line under the first. That also gives every word
//! its own rectangle, which is what highlights, selection and hit-testing use.
//!
//! Layout depends only on the text, the typography settings and the column width — never on
//! highlights, selection or theme (galleys are laid out in `Color32::PLACEHOLDER` and painted in
//! their [`Role`]'s colour), so selecting or highlighting an ayah never forces a relayout.

use std::sync::Arc;

use eframe::egui::text::{LayoutJob, TextFormat, TextWrapping};
use eframe::egui::{Align, Color32, FontId, Galley, Rect, Vec2, pos2, vec2};
use eframe::epaint::text::FontsView;
use quran_core::meta::{AyahRef, Juz, SurahId, marks, quarter_label};
use quran_core::text::{arabic_number, is_arabic, quran};
use quran_core::translation::Translation;
use serde::{Deserialize, Serialize};

use crate::theme::{
    self, ARABIC, NASKH, QURAN, SERIF, SERIF_DISPLAY, SERIF_ITALIC, UI, UI_MEDIUM, UI_SEMIBOLD, text,
};

/// CSS centres a font's content area (ascender + descender) in its line box, so its baseline lies
/// this many ems below the box's centre: (ascender − descender) / 2, from the fonts' hhea tables.
pub const LITERATA_BASELINE: f32 = (1.177 - 0.308) / 2.0;
pub const INTER_BASELINE: f32 = (1984.0 - 494.0) / 2048.0 / 2.0;

/// Where egui puts the baseline of a one-line galley in `format`, from the top of its row.
pub(crate) fn baseline(fonts: &mut FontsView<'_>, format: &TextFormat) -> f32 {
    let g = fonts.layout_job(LayoutJob::single_section("H".into(), format.clone()));
    g.rows.first().and_then(|r| r.glyphs.first()).map_or(0.0, |gl| gl.pos.y)
}

/// The ayah marker's number: its size, and how far above the ornament's baseline its centre
/// sits, both as a fraction of the Arabic size.
pub const MARKER_DIGITS: f32 = 0.48;
pub const MARKER_RISE: f32 = 0.2;

/// The design is drawn at a 20 pt reading size; its metrics scale with the user's size.
pub const DESIGN_EM: f32 = text::BODY.size;
/// From the toolbar to the top of a section scrolled to its start.
pub const PAGE_TOP: f32 = theme::space::S6;
/// From the text column to the margin markers.
pub const MARKER_GAP: f32 = theme::space::S8;
/// The left gutter of translation mode, where ayah numbers stand.
pub const GUTTER: f32 = 72.0;
/// Blank space after a surah's last section.
const SURAH_END: f32 = 72.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum Mode {
    /// Ayah by ayah: the Arabic, then its transliteration and translations.
    #[default]
    Translation,
    /// The Arabic alone, flowing like the page of a mushaf, ayah markers between ayahs.
    Reading,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Typography {
    /// Translation text size.
    pub size: f32,
    /// Arabic text size.
    pub arabic_size: f32,
    /// Translation line height (× size).
    pub line_height: f32,
    /// Arabic line height (× arabic size); Quranic marks need room above and below.
    pub arabic_line: f32,
    pub mode: Mode,
    pub show_translation: bool,
    pub transliteration: bool,
    /// Justify the Arabic lines (both edges), as a mushaf does.
    pub justify: bool,
    /// Ayah numbers in the margin (translation mode).
    pub ayah_numbers: bool,
    /// Surah headings, the Basmala and juz banners.
    pub headings: bool,
    /// Maximum text measure in ems of the translation size.
    pub measure: f32,
    /// Focus mode: the surah's head is one quiet line.
    #[serde(skip)]
    pub quiet_head: bool,
}

impl Default for Typography {
    fn default() -> Self {
        Self {
            size: 18.0,
            arabic_size: 30.0,
            line_height: 1.6,
            arabic_line: 2.05,
            mode: Mode::Translation,
            show_translation: true,
            transliteration: false,
            justify: false,
            ayah_numbers: true,
            headings: true,
            measure: 36.0,
            quiet_head: false,
        }
    }
}

/// The colour a galley or shape is painted in, resolved from the theme at paint time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Ink,
    Ink2,
    Ink3,
    /// Ayah markers and numbers, the heading's ornament (a muted gold).
    Verse,
    Accent,
    Rubric,
    Separator,
}

impl Role {
    pub fn color(self, p: &theme::Palette) -> Color32 {
        match self {
            Role::Ink => p.ink,
            Role::Ink2 => p.ink_2,
            Role::Ink3 => p.ink_3,
            Role::Verse => p.verse,
            Role::Accent => p.accent,
            Role::Rubric => p.rubric,
            Role::Separator => p.separator,
        }
    }
}

/// A galley placed in a section (relative to the column's left edge and the section's top).
pub struct Item {
    pub pos: Vec2,
    pub galley: Arc<Galley>,
    pub role: Role,
    /// The ayah it belongs to, if any.
    pub ayah: Option<u16>,
}

impl Item {
    pub fn rect(&self) -> Rect {
        Rect::from_min_size(self.pos.to_pos2(), self.galley.size())
    }
}

pub enum Deco {
    /// A hairline from `x0` to `x1` at `y`.
    Rule { y: f32, x0: f32, x1: f32, role: Role },
    /// The surah heading's frame: a rounded outline with a finer one inside it.
    Frame { rect: Rect, role: Role },
    /// A button (its label is an item) that goes to `to`: a tinted pill, or plain text when
    /// not `primary`.
    Button { rect: Rect, to: AyahRef, primary: bool },
}

/// Where an ayah is: its band from first line to last, and the rectangles that wash it when it
/// is highlighted, selected or flashed (one per line of Arabic in reading mode; the whole row in
/// translation mode).
pub struct AyahBox {
    pub ayah: u16,
    pub top: f32,
    pub bottom: f32,
    /// Top and height of its first line (margin markers centre on it).
    pub first_line: (f32, f32),
    pub rects: Vec<Rect>,
}

pub struct SectionLayout {
    pub section: u16,
    pub height: f32,
    pub items: Vec<Item>,
    pub decos: Vec<Deco>,
    pub ayahs: Vec<AyahBox>,
}

impl SectionLayout {
    /// The box of `ayah`, or the nearest after it (the last if none).
    pub fn anchor(&self, ayah: u16) -> Option<&AyahBox> {
        self.ayahs.iter().find(|a| a.ayah >= ayah).or_else(|| self.ayahs.last())
    }

    /// The ayah whose top is at or above `y` (the first if none).
    pub fn ayah_at_y(&self, y: f32) -> Option<&AyahBox> {
        self.ayahs.iter().rev().find(|a| a.top <= y).or_else(|| self.ayahs.first())
    }

    /// The ayah under a point (section-relative).
    pub fn ayah_at(&self, p: eframe::egui::Pos2) -> Option<u16> {
        if let Some(a) =
            self.ayahs.iter().find(|a| a.rects.iter().any(|r| r.expand2(vec2(4.0, 0.0)).contains(p)))
        {
            return Some(a.ayah);
        }
        self.items.iter().find(|i| i.ayah.is_some() && i.rect().expand(2.0).contains(p)).and_then(|i| i.ayah)
    }
}

/// The reading sections: one per ruku, which every surah starts with, also cut where a juz begins
/// — so every section lies in one juz, and the reader can show a juz at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Section {
    pub surah: SurahId,
    pub from: u16,
    pub to: u16,
}

impl Section {
    pub fn first(&self) -> AyahRef {
        AyahRef::new(self.surah.0, self.from)
    }

    pub fn starts_surah(&self) -> bool {
        self.from == 1
    }

    pub fn ends_surah(&self) -> bool {
        self.to == self.surah.ayahs()
    }
}

pub fn sections() -> &'static [Section] {
    static S: std::sync::OnceLock<Vec<Section>> = std::sync::OnceLock::new();
    S.get_or_init(|| {
        let mut starts: Vec<u16> = (1..=556u16)
            .filter_map(|r| {
                // Rukus begin every section; surah starts are among them.
                let a = ruku_start(r)?;
                Some(a.index())
            })
            .chain(SurahId::all().map(|s| s.start()))
            .chain(Juz::all().map(|j| j.start().index()))
            .collect();
        starts.sort_unstable();
        starts.dedup();
        let mut out = Vec::with_capacity(starts.len());
        for (i, &s) in starts.iter().enumerate() {
            let end = starts.get(i + 1).copied().unwrap_or(quran_core::AYAH_COUNT) - 1;
            let a = AyahRef::from_index(s).expect("in range");
            let b = AyahRef::from_index(end).expect("in range");
            out.push(Section { surah: a.surah_id(), from: a.ayah, to: b.ayah });
        }
        out
    })
}

fn ruku_start(r: u16) -> Option<AyahRef> {
    // The ruku table isn't public in the core; find each ruku's first ayah by scanning (once).
    static STARTS: std::sync::OnceLock<Vec<AyahRef>> = std::sync::OnceLock::new();
    let v = STARTS.get_or_init(|| {
        let mut v = Vec::new();
        let mut last = 0;
        for i in 0..quran_core::AYAH_COUNT {
            let a = AyahRef::from_index(i).unwrap();
            let k = quran_core::meta::ruku_of(a);
            if k != last {
                v.push(a);
                last = k;
            }
        }
        v
    });
    v.get(r as usize - 1).copied()
}

/// The section containing an ayah.
pub fn section_of(r: AyahRef) -> u16 {
    let i = r.index();
    let secs = sections();
    let at = secs.partition_point(|s| s.first().index() <= i);
    at.saturating_sub(1) as u16
}

/// The sections of a juz.
pub fn juz_sections(j: Juz) -> std::ops::RangeInclusive<u16> {
    section_of(j.start())..=section_of(j.end())
}

/// The juz a section lies in.
pub fn section_juz(index: u16) -> Juz {
    Juz(sections()[index as usize].first().juz())
}

/// What a section is set from: the translations shown and the transliteration, if on.
pub struct Ctx<'a> {
    pub typo: &'a Typography,
    pub width: f32,
    pub translations: &'a [Arc<Translation>],
    pub transliteration: Option<&'a Arc<Translation>>,
}

impl Ctx<'_> {
    fn s(&self, pt: f32) -> f32 {
        pt * self.typo.size / DESIGN_EM
    }

    fn arabic_lh(&self) -> f32 {
        (self.typo.arabic_size * self.typo.arabic_line).round()
    }

    fn lh(&self) -> f32 {
        (self.typo.size * self.typo.line_height).round()
    }

    fn quran_font(&self) -> FontId {
        FontId::new(self.typo.arabic_size, theme::family(QURAN))
    }
}

fn fmt(font: FontId, line_height: Option<f32>) -> TextFormat {
    TextFormat {
        font_id: font,
        color: Color32::PLACEHOLDER,
        line_height,
        valign: Align::BOTTOM,
        ..Default::default()
    }
}

fn one_line(fonts: &mut FontsView<'_>, text: &str, font: FontId) -> Arc<Galley> {
    fonts.layout_job(LayoutJob::single_section(text.to_string(), fmt(font, None)))
}

fn tracked(fonts: &mut FontsView<'_>, text: &str, font: FontId, tracking: f32) -> Arc<Galley> {
    let mut f = fmt(font, None);
    f.extra_letter_spacing = tracking;
    fonts.layout_job(LayoutJob::single_section(text.to_string(), f))
}

fn wrapped(fonts: &mut FontsView<'_>, text: &str, font: FontId, lh: f32, width: f32) -> Arc<Galley> {
    let mut job = LayoutJob::single_section(text.to_string(), fmt(font, Some(lh)));
    job.wrap = TextWrapping { max_width: width, ..Default::default() };
    fonts.layout_job(job)
}

/// Prepares a word of right-to-left text for egui, which shapes each word as one run: the
/// shaper reverses the whole run, so digit runs in it (which read left to right) are reversed
/// first; a word with no Arabic letters stays left to right (a lone bracket is mirrored).
pub fn rtl_word(word: &str) -> String {
    let arabic = word.chars().any(is_arabic);
    if !arabic {
        // A bracketed number reads the same either way; a lone bracket faces the other way.
        let mirrored = match word {
            "(" => ")",
            ")" => "(",
            "[" => "]",
            "]" => "[",
            "«" => "»",
            "»" => "«",
            w => w,
        };
        return mirrored.to_string();
    }
    let mut out = String::with_capacity(word.len());
    let mut digits = String::new();
    for c in word.chars() {
        if c.is_numeric() {
            digits.push(c);
        } else {
            out.extend(digits.chars().rev());
            digits.clear();
            out.push(c);
        }
    }
    out.extend(digits.chars().rev());
    out
}

/// A piece of right-to-left flow: a word, or an ayah's end marker.
struct Tok {
    galley: Arc<Galley>,
    /// The marker's number, set inside it.
    digits: Option<Arc<Galley>>,
    ayah: u16,
    marker: bool,
}

/// Fills lines of `width` from the right with `units` (tokens that stay together: an ayah's
/// last word and its marker), each line `lh` tall from `y`. Returns the y after the last line.
#[allow(clippy::too_many_arguments)]
fn flow_rtl(
    units: Vec<Vec<Tok>>,
    width: f32,
    space: f32,
    lh: f32,
    justify: bool,
    mut y: f32,
    marker_role: Role,
    text_role: Role,
    size: f32,
    items: &mut Vec<Item>,
    rects: &mut Vec<(u16, Rect)>,
) -> f32 {
    let unit_w = |u: &Vec<Tok>| -> f32 {
        u.iter().map(|t| t.galley.size().x).sum::<f32>() + space * 0.55 * (u.len().saturating_sub(1)) as f32
    };
    let mut lines: Vec<Vec<Vec<Tok>>> = Vec::new();
    let mut line: Vec<Vec<Tok>> = Vec::new();
    let mut used = 0.0;
    for u in units {
        let w = unit_w(&u);
        let need = if line.is_empty() { w } else { used + space + w };
        if !line.is_empty() && need > width {
            lines.push(std::mem::take(&mut line));
            used = w;
        } else {
            used = need;
        }
        line.push(u);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    let n_lines = lines.len();
    for (li, line) in lines.into_iter().enumerate() {
        let total: f32 =
            line.iter().map(&unit_w).sum::<f32>() + space * (line.len().saturating_sub(1)) as f32;
        let gaps = line.len().saturating_sub(1) as f32;
        let extra =
            if justify && li + 1 < n_lines && gaps > 0.0 { ((width - total) / gaps).max(0.0) } else { 0.0 };
        let mut x = width;
        for unit in line {
            let n = unit.len();
            for (k, t) in unit.into_iter().enumerate() {
                let w = t.galley.size().x;
                x -= w;
                let gy = y + (lh - t.galley.size().y) / 2.0;
                let pos = vec2(x, gy);
                rects.push((t.ayah, Rect::from_min_size(pos2(x, y), vec2(w, lh))));
                if let Some(d) = t.digits {
                    // The number sits in the middle of the ornament.
                    let baseline =
                        t.galley.rows.first().and_then(|r| r.glyphs.first()).map_or(0.0, |g| g.pos.y);
                    let cy = gy + baseline - size * MARKER_RISE;
                    let dp = vec2(x + (w - d.size().x) / 2.0, cy - d.size().y / 2.0);
                    items.push(Item { pos: dp, galley: d, role: marker_role, ayah: Some(t.ayah) });
                }
                items.push(Item {
                    pos,
                    galley: t.galley,
                    role: if t.marker { marker_role } else { text_role },
                    ayah: Some(t.ayah),
                });
                if k + 1 < n {
                    x -= space * 0.55;
                }
            }
            x -= space + extra;
        }
        y += lh;
    }
    y
}

/// The ayah's words and marker as flow units.
fn ayah_units(fonts: &mut FontsView<'_>, cx: &Ctx<'_>, r: AyahRef) -> Vec<Vec<Tok>> {
    let font = cx.quran_font();
    let words = quran().words(r);
    let mut units: Vec<Vec<Tok>> = words
        .iter()
        .map(|w| {
            vec![Tok { galley: one_line(fonts, w, font.clone()), digits: None, ayah: r.ayah, marker: false }]
        })
        .collect();
    let frame = one_line(fonts, "\u{06DD}", font.clone());
    let digits_font = FontId::new((cx.typo.arabic_size * MARKER_DIGITS).round(), theme::family(ARABIC));
    let digits: String = arabic_number(r.ayah as u32).chars().rev().collect();
    let marker = Tok {
        galley: frame,
        digits: Some(one_line(fonts, &digits, digits_font)),
        ayah: r.ayah,
        marker: true,
    };
    match units.last_mut() {
        Some(last) => last.push(marker),
        None => units.push(vec![marker]),
    }
    units
}

/// Lays out a run of right-to-left text (an Urdu translation) word by word.
#[allow(clippy::too_many_arguments)]
fn rtl_paragraph(
    fonts: &mut FontsView<'_>,
    text: &str,
    font: FontId,
    width: f32,
    lh: f32,
    y: f32,
    ayah: u16,
    items: &mut Vec<Item>,
) -> f32 {
    let space = fonts.glyph_width(&font, ' ').max(font.size * 0.25);
    let units: Vec<Vec<Tok>> = text
        .split_whitespace()
        .map(|w| {
            vec![Tok {
                galley: one_line(fonts, &rtl_word(w), font.clone()),
                digits: None,
                ayah,
                marker: false,
            }]
        })
        .collect();
    let mut rects = Vec::new();
    flow_rtl(units, width, space, lh, false, y, Role::Ink, Role::Ink, font.size, items, &mut rects)
}

struct Builder<'c, 'a> {
    cx: &'c Ctx<'a>,
    y: f32,
    items: Vec<Item>,
    decos: Vec<Deco>,
    ayahs: Vec<AyahBox>,
}

impl Builder<'_, '_> {
    fn centered(&mut self, g: Arc<Galley>, role: Role) {
        let x = ((self.cx.width - g.size().x) / 2.0).round();
        let h = g.size().y;
        self.items.push(Item { pos: vec2(x, self.y), galley: g, role, ayah: None });
        self.y += h;
    }

    /// A banner across the column: a hairline either side of a tracked label.
    fn banner(&mut self, fonts: &mut FontsView<'_>, label: &str) {
        let cx = self.cx;
        let g = tracked(fonts, &label.to_uppercase(), FontId::new(11.0, theme::family(UI_SEMIBOLD)), 1.3);
        let w = g.size().x;
        let mid = cx.width / 2.0;
        let cy = self.y + g.size().y / 2.0;
        let reach = (cx.width * 0.28).min(160.0);
        self.decos.push(Deco::Rule {
            y: cy,
            x0: mid - w / 2.0 - 14.0 - reach,
            x1: mid - w / 2.0 - 14.0,
            role: Role::Separator,
        });
        self.decos.push(Deco::Rule {
            y: cy,
            x0: mid + w / 2.0 + 14.0,
            x1: mid + w / 2.0 + 14.0 + reach,
            role: Role::Separator,
        });
        self.centered(g, Role::Ink3);
    }

    /// A button centred in the column.
    fn button(&mut self, fonts: &mut FontsView<'_>, label: &str, to: AyahRef, primary: bool) {
        let cx = self.cx;
        let font = if primary { UI_SEMIBOLD } else { UI_MEDIUM };
        let g = one_line(fonts, label, FontId::new(13.0, theme::family(font)));
        let size = vec2(g.size().x + 36.0, (g.size().y + 16.0).max(32.0));
        let rect = Rect::from_min_size(pos2(((cx.width - size.x) / 2.0).round(), self.y), size);
        self.items.push(Item {
            pos: vec2((rect.center().x - g.size().x / 2.0).round(), rect.center().y - g.size().y / 2.0),
            galley: g,
            role: if primary { Role::Accent } else { Role::Ink2 },
            ayah: None,
        });
        self.decos.push(Deco::Button { rect, to, primary });
        self.y += size.y;
    }

    /// The top of a juz's page: the way back to the juz before, then the juz framed like a surah
    /// heading — its number, its name in Arabic and in transliteration, and what it spans.
    fn juz_top(&mut self, fonts: &mut FontsView<'_>, j: Juz, starts_surah: bool) {
        let cx = self.cx;
        self.y += cx.s(8.0);
        if j.0 > 1 {
            let prev = Juz(j.0 - 1);
            self.button(fonts, &format!("‹  Juz {} · {}", prev.0, prev.name()), prev.start(), false);
            self.y += cx.s(16.0);
        }
        if cx.typo.headings {
            self.juz_head(fonts, j);
            self.y += cx.s(28.0);
        }
        if !starts_surah {
            // A juz that begins mid-surah says which surah it continues.
            let s = j.start().surah_id();
            let label = format!("{} · continued from ayah {}", s.name(), j.start().ayah);
            self.banner(fonts, &label);
            self.y += cx.s(12.0);
        }
    }

    /// The end of a juz's page: "End of Juz 1" and the way on to the next.
    fn juz_foot(&mut self, fonts: &mut FontsView<'_>, j: Juz) {
        let cx = self.cx;
        self.y += cx.s(36.0);
        self.banner(fonts, &format!("End of Juz {} · {}", j.0, j.name()));
        self.y += cx.s(20.0);
        match Juz::new(j.0 as u32 + 1) {
            Some(next) => self.button(
                fonts,
                &format!("Continue to Juz {} · {}  ›", next.0, next.name()),
                next.start(),
                true,
            ),
            None => self.button(fonts, "Begin again · Juz 1 · Alif Lam Mim", AyahRef::FIRST, true),
        }
        self.y += cx.s(64.0);
    }

    /// A juz framed like a surah heading — its number, its name in Arabic and in
    /// transliteration, and what it spans.
    fn juz_head(&mut self, fonts: &mut FontsView<'_>, j: Juz) {
        let cx = self.cx;
        if cx.typo.quiet_head {
            let label = format!("JUZ {} · {}", j.0, j.name().to_uppercase());
            let g = tracked(fonts, &label, FontId::new(11.0, theme::family(UI_SEMIBOLD)), 1.2);
            self.centered(g, Role::Verse);
            self.y += cx.s(20.0);
            return;
        }
        let top = self.y;
        self.y += cx.s(20.0);
        let label =
            tracked(fonts, &format!("JUZ {}", j.0), FontId::new(11.0, theme::family(UI_SEMIBOLD)), 1.3);
        self.centered(label, Role::Verse);
        self.y += cx.s(6.0);
        let name = one_line(fonts, j.arabic(), FontId::new(cx.s(30.0).round(), theme::family(ARABIC)));
        self.centered(name, Role::Ink);
        self.y += cx.s(2.0);
        let latin = one_line(fonts, j.name(), FontId::new(cx.s(22.0).round(), theme::family(SERIF_DISPLAY)));
        self.centered(latin, Role::Ink);
        self.y += cx.s(6.0);
        let span = quran_core::plans::Passage::new(j.start(), j.end()).label();
        let meta = format!("{span} · {} ayahs · {} pages", j.ayahs(), j.pages());
        let meta = one_line(fonts, &meta, FontId::new(cx.s(13.0).round(), theme::family(UI)));
        self.centered(meta, Role::Ink2);
        self.y += cx.s(20.0);
        let w = (cx.width * 0.82).min(cx.s(520.0)).max(200.0);
        let rect = Rect::from_min_max(pos2((cx.width - w) / 2.0, top), pos2((cx.width + w) / 2.0, self.y));
        self.decos.push(Deco::Frame { rect, role: Role::Verse });
    }

    fn surah_head(&mut self, fonts: &mut FontsView<'_>, s: SurahId) {
        let cx = self.cx;
        if cx.typo.quiet_head {
            let label = format!("{} · {}", s.name().to_uppercase(), s.0);
            let g = tracked(fonts, &label, FontId::new(11.0, theme::family(UI_SEMIBOLD)), 1.2);
            self.centered(g, Role::Ink3);
            self.y += 20.0;
        } else {
            self.y += cx.s(8.0);
            // The frame: the surah's number over its name in Arabic, then its name and meaning.
            let top = self.y;
            self.y += cx.s(22.0);
            let label =
                tracked(fonts, &format!("SURAH {}", s.0), FontId::new(11.0, theme::family(UI_SEMIBOLD)), 1.3);
            self.centered(label, Role::Verse);
            self.y += cx.s(6.0);
            let name = one_line(
                fonts,
                &format!("سورة {}", s.arabic()),
                FontId::new(cx.s(34.0).round(), theme::family(ARABIC)),
            );
            self.centered(name, Role::Ink);
            self.y += cx.s(2.0);
            let latin =
                one_line(fonts, s.name(), FontId::new(cx.s(26.0).round(), theme::family(SERIF_DISPLAY)));
            self.centered(latin, Role::Ink);
            self.y += cx.s(6.0);
            let meta = format!("{} · {} · {} ayahs", s.english(), s.revelation(), s.ayahs());
            let meta = one_line(fonts, &meta, FontId::new(cx.s(13.0).round(), theme::family(UI)));
            self.centered(meta, Role::Ink2);
            self.y += cx.s(22.0);
            let w = (cx.width * 0.82).min(cx.s(520.0)).max(200.0);
            let rect =
                Rect::from_min_max(pos2((cx.width - w) / 2.0, top), pos2((cx.width + w) / 2.0, self.y));
            self.decos.push(Deco::Frame { rect, role: Role::Verse });
            self.y += cx.s(28.0);
        }
        if s.has_basmala_heading() {
            let g = one_line(
                fonts,
                quran_core::text::BASMALA,
                FontId::new((cx.typo.arabic_size * 0.9).round(), theme::family(QURAN)),
            );
            let lh = cx.arabic_lh();
            let x = ((cx.width - g.size().x) / 2.0).round();
            self.items.push(Item {
                pos: vec2(x, self.y + (lh - g.size().y) / 2.0),
                galley: g,
                role: Role::Ink,
                ayah: None,
            });
            self.y += lh + cx.s(16.0);
        } else {
            self.y += cx.s(4.0);
        }
    }

    fn ayah_box(&mut self, ayah: u16, top: f32, first_line: (f32, f32), rects: Vec<Rect>) {
        let bottom = rects.iter().map(|r| r.bottom()).fold(top, f32::max).max(self.y);
        self.ayahs.push(AyahBox { ayah, top, bottom, first_line, rects });
    }
}

/// Merges per-word rectangles into one per line and ayah.
fn merge_lines(rects: &[(u16, Rect)], ayah: u16) -> Vec<Rect> {
    let mut out: Vec<Rect> = Vec::new();
    for (_, r) in rects.iter().filter(|(a, _)| *a == ayah) {
        match out.iter_mut().find(|o| (o.top() - r.top()).abs() < 0.5) {
            Some(o) => *o = o.union(*r),
            None => out.push(*r),
        }
    }
    out
}

/// Lays out one section.
pub fn layout_section(fonts: &mut FontsView<'_>, cx: &Ctx<'_>, index: u16) -> SectionLayout {
    let sec = sections()[index as usize];
    let mut b = Builder { cx, y: 0.0, items: Vec::new(), decos: Vec::new(), ayahs: Vec::new() };
    let s = sec.surah;
    let first = sec.first();
    let juz = Juz(first.juz());
    if juz.start() == first {
        b.juz_top(fonts, juz, sec.starts_surah());
    }
    if sec.starts_surah() {
        if juz.start() != first {
            b.y += cx.s(8.0);
        }
        b.surah_head(fonts, s);
    }
    let font = cx.quran_font();
    let space = (fonts.glyph_width(&font, ' ') * 1.15).max(cx.typo.arabic_size * 0.22);
    let lh = cx.arabic_lh();
    match cx.typo.mode {
        Mode::Translation => {
            for a in sec.from..=sec.to {
                let r = AyahRef::new(s.0, a);
                let m = marks(r);
                let top = b.y;
                b.y += cx.s(14.0);
                let units = ayah_units(fonts, cx, r);
                let mut rects = Vec::new();
                let arabic_top = b.y;
                b.y = flow_rtl(
                    units,
                    cx.width,
                    space,
                    lh,
                    cx.typo.justify,
                    b.y,
                    Role::Verse,
                    Role::Ink,
                    cx.typo.arabic_size,
                    &mut b.items,
                    &mut rects,
                );
                if let Some(t) = cx.transliteration {
                    b.y += cx.s(2.0);
                    let g = wrapped(
                        fonts,
                        t.get(r),
                        FontId::new(cx.s(16.0).round(), theme::family(SERIF_ITALIC)),
                        cx.s(24.0).round(),
                        cx.width,
                    );
                    let h = g.size().y;
                    b.items.push(Item { pos: vec2(0.0, b.y), galley: g, role: Role::Ink2, ayah: Some(a) });
                    b.y += h;
                }
                if cx.typo.show_translation {
                    let many = cx.translations.len() > 1;
                    for t in cx.translations {
                        b.y += cx.s(10.0);
                        if many {
                            let g = tracked(
                                fonts,
                                &t.meta.abbrev.to_uppercase(),
                                FontId::new(10.0, theme::family(UI_SEMIBOLD)),
                                1.0,
                            );
                            let h = g.size().y;
                            b.items.push(Item {
                                pos: vec2(0.0, b.y),
                                galley: g,
                                role: Role::Ink3,
                                ayah: Some(a),
                            });
                            b.y += h + 3.0;
                        }
                        let body = t.get(r);
                        if t.meta.rtl() {
                            let f = FontId::new((cx.typo.size * 1.12).round(), theme::family(NASKH));
                            let lh = (f.size * 1.95).round();
                            b.y = rtl_paragraph(fonts, body, f, cx.width, lh, b.y, a, &mut b.items);
                        } else {
                            let g = wrapped(
                                fonts,
                                body,
                                FontId::new(cx.typo.size, theme::family(SERIF)),
                                cx.lh(),
                                cx.width,
                            );
                            let h = g.size().y;
                            b.items.push(Item {
                                pos: vec2(0.0, b.y),
                                galley: g,
                                role: Role::Ink,
                                ayah: Some(a),
                            });
                            b.y += h;
                        }
                    }
                }
                b.y += cx.s(14.0);
                // The ayah's number and divisions in the margin, on its first line.
                if cx.typo.ayah_numbers {
                    let n = one_line(fonts, &a.to_string(), FontId::new(12.0, theme::family(UI_SEMIBOLD)));
                    let ny = arabic_top + (lh - n.size().y) / 2.0;
                    let nx = -GUTTER + 8.0;
                    b.items.push(Item { pos: vec2(nx, ny), galley: n, role: Role::Verse, ayah: Some(a) });
                    let mut gy = ny + 18.0;
                    let mut note = |text: String, role: Role, b: &mut Builder<'_, '_>| {
                        let g = one_line(fonts, &text, FontId::new(10.0, theme::family(UI_MEDIUM)));
                        b.items.push(Item { pos: vec2(nx, gy), galley: g, role, ayah: Some(a) });
                        gy += 14.0;
                    };
                    if let Some(q) = m.quarter.filter(|_| cx.typo.headings && m.juz.is_none()) {
                        note(quarter_label(q), Role::Ink3, &mut b);
                    }
                    if m.sajda.is_some() {
                        note("Sajdah".into(), Role::Rubric, &mut b);
                    }
                }
                let band = Rect::from_min_max(pos2(-12.0, top + 4.0), pos2(cx.width + 12.0, b.y - 4.0));
                b.ayah_box(a, top, (arabic_top, lh), vec![band]);
            }
        }
        Mode::Reading => {
            b.y += cx.s(10.0);
            {
                let ayahs: Vec<u16> = (sec.from..=sec.to).collect();
                let units: Vec<Vec<Tok>> =
                    ayahs.iter().flat_map(|&a| ayah_units(fonts, cx, AyahRef::new(s.0, a))).collect();
                let mut rects = Vec::new();
                let top = b.y;
                b.y = flow_rtl(
                    units,
                    cx.width,
                    space,
                    lh,
                    cx.typo.justify,
                    b.y,
                    Role::Verse,
                    Role::Ink,
                    cx.typo.arabic_size,
                    &mut b.items,
                    &mut rects,
                );
                for a in ayahs {
                    let lines = merge_lines(&rects, a);
                    let t = lines.iter().map(|r| r.top()).fold(f32::MAX, f32::min).min(b.y);
                    let first_line = (t, lh);
                    let bottom = lines.iter().map(|r| r.bottom()).fold(top, f32::max);
                    b.ayahs.push(AyahBox { ayah: a, top: t, bottom, first_line, rects: lines });
                }
            }
            b.y += cx.s(10.0);
        }
    }
    if sec.ends_surah() {
        b.y += SURAH_END * cx.typo.size / 18.0;
    }
    if juz.end() == AyahRef::new(s.0, sec.to) {
        b.juz_foot(fonts, juz);
    }
    SectionLayout { section: index, height: b.y.ceil(), items: b.items, decos: b.decos, ayahs: b.ayahs }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_cover_the_quran() {
        let s = sections();
        assert!(s.len() > 556, "{}", s.len());
        let total: u32 = s.iter().map(|x| (x.to - x.from + 1) as u32).sum();
        assert_eq!(total, quran_core::AYAH_COUNT as u32);
        assert!(s[0].starts_surah() && s[0].ends_surah());
        assert_eq!(section_of(AyahRef::new(1, 7)), 0);
        assert_eq!(section_of(AyahRef::new(2, 1)), 1);
        assert_eq!(section_of(AyahRef::new(114, 6)) as usize, s.len() - 1);
        for (i, sec) in s.iter().enumerate() {
            assert_eq!(section_of(sec.first()) as usize, i);
            // Every section lies in one juz.
            assert_eq!(sec.first().juz(), AyahRef::new(sec.surah.0, sec.to).juz());
        }
        // The juz tile the sections.
        assert_eq!(*juz_sections(Juz(1)).start(), 0);
        assert_eq!(juz_sections(Juz(2)), section_of(AyahRef::new(2, 142))..=section_of(AyahRef::new(2, 252)));
        assert_eq!(sections()[*juz_sections(Juz(2)).start() as usize].from, 142);
        assert_eq!(*juz_sections(Juz(30)).end() as usize, s.len() - 1);
    }

    #[test]
    fn rtl_words() {
        assert_eq!(rtl_word("(2)"), "(2)");
        assert_eq!(rtl_word("("), ")");
        assert_eq!(rtl_word("سورة٢٥"), "سورة٥٢");
        assert_eq!(rtl_word("قرآن"), "قرآن");
    }
}
