//! The Quran's structure: 114 surahs, 6,236 ayahs, and the divisions laid over them — 30 juz,
//! 60 hizb (240 quarters), 7 manzil, 556 rukus, 604 Madani mushaf pages and 15 sajdas.
//!
//! Positions are [`AyahRef`]s (surah and ayah, both from 1) or global indices (0–6235, the
//! ayah's place in the whole text); the tables come from Tanzil's metadata (`meta_gen.rs`).

use std::fmt;

#[allow(clippy::all)]
mod generated {
    include!("meta_gen.rs");
}
pub(crate) use generated::*;

pub const SURAH_COUNT: u8 = 114;
pub const AYAH_COUNT: u16 = 6236;
pub const JUZ_COUNT: u8 = 30;
pub const PAGE_COUNT: u16 = 604;

/// A surah, numbered from 1 to 114.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SurahId(pub u8);

impl SurahId {
    pub const FIRST: SurahId = SurahId(1);
    pub const LAST: SurahId = SurahId(SURAH_COUNT);

    pub fn new(n: u32) -> Option<SurahId> {
        (1..=SURAH_COUNT as u32).contains(&n).then_some(SurahId(n as u8))
    }

    pub fn all() -> impl DoubleEndedIterator<Item = SurahId> + ExactSizeIterator {
        (1..=SURAH_COUNT).map(SurahId)
    }

    fn i(self) -> usize {
        (self.0.clamp(1, SURAH_COUNT) - 1) as usize
    }

    pub fn number(self) -> u8 {
        self.0
    }

    pub fn ayahs(self) -> u16 {
        SURAH_AYAHS[self.i()]
    }

    /// The global index of its first ayah.
    pub fn start(self) -> u16 {
        SURAH_START[self.i()]
    }

    /// Its global ayah indices.
    pub fn range(self) -> std::ops::Range<u16> {
        self.start()..self.start() + self.ayahs()
    }

    pub fn first_ayah(self) -> AyahRef {
        AyahRef::new(self.0, 1)
    }

    pub fn last_ayah(self) -> AyahRef {
        AyahRef::new(self.0, self.ayahs())
    }

    /// The common transliteration: "Al-Baqarah".
    pub fn name(self) -> &'static str {
        NAMES[self.i()]
    }

    /// The name in Arabic: "البقرة".
    pub fn arabic(self) -> &'static str {
        SURAH_ARABIC[self.i()]
    }

    /// What the name means: "The Cow".
    pub fn english(self) -> &'static str {
        SURAH_ENGLISH[self.i()]
    }

    pub fn medinan(self) -> bool {
        SURAH_MEDINAN[self.i()]
    }

    pub fn revelation(self) -> &'static str {
        if self.medinan() { "Medinan" } else { "Meccan" }
    }

    /// Its place in the order of revelation (1–114).
    pub fn revelation_order(self) -> u8 {
        SURAH_ORDER[self.i()]
    }

    pub fn rukus(self) -> u8 {
        SURAH_RUKUS[self.i()]
    }

    /// Every surah but At-Tawbah opens with the Basmala; in Al-Fatihah it is the first ayah.
    pub fn has_basmala_heading(self) -> bool {
        self.0 != 1 && self.0 != 9
    }

    /// The juz it starts in and the one it ends in.
    pub fn juz_span(self) -> (u8, u8) {
        (juz_of(self.first_ayah()), juz_of(self.last_ayah()))
    }

    pub fn prev(self) -> Option<SurahId> {
        (self.0 > 1).then(|| SurahId(self.0 - 1))
    }

    pub fn next(self) -> Option<SurahId> {
        (self.0 < SURAH_COUNT).then(|| SurahId(self.0 + 1))
    }

    /// Other names it is known by (for the jump palette and the reference parser).
    pub fn aliases(self) -> &'static [&'static str] {
        ALIASES.iter().find(|(n, _)| *n == self.0).map_or(&[], |(_, a)| a)
    }

    /// Tanzil's transliteration ("Al-Faatiha"), another spelling people type.
    pub fn tanzil_name(self) -> &'static str {
        SURAH_TANZIL_NAME[self.i()]
    }
}

