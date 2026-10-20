//! Parsing what people type to go somewhere: "2:255", "2 255-257", "baqarah 255", "al-kahf",
//! "yaseen" (common spellings and typos), "the cow", "البقرة", "36", "juz 30", "para 5",
//! "page 50", "hizb 12".

use crate::meta::{AyahRef, Juz, PAGE_COUNT, SurahId, quarter_start};
use crate::search::normalize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// An ayah, or a range of ayahs in one surah (`end` inclusive).
    Ayah {
        at: AyahRef,
        end: Option<u16>,
    },
    Surah(SurahId),
    Juz(Juz),
    /// A page of the Madani mushaf (1–604).
    Page(u16),
    /// A hizb (1–60).
    Hizb(u8),
}

impl Target {
    /// Where reading starts.
    pub fn start(&self) -> AyahRef {
        match *self {
            Target::Ayah { at, .. } => at,
            Target::Surah(s) => s.first_ayah(),
            Target::Juz(j) => j.start(),
            Target::Page(p) => crate::meta::page_start(p).unwrap_or(AyahRef::FIRST),
            Target::Hizb(h) => quarter_start(h * 4 - 3).unwrap_or(AyahRef::FIRST),
        }
    }

    /// Whether it names an ayah (the reader then flashes and selects it).
    pub fn is_ayah(&self) -> bool {
        matches!(self, Target::Ayah { .. })
    }

    pub fn display(&self) -> String {
        match *self {
            Target::Ayah { at, end } => {
                let mut s = format!("{} {}:{}", at.surah_id().name(), at.surah, at.ayah);
                if let Some(e) = end.filter(|&e| e > at.ayah) {
                    s.push_str(&format!("–{e}"));
                }
                s
            }
            Target::Surah(s) => format!("{} · {}", s.name(), s.english()),
            Target::Juz(j) => format!("Juz {} · {}", j.0, j.name()),
            Target::Page(p) => format!("Page {p}"),
            Target::Hizb(h) => format!("Hizb {h}"),
        }
    }
}

/// Formats a range of ayahs in one surah: "Al-Baqarah 2:255", "Al-Baqarah 2:255–257".
pub fn format_range(from: AyahRef, to: AyahRef) -> String {
    if from == to || from.surah != to.surah {
        return from.long();
    }
    format!("{} {}:{}–{}", from.surah_id().name(), from.surah, from.ayah, to.ayah)
}

/// The best interpretation of `input`, if any.
pub fn parse(input: &str) -> Option<Target> {
    suggest(input, 1).into_iter().next()
}

