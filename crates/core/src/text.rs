//! The Arabic text (Tanzil's Uthmani script, verbatim) and its simple form for search, and the
//! line-per-ayah text format translations share.

use std::sync::OnceLock;

use anyhow::{Result, bail};

use crate::meta::{AYAH_COUNT, AyahRef};

/// 6,236 texts, one per ayah, in one string.
#[derive(Clone)]
pub struct Lines {
    text: String,
    /// Start of each ayah's text in `text`, plus one past the end.
    starts: Vec<u32>,
}

impl Lines {
    /// One ayah per line; exactly 6,236 lines.
    pub fn parse(text: String) -> Result<Self> {
        let mut starts = Vec::with_capacity(AYAH_COUNT as usize + 1);
        starts.push(0);
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                starts.push(i as u32 + 1);
            }
        }
        if text.ends_with('\n') {
            starts.pop();
        }
        if starts.len() != AYAH_COUNT as usize {
            bail!("{} lines, expected {AYAH_COUNT}", starts.len());
        }
        starts.push(text.len() as u32 + 1);
        Ok(Self { text, starts })
    }

    pub fn from_vec(lines: &[String]) -> Result<Self> {
        if lines.iter().any(|l| l.contains('\n')) {
            bail!("a line break inside an ayah's text");
        }
        Self::parse(lines.join("\n"))
    }

    /// The text of the ayah at global index `i`.
    pub fn at(&self, i: u16) -> &str {
        let i = i as usize;
        let (a, b) = (self.starts[i] as usize, self.starts[i + 1] as usize - 1);
        self.text.get(a..b.max(a).min(self.text.len())).unwrap_or("")
    }

    pub fn get(&self, r: AyahRef) -> &str {
        self.at(r.index())
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn len_bytes(&self) -> usize {
        self.text.len()
    }
}

pub struct Quran {
    pub uthmani: Lines,
    pub simple: Lines,
}

#[cfg(feature = "bundled")]
static UTHMANI: &[u8] = include_bytes!("../../../data/quran/uthmani.zst");
#[cfg(feature = "bundled")]
static SIMPLE: &[u8] = include_bytes!("../../../data/quran/simple.zst");

/// The Tanzil Project's copyright notice, which must travel with the text.
#[cfg(feature = "bundled")]
pub const NOTICE: &str = include_str!("../../../data/quran/NOTICE.txt");

fn decode(bytes: &[u8]) -> Result<Lines> {
    let raw = zstd::decode_all(bytes)?;
    Lines::parse(String::from_utf8(raw)?)
}

/// The Quran, decoded on first use (about 10 ms).
#[cfg(feature = "bundled")]
pub fn quran() -> &'static Quran {
    static Q: OnceLock<Quran> = OnceLock::new();
    Q.get_or_init(|| Quran {
        uthmani: decode(UTHMANI).expect("bundled Uthmani text"),
        simple: decode(SIMPLE).expect("bundled simple text"),
    })
}

/// The Basmala as it opens every surah but At-Tawbah.
pub const BASMALA: &str = "\u{628}\u{650}\u{633}\u{652}\u{645}\u{650} \u{671}\u{644}\u{644}\u{651}\u{64e}\u{647}\u{650} \u{671}\u{644}\u{631}\u{651}\u{64e}\u{62d}\u{652}\u{645}\u{64e}\u{670}\u{646}\u{650} \u{671}\u{644}\u{631}\u{651}\u{64e}\u{62d}\u{650}\u{64a}\u{645}\u{650}";

/// Whether the first ayah of `surah` carries the Basmala as a prefix in Tanzil's text (every
/// surah but Al-Fatihah, where it *is* the first ayah, and At-Tawbah, which has none).
fn basmala_prefixed(r: AyahRef) -> bool {
    r.ayah == 1 && r.surah != 1 && r.surah != 9
}