impl fmt::Display for SurahId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// One ayah: surah and ayah number, both from 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AyahRef {
    pub surah: u8,
    pub ayah: u16,
}

impl AyahRef {
    pub const FIRST: AyahRef = AyahRef { surah: 1, ayah: 1 };

    pub const fn new(surah: u8, ayah: u16) -> Self {
        Self { surah, ayah }
    }

    /// A reference that exists (surah 1–114, ayah within it).
    pub fn checked(surah: u32, ayah: u32) -> Option<Self> {
        let s = SurahId::new(surah)?;
        (ayah >= 1 && ayah <= s.ayahs() as u32).then(|| Self::new(s.0, ayah as u16))
    }

    pub fn surah_id(self) -> SurahId {
        SurahId(self.surah)
    }

    pub fn is_valid(self) -> bool {
        Self::checked(self.surah as u32, self.ayah as u32).is_some()
    }

    /// Place in the whole text, 0–6235.
    pub fn index(self) -> u16 {
        self.surah_id().start() + self.ayah.saturating_sub(1).min(self.surah_id().ayahs() - 1)
    }

    pub fn from_index(i: u16) -> Option<AyahRef> {
        if i >= AYAH_COUNT {
            return None;
        }
        let s = SURAH_START.partition_point(|&st| st <= i);
        Some(AyahRef::new(s as u8, i - SURAH_START[s - 1] + 1))
    }

    /// A compact key for maps: surah in the high bits.
    pub fn key(self) -> u32 {
        (self.surah as u32) << 16 | self.ayah as u32
    }

    pub fn from_key(k: u32) -> Option<AyahRef> {
        let r = AyahRef::new((k >> 16) as u8, (k & 0xFFFF) as u16);
        r.is_valid().then_some(r)
    }

    pub fn next(self) -> Option<AyahRef> {
        AyahRef::from_index(self.index() + 1)
    }

    pub fn prev(self) -> Option<AyahRef> {
        self.index().checked_sub(1).and_then(AyahRef::from_index)
    }

    /// "Al-Baqarah 2:255".
    pub fn long(self) -> String {
        format!("{} {}:{}", self.surah_id().name(), self.surah, self.ayah)
    }

    pub fn juz(self) -> u8 {
        juz_of(self)
    }

    pub fn page(self) -> u16 {
        page_of(self)
    }
}

impl fmt::Display for AyahRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.surah, self.ayah)
    }
}

fn starts_index(table: &[(u8, u16)]) -> impl Fn(usize) -> u16 + '_ {
    |i| AyahRef::new(table[i].0, table[i].1).index()
}

/// Which of `table`'s divisions (from 1) `r` falls in.
fn division_of(table: &[(u8, u16)], r: AyahRef) -> usize {
    let at = starts_index(table);
    let i = r.index();
    let mut lo = 0;
    let mut hi = table.len();
    while lo < hi {
        let mid = (lo + hi) / 2;
        if at(mid) <= i { lo = mid + 1 } else { hi = mid }
    }
    lo.max(1)
}

pub fn juz_of(r: AyahRef) -> u8 {
    division_of(&JUZ_START, r) as u8
}

/// The hizb quarter (1–240); hizb n is quarters 4n−3 … 4n.
pub fn quarter_of(r: AyahRef) -> u8 {
    division_of(&HIZB_QUARTER_START, r) as u8
}

pub fn manzil_of(r: AyahRef) -> u8 {
    division_of(&MANZIL_START, r) as u8
}

pub fn ruku_of(r: AyahRef) -> u16 {
    division_of(&RUKU_START, r) as u16
}

/// The page of the Madani mushaf (1–604) the ayah starts on.
pub fn page_of(r: AyahRef) -> u16 {
    division_of(&PAGE_START, r) as u16
}

/// A juz, 1–30.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Juz(pub u8);

impl Juz {
    pub fn new(n: u32) -> Option<Juz> {
        (1..=JUZ_COUNT as u32).contains(&n).then_some(Juz(n as u8))
    }

    pub fn all() -> impl DoubleEndedIterator<Item = Juz> + ExactSizeIterator {
        (1..=JUZ_COUNT).map(Juz)
    }

    fn i(self) -> usize {
        (self.0.clamp(1, JUZ_COUNT) - 1) as usize
    }

