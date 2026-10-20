//! Reading plans: a catalogue of day-by-day readings (a juz a day, a manzil a day, the whole
//! Quran in a year by mushaf pages…). Progress — which plans are under way, which passages are
//! read — lives in the study store.

use std::sync::OnceLock;

use crate::meta::{AYAH_COUNT, AyahRef, Juz, MANZIL_START, PAGE_COUNT, SurahId, page_start, quarter_start};

/// One reading: a run of ayahs, possibly across surahs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Passage {
    pub from: AyahRef,
    pub to: AyahRef,
}

impl Passage {
    pub fn new(from: AyahRef, to: AyahRef) -> Self {
        Self { from, to }
    }

    pub fn surah(s: SurahId) -> Self {
        Self { from: s.first_ayah(), to: s.last_ayah() }
    }

    pub fn ayahs(&self) -> u16 {
        self.to.index().saturating_sub(self.from.index()) + 1
    }

    /// "Al-Kahf", "Al-Baqarah 1–141", "Al-Fatihah 1 – Al-Baqarah 141", "Al-Mulk – Al-Mursalat".
    pub fn label(&self) -> String {
        let (a, b) = (self.from, self.to);
        let whole_start = a.ayah == 1;
        let whole_end = b.ayah == b.surah_id().ayahs();
        if a.surah == b.surah {
            if whole_start && whole_end {
                return a.surah_id().name().to_string();
            }
            return format!("{} {}–{}", a.surah_id().name(), a.ayah, b.ayah);
        }
        if whole_start && whole_end {
            return format!("{} – {}", a.surah_id().name(), b.surah_id().name());
        }
        format!("{} {} – {} {}", a.surah_id().name(), a.ayah, b.surah_id().name(), b.ayah)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanIcon {
    Calendar,
    Book,
    Quote,
    History,
    Flame,
    Note,
}

#[derive(Debug)]
pub struct Plan {
    pub id: &'static str,
    pub title: &'static str,
    /// Length and pace ("30 days · a juz a day").
    pub meta: &'static str,
    pub description: &'static str,
    pub icon: PlanIcon,
    pub days: Vec<Vec<Passage>>,
}

impl Plan {
    pub fn len(&self) -> usize {
        self.days.len()
    }

    pub fn is_empty(&self) -> bool {
        self.days.is_empty()
    }

    /// The passages for day `n` (1-based).
    pub fn day(&self, n: usize) -> &[Passage] {
        n.checked_sub(1).and_then(|i| self.days.get(i)).map_or(&[], Vec::as_slice)
    }
}

fn at(i: u16) -> AyahRef {
    AyahRef::from_index(i.min(AYAH_COUNT - 1)).expect("in range")
}

/// Splits a run at surah boundaries, so each passage reads within one surah where it can.
fn by_surah(from: u16, to: u16) -> Vec<Passage> {
    let mut out = Vec::new();
    let mut i = from;
    while i <= to {
        let a = at(i);
        let end = a.surah_id().range().end - 1;
        let e = end.min(to);
        out.push(Passage::new(a, at(e)));
        i = e + 1;
    }
    out
}

/// Global index ranges [start, end) cut into `days` days at the given start points.
fn days_from_starts(starts: &[u16], days: usize) -> Vec<Vec<Passage>> {
    let n = starts.len();
    (0..days)
        .map(|d| {
            let a = starts[d * n / days];
            let b = if (d + 1) * n / days >= n { AYAH_COUNT } else { starts[(d + 1) * n / days] };
            by_surah(a, b - 1)
        })
        .collect()
}

/// A day per juz, by surah.
fn juz_days() -> Vec<Vec<Passage>> {
    Juz::all().map(|j| by_surah(j.start().index(), j.end().index())).collect()
}

/// The 60 hizbs, one a day.
fn hizb_days() -> Vec<Vec<Passage>> {
    let starts: Vec<u16> = (0..60).filter_map(|h| quarter_start(h * 4 + 1)).map(|r| r.index()).collect();
    days_from_starts(&starts, 60)
}

fn page_days(days: usize) -> Vec<Vec<Passage>> {
    let starts: Vec<u16> = (1..=PAGE_COUNT).filter_map(page_start).map(|r| r.index()).collect();
    days_from_starts(&starts, days)
}

fn manzil_days() -> Vec<Vec<Passage>> {
    let starts: Vec<u16> = MANZIL_START.iter().map(|&(s, a)| AyahRef::new(s, a).index()).collect();
    days_from_starts(&starts, 7)
}

/// Whole surahs `first..=last` spread over `days`, balanced by length.
fn surah_days(first: u8, last: u8, days: usize) -> Vec<Vec<Passage>> {
    let surahs: Vec<SurahId> = (first..=last).map(SurahId).collect();
    let total: u32 = surahs.iter().map(|s| s.ayahs() as u32).sum();
    let mut out: Vec<Vec<Passage>> = vec![Vec::new(); days];
    let mut acc = 0u32;
    for s in surahs {
        let d = ((acc as u64 * days as u64) / total as u64).min(days as u64 - 1) as usize;
        out[d].push(Passage::surah(s));
        acc += s.ayahs() as u32;
    }
    out.retain(|d| !d.is_empty());
    out
}

fn build() -> Vec<Plan> {
    vec![
        Plan {
            id: "khatm-30",
            title: "End the Quran in 30 Days",
            meta: "30 days · a juz a day",
            description: "The whole Quran in a month, one juz each day — the Ramadan pace.",
            icon: PlanIcon::Calendar,
            days: juz_days(),
        },
        Plan {
            id: "khatm-60",
            title: "End the Quran in 60 Days",
            meta: "60 days · a hizb a day",
            description: "Half a juz each day, about ten pages: a steady two months.",
            icon: PlanIcon::Book,
            days: hizb_days(),
        },
        Plan {
            id: "manzil-7",
            title: "A Manzil a Day",
            meta: "7 days · a manzil a day",
            description: "The traditional weekly reading in seven stations, Friday to Thursday.",
            icon: PlanIcon::Flame,
            days: manzil_days(),
        },
        Plan {
            id: "quran-year",
            title: "Quran in a Year",
            meta: "365 days · about 2 pages a day",
            description: "Al-Fatihah to An-Nas by the pages of the mushaf, a few minutes a day.",
            icon: PlanIcon::History,
            days: page_days(365),
        },
        Plan {
            id: "juz-amma",
            title: "Juz 'Amma",
            meta: "15 days · the short surahs",
            description: "An-Naba to An-Nas: the surahs most often recited in prayer.",
            icon: PlanIcon::Quote,
            days: surah_days(78, 114, 15),
        },
        Plan {
            id: "seven-long",
            title: "The Seven Long Surahs",
            meta: "30 days · about 40 ayahs a day",
            description: "Al-Baqarah to Al-A'raf and At-Tawbah: the great Medinan and Meccan surahs.",
            icon: PlanIcon::Note,
            days: {
                let parts: Vec<SurahId> = [2u8, 3, 4, 5, 6, 7, 9].into_iter().map(SurahId).collect();
                let all: Vec<u16> = parts.iter().flat_map(|s| s.range()).collect();
                let days = 30;
                let n = all.len();
                (0..days)
                    .map(|d| {
                        let chunk = &all[d * n / days..(d + 1) * n / days];
                        // Contiguous runs inside the chunk (At-Tawbah follows Al-A'raf with a gap).
                        let mut out = Vec::new();
                        let mut start = chunk[0];
                        for w in chunk.windows(2) {
                            if w[1] != w[0] + 1 {
                                out.extend(by_surah(start, w[0]));
                                start = w[1];
                            }
                        }
                        out.extend(by_surah(start, *chunk.last().unwrap()));
                        out
                    })
                    .collect()
            },
        },
    ]
}

/// Every plan, in the order they're offered.
pub fn catalogue() -> &'static [Plan] {
    static PLANS: OnceLock<Vec<Plan>> = OnceLock::new();
    PLANS.get_or_init(build)
}

pub fn get(id: &str) -> Option<&'static Plan> {
    catalogue().iter().find(|p| p.id == id)
}