/// Drops the first four words (the Basmala; surahs 95 and 97 spell it with a shadda).
fn strip_basmala(s: &str) -> &str {
    let mut rest = s;
    for _ in 0..4 {
        rest = rest.trim_start();
        match rest.find(' ') {
            Some(i) => rest = &rest[i..],
            None => return "",
        }
    }
    rest.trim_start()
}

impl Quran {
    /// The ayah as Tanzil gives it (with the Basmala before the first ayah of most surahs).
    pub fn verbatim(&self, r: AyahRef) -> &str {
        self.uthmani.get(r)
    }

    /// The ayah's own text, for display: the Basmala that opens a surah is shown as its heading.
    pub fn arabic(&self, r: AyahRef) -> &str {
        let t = self.uthmani.get(r);
        if basmala_prefixed(r) { strip_basmala(t) } else { t }
    }

    /// The same in simple script without diacritics (search, plain copies).
    pub fn simple(&self, r: AyahRef) -> &str {
        let t = self.simple.get(r);
        if basmala_prefixed(r) { strip_basmala(t) } else { t }
    }

    /// Words of an ayah for typesetting. Pause marks (small high letters standing alone, such as
    /// ۚ) belong over the end of the word before them, so they join it.
    pub fn words(&self, r: AyahRef) -> Vec<String> {
        words(self.arabic(r))
    }
}

/// See [`Quran::words`].
pub fn words(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for w in text.split(' ').filter(|w| !w.is_empty()) {
        let mark = w.chars().all(is_pause_mark);
        match out.last_mut() {
            Some(last) if mark => {
                last.push(' ');
                last.push_str(w);
            }
            _ => out.push(w.to_string()),
        }
    }
    out
}

/// Quranic annotation signs that sit above a word (U+06D6–U+06DC).
pub fn is_pause_mark(c: char) -> bool {
    ('\u{06D6}'..='\u{06DC}').contains(&c)
}

/// Whether a character is from an Arabic-script block.
pub fn is_arabic(c: char) -> bool {
    matches!(c, '\u{0600}'..='\u{06FF}' | '\u{0750}'..='\u{077F}' | '\u{08A0}'..='\u{08FF}' | '\u{FB50}'..='\u{FDFF}' | '\u{FE70}'..='\u{FEFF}')
}

/// Whether a character is written right to left (Arabic and Hebrew scripts).
pub fn is_rtl(c: char) -> bool {
    is_arabic(c) || ('\u{0590}'..='\u{05FF}').contains(&c)
}

/// `n` in Arabic-Indic digits: 255 → ٢٥٥.
pub fn arabic_number(n: u32) -> String {
    n.to_string().chars().map(|c| char::from_u32(c as u32 - '0' as u32 + 0x0660).unwrap_or(c)).collect()
}

#[cfg(all(test, feature = "bundled"))]
mod tests {
    use super::*;

    #[test]
    fn text_and_basmala() {
        let q = quran();
        assert_eq!(q.arabic(AyahRef::new(1, 1)), BASMALA);
        assert_eq!(q.arabic(AyahRef::new(2, 1)), "الٓمٓ");
        assert!(q.verbatim(AyahRef::new(2, 1)).starts_with(BASMALA));
        assert_eq!(q.arabic(AyahRef::new(9, 1)), q.verbatim(AyahRef::new(9, 1)));
        let wc = |s: &str| s.split(' ').count();
        assert_eq!(wc(q.arabic(AyahRef::new(95, 1))) + 4, wc(q.verbatim(AyahRef::new(95, 1))));
        assert!(q.arabic(AyahRef::new(95, 1)).starts_with('\u{648}'));
        assert_eq!(q.simple(AyahRef::new(2, 1)), "الم");
        assert_eq!(q.arabic(AyahRef::new(114, 6)).split(' ').count(), 3);
    }

    #[test]
    fn pause_marks_join_the_word_before() {
        let w = words("لَا رَيْبَ ۛ فِيهِ ۛ هُدًى");
        assert_eq!(w, vec!["لَا", "رَيْبَ ۛ", "فِيهِ ۛ", "هُدًى"]);
        assert_eq!(arabic_number(255), "٢٥٥");
    }
}