    pub fn start(self) -> AyahRef {
        let (s, a) = JUZ_START[self.i()];
        AyahRef::new(s, a)
    }

    pub fn end(self) -> AyahRef {
        match Juz::new(self.0 as u32 + 1) {
            Some(n) => n.start().prev().unwrap_or(AyahRef::FIRST),
            None => SurahId::LAST.last_ayah(),
        }
    }

    /// Global indices of its ayahs.
    pub fn range(self) -> std::ops::Range<u16> {
        self.start().index()..self.end().index() + 1
    }

    pub fn ayahs(self) -> u16 {
        self.end().index() - self.start().index() + 1
    }

    /// Its traditional name, from its opening words: "Sayaqul".
    pub fn name(self) -> &'static str {
        JUZ_NAMES[self.i()]
    }

    /// Its name in Arabic: "سيقول".
    pub fn arabic(self) -> &'static str {
        JUZ_ARABIC[self.i()]
    }

    /// The mushaf pages it spans (a juz is about twenty).
    pub fn pages(self) -> u16 {
        page_of(self.end()) - page_of(self.start()) + 1
    }

    /// The surahs it touches, in order.
    pub fn surahs(self) -> impl Iterator<Item = SurahId> {
        (self.start().surah..=self.end().surah).map(SurahId)
    }
}

/// The start of hizb quarter `q` (1–240).
pub fn quarter_start(q: u8) -> Option<AyahRef> {
    let (s, a) = *HIZB_QUARTER_START.get((q as usize).checked_sub(1)?)?;
    Some(AyahRef::new(s, a))
}

/// The first ayah of page `p` (1–604).
pub fn page_start(p: u16) -> Option<AyahRef> {
    let (s, a) = *PAGE_START.get((p as usize).checked_sub(1)?)?;
    Some(AyahRef::new(s, a))
}

/// What starts at an ayah: a juz, a hizb or one of its quarters, a ruku.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Marks {
    pub juz: Option<u8>,
    /// Hizb quarter (1–240) starting here.
    pub quarter: Option<u8>,
    pub ruku: bool,
    pub page: Option<u16>,
    /// A prostration: `Some(true)` where it is obligatory (in the four schools' majority view).
    pub sajda: Option<bool>,
}

pub fn marks(r: AyahRef) -> Marks {
    let starts = |t: &[(u8, u16)]| t.iter().position(|&(s, a)| s == r.surah && a == r.ayah);
    Marks {
        juz: starts(&JUZ_START).map(|i| i as u8 + 1),
        quarter: starts(&HIZB_QUARTER_START).map(|i| i as u8 + 1),
        ruku: starts(&RUKU_START).is_some(),
        page: starts(&PAGE_START).map(|i| i as u16 + 1),
        sajda: SAJDA.iter().find(|&&(s, a, _)| s == r.surah && a == r.ayah).map(|&(_, _, o)| o),
    }
}

/// "Hizb 3", "¼ Hizb 3", "½ Hizb 3", "¾ Hizb 3" for quarter 9–12.
pub fn quarter_label(q: u8) -> String {
    let hizb = (q as u32 - 1) / 4 + 1;
    match (q - 1) % 4 {
        0 => format!("Hizb {hizb}"),
        1 => format!("¼ Hizb {hizb}"),
        2 => format!("½ Hizb {hizb}"),
        _ => format!("¾ Hizb {hizb}"),
    }
}

pub fn sajdas() -> impl Iterator<Item = (AyahRef, bool)> {
    SAJDA.iter().map(|&(s, a, o)| (AyahRef::new(s, a), o))
}