/// Ranked interpretations of `input`.
pub fn suggest(input: &str, limit: usize) -> Vec<Target> {
    let input = input.trim();
    if input.is_empty() || limit == 0 {
        return Vec::new();
    }
    let (words, nums) = split(input);
    let name = words.join(" ");
    let lower = name.to_lowercase();
    let n = |i: usize| nums.get(i).copied();

    // Divisions: "juz 30", "j30", "para 5", "part 5", "page 50", "p 50", "hizb 12".
    let division = |w: &str| -> Option<&'static str> {
        match w {
            "juz" | "juzz" | "j" | "para" | "parah" | "sipara" | "part" | "جزء" | "الجزء" => {
                Some("juz")
            }
            "page" | "p" | "pg" | "pages" | "صفحة" => Some("page"),
            "hizb" | "h" | "حزب" => Some("hizb"),
            _ => None,
        }
    };
    if let Some(kind) = division(lower.trim_end_matches('.')) {
        let t = match (kind, n(0)) {
            ("juz", Some(x)) => Juz::new(x).map(Target::Juz),
            ("page", Some(x)) if (1..=PAGE_COUNT as u32).contains(&x) => Some(Target::Page(x as u16)),
            ("hizb", Some(x)) if (1..=60).contains(&x) => Some(Target::Hizb(x as u8)),
            _ => None,
        };
        return t.into_iter().collect();
    }
    // "juz amma", "juz tabarak": a juz by its name.
    if let Some(rest) = lower.strip_prefix("juz ").or_else(|| lower.strip_prefix("para "))
        && let Some(j) = Juz::all()
            .find(|j| key(j.name()) == key(rest))
            .or_else(|| Juz::all().find(|j| key(j.name()).starts_with(&key(rest))))
    {
        return vec![Target::Juz(j)];
    }

    let mut out = Vec::new();
    if name.is_empty() {
        // Numbers only: "2:255", "2 255-257", "36".
        let Some(s) = n(0).and_then(SurahId::new) else { return out };
        out.push(match n(1) {
            Some(a) => match AyahRef::checked(s.0 as u32, a) {
                Some(at) => Target::Ayah { at, end: end_of(at, n(2)) },
                None => Target::Surah(s),
            },
            None => Target::Surah(s),
        });
        return out;
    }
    for s in surah_candidates(&name, limit.max(1)) {
        let t = match (n(0), n(1)) {
            // "baqarah 2:255" names both; the numbers then are surah and ayah.
            (Some(sn), Some(a)) if sn == s.0 as u32 => {
                AyahRef::checked(sn, a).map(|at| Target::Ayah { at, end: end_of(at, n(2)) })
            }
            (Some(a), e) => AyahRef::checked(s.0 as u32, a).map(|at| Target::Ayah { at, end: end_of(at, e) }),
            (None, _) => Some(Target::Surah(s)),
        };
        if let Some(t) = t {
            out.push(t);
            if out.len() >= limit {
                break;
            }
        }
    }
    out
}

fn end_of(at: AyahRef, e: Option<u32>) -> Option<u16> {
    e.filter(|&e| e > at.ayah as u32 && e <= at.surah_id().ayahs() as u32).map(|e| e as u16)
}

/// Separates the words of the input from its numbers ("baqarah 2:255" → ["baqarah"], [2, 255];
/// "j30" → ["j"], [30]). Arabic-Indic digits count.
fn split(input: &str) -> (Vec<String>, Vec<u32>) {
    let mut words = Vec::new();
    let mut nums = Vec::new();
    let mut word = String::new();
    let mut num = String::new();
    for c in input.chars() {
        let digit = c.to_digit(10).or(match c {
            '\u{0660}'..='\u{0669}' => Some(c as u32 - 0x0660),
            '\u{06F0}'..='\u{06F9}' => Some(c as u32 - 0x06F0),
            _ => None,
        });
        if let Some(d) = digit {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
            num.push(char::from_digit(d, 10).unwrap_or('0'));
            continue;
        }
        if !num.is_empty() {
            nums.extend(num.parse::<u32>().ok());
            num.clear();
        }
        if c.is_alphabetic() || is_mark(c) || c == '\'' || c == '’' || (c == '-' && !word.is_empty()) {
            word.push(c);
        } else if !word.is_empty() {
            words.push(std::mem::take(&mut word));
        }
    }
    nums.extend(num.parse::<u32>().ok());
    if !word.is_empty() {
        words.push(word);
    }
    let words = words
        .into_iter()
        .map(|w| w.trim_matches(|c: char| c == '-' || c == '\'' || c == '’').to_string())
        .filter(|w| !w.is_empty())
        .collect();
    (words, nums)
}

fn is_mark(c: char) -> bool {
    matches!(c, '\u{064B}'..='\u{065F}' | '\u{0670}' | '\u{06D6}'..='\u{06ED}')
}

const ARTICLES: [&str; 16] = [
    "al", "an", "ash", "at", "ar", "as", "az", "ad", "adh", "ath", "aal", "el", "surah", "surat", "sura",
    "ال",
];