/// A day's passages as a short label.
pub fn label(passages: &[Passage]) -> String {
    match passages {
        [] => String::new(),
        [p] => p.label(),
        [first, .., last] => Passage::new(first.from, last.to).label(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn total(p: &Plan) -> u32 {
        p.days.iter().flatten().map(|x| x.ayahs() as u32).sum()
    }

    #[test]
    fn plans_cover_what_they_say() {
        for id in ["khatm-30", "khatm-60", "manzil-7", "quran-year"] {
            let p = get(id).unwrap();
            assert_eq!(total(p), AYAH_COUNT as u32, "{id}");
        }
        assert_eq!(get("khatm-30").unwrap().len(), 30);
        assert_eq!(get("khatm-60").unwrap().len(), 60);
        assert_eq!(get("manzil-7").unwrap().len(), 7);
        assert_eq!(get("quran-year").unwrap().len(), 365);
        assert_eq!(get("juz-amma").unwrap().len(), 15);
        assert_eq!(total(get("juz-amma").unwrap()), Juz(30).ayahs() as u32);
        let long: u32 = [2u8, 3, 4, 5, 6, 7, 9].iter().map(|s| SurahId(*s).ayahs() as u32).sum();
        assert_eq!(total(get("seven-long").unwrap()), long);
        assert_eq!(label(get("khatm-30").unwrap().day(1)), "Al-Fatihah 1 – Al-Baqarah 141");
        assert_eq!(label(get("khatm-30").unwrap().day(30)), "An-Naba – An-Nas");
        assert_eq!(Passage::surah(SurahId(18)).label(), "Al-Kahf");
        assert_eq!(Passage::new(AyahRef::new(2, 1), AyahRef::new(2, 141)).label(), "Al-Baqarah 1–141");
    }
}