const NAMES: [&str; 114] = [
    "Al-Fatihah",
    "Al-Baqarah",
    "Ali 'Imran",
    "An-Nisa",
    "Al-Ma'idah",
    "Al-An'am",
    "Al-A'raf",
    "Al-Anfal",
    "At-Tawbah",
    "Yunus",
    "Hud",
    "Yusuf",
    "Ar-Ra'd",
    "Ibrahim",
    "Al-Hijr",
    "An-Nahl",
    "Al-Isra",
    "Al-Kahf",
    "Maryam",
    "Taha",
    "Al-Anbiya",
    "Al-Hajj",
    "Al-Mu'minun",
    "An-Nur",
    "Al-Furqan",
    "Ash-Shu'ara",
    "An-Naml",
    "Al-Qasas",
    "Al-'Ankabut",
    "Ar-Rum",
    "Luqman",
    "As-Sajdah",
    "Al-Ahzab",
    "Saba",
    "Fatir",
    "Ya-Sin",
    "As-Saffat",
    "Sad",
    "Az-Zumar",
    "Ghafir",
    "Fussilat",
    "Ash-Shura",
    "Az-Zukhruf",
    "Ad-Dukhan",
    "Al-Jathiyah",
    "Al-Ahqaf",
    "Muhammad",
    "Al-Fath",
    "Al-Hujurat",
    "Qaf",
    "Adh-Dhariyat",
    "At-Tur",
    "An-Najm",
    "Al-Qamar",
    "Ar-Rahman",
    "Al-Waqi'ah",
    "Al-Hadid",
    "Al-Mujadilah",
    "Al-Hashr",
    "Al-Mumtahanah",
    "As-Saff",
    "Al-Jumu'ah",
    "Al-Munafiqun",
    "At-Taghabun",
    "At-Talaq",
    "At-Tahrim",
    "Al-Mulk",
    "Al-Qalam",
    "Al-Haqqah",
    "Al-Ma'arij",
    "Nuh",
    "Al-Jinn",
    "Al-Muzzammil",
    "Al-Muddaththir",
    "Al-Qiyamah",
    "Al-Insan",
    "Al-Mursalat",
    "An-Naba",
    "An-Nazi'at",
    "'Abasa",
    "At-Takwir",
    "Al-Infitar",
    "Al-Mutaffifin",
    "Al-Inshiqaq",
    "Al-Buruj",
    "At-Tariq",
    "Al-A'la",
    "Al-Ghashiyah",
    "Al-Fajr",
    "Al-Balad",
    "Ash-Shams",
    "Al-Layl",
    "Ad-Duha",
    "Ash-Sharh",
    "At-Tin",
    "Al-'Alaq",
    "Al-Qadr",
    "Al-Bayyinah",
    "Az-Zalzalah",
    "Al-'Adiyat",
    "Al-Qari'ah",
    "At-Takathur",
    "Al-'Asr",
    "Al-Humazah",
    "Al-Fil",
    "Quraysh",
    "Al-Ma'un",
    "Al-Kawthar",
    "Al-Kafirun",
    "An-Nasr",
    "Al-Masad",
    "Al-Ikhlas",
    "Al-Falaq",
    "An-Nas",
];

/// Other names in common use.
const ALIASES: [(u8, &[&str]); 30] = [
    (1, &["Fatiha", "Umm al-Kitab", "The Opening"]),
    (2, &["Baqara"]),
    (3, &["Imran", "Al Imran", "Aal Imran"]),
    (5, &["Maida", "Maidah"]),
    (9, &["Tauba", "Taubah", "Bara'ah", "Baraat"]),
    (17, &["Bani Isra'il", "Bani Israil", "Isra"]),
    (20, &["Ta-Ha", "Ta Ha"]),
    (23, &["Muminun", "Mominoon"]),
    (36, &["Yasin", "Yaseen", "Ya Sin"]),
    (38, &["Saad", "Suad"]),
    (40, &["Al-Mu'min", "Mumin", "Momin"]),
    (41, &["Ha Mim Sajdah", "Hamim Sajdah"]),
    (55, &["Rahman", "Rehman"]),
    (56, &["Waqiah", "Waqia"]),
    (62, &["Jumuah", "Juma", "Jumah"]),
    (67, &["Tabarak"]),
    (73, &["Muzammil"]),
    (74, &["Muddassir", "Mudassir"]),
    (76, &["Ad-Dahr", "Dahr"]),
    (78, &["Naba", "Amma"]),
    (87, &["Ala"]),
    (94, &["Al-Inshirah", "Inshirah", "Alam Nashrah"]),
    (96, &["Iqra"]),
    (98, &["Bayyina"]),
    (99, &["Zilzal", "Zilzaal"]),
    (106, &["Quraish", "Qureysh"]),
    (108, &["Kauthar", "Kausar"]),
    (109, &["Kafiroon"]),
    (111, &["Al-Lahab", "Lahab", "Tabbat"]),
    (112, &["Ikhlaas", "Tawhid", "Qul Huwallahu"]),
];