/// A spelling-insensitive key: folded, letters only, articles dropped, doubled letters and
/// vowel spellings evened out ("Al-Faatiha" and "fatihah" → "fatiha").
pub fn key(s: &str) -> String {
    let n = normalize(s);
    let mut parts: Vec<&str> = n.split_whitespace().collect();
    while parts.len() > 1 && ARTICLES.contains(&parts[0]) {
        parts.remove(0);
    }
    let mut joined: String = parts.concat();
    // A hyphenated article ("an-nas" normalizes to "an nas" above); an Arabic "ال".
    if let Some(rest) = joined.strip_prefix("ال").filter(|r| r.chars().count() >= 2) {
        joined = rest.to_string();
    }
    let mut out = String::with_capacity(joined.len());
    let mut prev = None;
    for c in joined.chars() {
        let c = match c {
            'e' => 'i',
            'o' => 'u',
            'w' if prev.is_some_and(|p: char| "aiu".contains(p)) => 'u',
            'y' if prev.is_some_and(|p: char| "aiu".contains(p)) => 'i',
            c => c,
        };
        if Some(c) == prev {
            continue;
        }
        out.push(c);
        prev = Some(c);
    }
    // A final "h" after a vowel is often left off: "baqarah" / "baqara".
    if out.len() > 3 && out.ends_with('h') && out[..out.len() - 1].ends_with(['a', 'i', 'u']) {
        out.pop();
    }
    out
}

/// Every name a surah answers to, as keys.
fn surah_keys(s: SurahId) -> Vec<String> {
    let mut keys = vec![key(s.name()), key(s.tanzil_name()), key(s.english()), key(s.arabic())];
    keys.extend(s.aliases().iter().map(|a| key(a)));
    // "Ali 'Imran" also as "imran"; "the cow" as "cow".
    keys.push(key(s.english().trim_start_matches("The ")));
    if let Some(last) = s.name().split([' ', '-']).next_back() {
        keys.push(key(last));
    }
    keys.retain(|k| !k.is_empty());
    keys.sort();
    keys.dedup();
    keys
}

/// Surahs whose names are nearest to `typed`, best first.
pub fn surah_candidates(typed: &str, limit: usize) -> Vec<SurahId> {
    let k = key(typed);
    if k.is_empty() {
        return Vec::new();
    }
    let kc: Vec<char> = k.chars().collect();
    let mut scored: Vec<(u32, SurahId)> = Vec::new();
    for s in SurahId::all() {
        let mut best: Option<u32> = None;
        for name in surah_keys(s) {
            let nc: Vec<char> = name.chars().collect();
            let score = if name == k {
                Some(0)
            } else if name.starts_with(&k) {
                Some(10 + (nc.len() - kc.len()) as u32)
            } else if kc.len() >= 3 {
                let max = if kc.len() >= 7 {
                    3
                } else if kc.len() >= 5 {
                    2
                } else {
                    1
                };
                // Compare against the name's prefix of about the typed length too ("baqra" ~ "baqara").
                let full = distance(&kc, &nc, max);
                let prefix = (nc.len() > kc.len())
                    .then(|| distance(&kc, &nc[..kc.len().min(nc.len())], max))
                    .flatten();
                match (full, prefix) {
                    (Some(d), _) => Some(100 + d as u32 * 50),
                    (None, Some(d)) => Some(130 + d as u32 * 50 + (nc.len() - kc.len()) as u32),
                    _ => None,
                }
            } else {
                None
            };
            if let Some(sc) = score {
                best = Some(best.map_or(sc, |b: u32| b.min(sc)));
            }
        }
        if let Some(b) = best {
            scored.push((b, s));
        }
    }
    scored.sort_by_key(|(sc, s)| (*sc, s.0));
    scored.into_iter().take(limit).map(|(_, s)| s).collect()
}