const JUZ_NAMES: [&str; 30] = [
    "Alif Lam Mim",
    "Sayaqul",
    "Tilka ar-Rusul",
    "Lan Tanalu",
    "Wal-Muhsanat",
    "La Yuhibbullah",
    "Wa Idha Sami'u",
    "Wa Law Annana",
    "Qal al-Mala'u",
    "Wa'lamu",
    "Ya'tadhirun",
    "Wa Ma Min Dabbah",
    "Wa Ma Ubarri'u",
    "Rubama",
    "Subhanalladhi",
    "Qal Alam",
    "Iqtaraba",
    "Qad Aflaha",
    "Wa Qalalladhina",
    "Amman Khalaq",
    "Utlu Ma Uhiya",
    "Wa Man Yaqnut",
    "Wa Mali",
    "Faman Azlamu",
    "Ilayhi Yuraddu",
    "Ha Mim",
    "Qala Fama Khatbukum",
    "Qad Sami'allah",
    "Tabarakalladhi",
    "'Amma",
];

const JUZ_ARABIC: [&str; 30] = [
    "الم",
    "سيقول",
    "تلك الرسل",
    "لن تنالوا",
    "والمحصنات",
    "لا يحب الله",
    "وإذا سمعوا",
    "ولو أننا",
    "قال الملأ",
    "واعلموا",
    "يعتذرون",
    "وما من دابة",
    "وما أبرئ",
    "ربما",
    "سبحان الذي",
    "قال ألم",
    "اقترب للناس",
    "قد أفلح",
    "وقال الذين",
    "أمن خلق",
    "اتل ما أوحي",
    "ومن يقنت",
    "وما لي",
    "فمن أظلم",
    "إليه يرد",
    "حم",
    "قال فما خطبكم",
    "قد سمع الله",
    "تبارك الذي",
    "عم",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_add_up() {
        let total: u32 = SurahId::all().map(|s| s.ayahs() as u32).sum();
        assert_eq!(total, AYAH_COUNT as u32);
        assert_eq!(SurahId(114).range().end, AYAH_COUNT);
        let juz_total: u32 = Juz::all().map(|j| j.ayahs() as u32).sum();
        assert_eq!(juz_total, AYAH_COUNT as u32);
    }

    #[test]
    fn indices_round_trip() {
        for i in 0..AYAH_COUNT {
            let r = AyahRef::from_index(i).unwrap();
            assert!(r.is_valid(), "{r}");
            assert_eq!(r.index(), i);
            assert_eq!(AyahRef::from_key(r.key()), Some(r));
        }
        assert_eq!(AyahRef::from_index(AYAH_COUNT), None);
        assert_eq!(AyahRef::new(2, 255).index(), 7 + 254);
    }

    #[test]
    fn divisions() {
        assert_eq!(juz_of(AyahRef::new(1, 1)), 1);
        assert_eq!(juz_of(AyahRef::new(2, 141)), 1);
        assert_eq!(juz_of(AyahRef::new(2, 142)), 2);
        assert_eq!(juz_of(AyahRef::new(114, 6)), 30);
        assert_eq!(Juz(30).start(), AyahRef::new(78, 1));
        assert_eq!(Juz(1).end(), AyahRef::new(2, 141));
        assert_eq!(Juz(2).arabic(), "سيقول");
        assert_eq!(Juz(30).pages(), 23);
        assert_eq!(page_of(AyahRef::new(2, 1)), 2);
        assert_eq!(page_of(AyahRef::new(114, 6)), 604);
        assert_eq!(quarter_of(AyahRef::new(2, 26)), 2);
        assert_eq!(quarter_label(10), "¼ Hizb 3");
        assert_eq!(SurahId(2).juz_span(), (1, 3));
        let m = marks(AyahRef::new(32, 15));
        assert_eq!(m.sajda, Some(true));
        assert_eq!(marks(AyahRef::new(2, 142)).juz, Some(2));
        assert_eq!(SurahId(2).name(), "Al-Baqarah");
        assert_eq!(SurahId(36).aliases()[0], "Yasin");
    }
}