fn distance(a: &[char], b: &[char], max: usize) -> Option<usize> {
    if a.len().abs_diff(b.len()) > max {
        return None;
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        let mut row_min = cur[0];
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
            row_min = row_min.min(cur[j]);
        }
        if row_min > max {
            return None;
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    (prev[b.len()] <= max).then_some(prev[b.len()])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Option<Target> {
        parse(s)
    }

    fn ayah(s: u8, a: u16) -> Option<Target> {
        Some(Target::Ayah { at: AyahRef::new(s, a), end: None })
    }

    #[test]
    fn numbers() {
        assert_eq!(p("2:255"), ayah(2, 255));
        assert_eq!(p("2 255"), ayah(2, 255));
        assert_eq!(p("2.255"), ayah(2, 255));
        assert_eq!(p("2:255-257"), Some(Target::Ayah { at: AyahRef::new(2, 255), end: Some(257) }));
        assert_eq!(p("36"), Some(Target::Surah(SurahId(36))));
        assert_eq!(p("٢:٢٥٥"), ayah(2, 255));
        assert_eq!(p("115"), None);
        assert_eq!(p("1:8"), Some(Target::Surah(SurahId(1))));
    }

    #[test]
    fn names_and_spellings() {
        assert_eq!(p("baqarah 255"), ayah(2, 255));
        assert_eq!(p("Al-Baqara 255"), ayah(2, 255));
        assert_eq!(p("al baqarah:255"), ayah(2, 255));
        assert_eq!(p("baqarah 2:255"), ayah(2, 255));
        assert_eq!(p("yaseen"), Some(Target::Surah(SurahId(36))));
        assert_eq!(p("yasin 12"), ayah(36, 12));
        assert_eq!(p("kahf"), Some(Target::Surah(SurahId(18))));
        assert_eq!(p("al-faatiha"), Some(Target::Surah(SurahId(1))));
        assert_eq!(p("rehman"), Some(Target::Surah(SurahId(55))));
        assert_eq!(p("the cow"), Some(Target::Surah(SurahId(2))));
        assert_eq!(p("ikhlas"), Some(Target::Surah(SurahId(112))));
        assert_eq!(p("an-nas"), Some(Target::Surah(SurahId(114))));
        assert_eq!(p("nas"), Some(Target::Surah(SurahId(114))));
        assert_eq!(p("mulk 1-5"), Some(Target::Ayah { at: AyahRef::new(67, 1), end: Some(5) }));
        assert_eq!(p("ali imran 18"), ayah(3, 18));
        assert_eq!(p("البقرة ٢٥٥"), ayah(2, 255));
        assert_eq!(p("الكهف"), Some(Target::Surah(SurahId(18))));
        assert_eq!(p("tauba"), Some(Target::Surah(SurahId(9))));
        assert_eq!(p("waqiah"), Some(Target::Surah(SurahId(56))));
        assert_eq!(p("baqra"), Some(Target::Surah(SurahId(2))));
    }

    #[test]
    fn divisions() {
        assert_eq!(p("juz 30"), Some(Target::Juz(Juz(30))));
        assert_eq!(p("j30"), Some(Target::Juz(Juz(30))));
        assert_eq!(p("para 5"), Some(Target::Juz(Juz(5))));
        assert_eq!(p("juz amma"), Some(Target::Juz(Juz(30))));
        assert_eq!(p("page 50"), Some(Target::Page(50)));
        assert_eq!(p("p 604"), Some(Target::Page(604)));
        assert_eq!(p("hizb 12"), Some(Target::Hizb(12)));
        assert_eq!(p("juz 31"), None);
        assert_eq!(Target::Juz(Juz(2)).start(), AyahRef::new(2, 142));
        assert_eq!(Target::Hizb(2).start(), AyahRef::new(2, 75));
    }

    #[test]
    fn several_suggestions() {
        let s = suggest("an", 5);
        assert!(s.len() >= 3, "{s:?}");
        assert_eq!(format_range(AyahRef::new(2, 255), AyahRef::new(2, 257)), "Al-Baqarah 2:255–257");
    }
}
