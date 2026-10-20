//! Full-text search over a translation or the Arabic text.
//!
//! Each text lazily builds a normalized corpus — lower-case, diacritics folded (Latin accents,
//! Arabic harakat and Quranic marks), alef/yaa/taa-marbuta forms unified, apostrophes joined,
//! everything else a single space — laid out as `" w1 w2 … "` per ayah with a parallel array of
//! ayah start offsets. A term is a SIMD substring scan (`memchr::memmem`) for `" term"` (word
//! prefix) or `" term "` (whole word); phrases are longer needles. Every ayah is wrapped in
//! spaces and needles never hold two spaces in a row, so a match can't straddle two ayahs.
//!
//! Ported from bible-rs (MIT), which measured single-term queries well under a millisecond.

use std::ops::Range;
use std::time::{Duration, Instant};

use memchr::memmem;

use crate::meta::{AYAH_COUNT, AyahRef, Juz, SurahId};
use crate::text::{Lines, is_arabic};

pub struct SearchIndex {
    corpus: Vec<u8>,
    /// `starts[i]` = offset of ayah `i`'s leading space; one extra entry at the end.
    starts: Vec<u32>,
}

impl SearchIndex {
    pub fn build(lines: &Lines) -> Self {
        Self::build_from((0..AYAH_COUNT).map(|i| lines.at(i)))
    }

    /// From 6,236 texts in order.
    pub fn build_from<'a>(texts: impl Iterator<Item = &'a str>) -> Self {
        let mut corpus = Vec::with_capacity(1 << 20);
        let mut starts = Vec::with_capacity(AYAH_COUNT as usize + 1);
        for t in texts {
            starts.push(corpus.len() as u32);
            corpus.push(b' ');
            normalize_into(t, &mut corpus);
            push_space(&mut corpus);
        }
        starts.push(corpus.len() as u32);
        corpus.shrink_to_fit();
        Self { corpus, starts }
    }

    fn ayah_at(&self, pos: usize) -> u16 {
        (self.starts.partition_point(|&s| s as usize <= pos) - 1) as u16
    }

    fn len(&self) -> u16 {
        (self.starts.len() - 1) as u16
    }

    /// Ayahs (global indices, in order) matching every term of `query` within `scope`.
    pub fn search(&self, query: &Query, scope: Scope) -> Vec<u16> {
        let range = scope.range();
        let range = range.start.min(self.len())..range.end.min(self.len());
        if query.terms.is_empty() || range.is_empty() {
            return Vec::new();
        }
        let mut needles: Vec<Vec<u8>> = query.terms.iter().map(Term::needle).collect();
        // Longer needles are rarer: scanning for them first keeps the candidate set small.
        needles.sort_by_key(|n| std::cmp::Reverse(n.len()));
        let mut hits = self.scan(&needles[0], range.clone());
        for needle in &needles[1..] {
            if hits.is_empty() {
                break;
            }
            let finder = memmem::Finder::new(needle);
            hits.retain(|&v| finder.find(self.ayah_bytes(v)).is_some());
        }
        for term in &query.excluded {
            let finder = memmem::Finder::new(&term.needle()).into_owned();
            hits.retain(|&v| finder.find(self.ayah_bytes(v)).is_none());
        }
        hits
    }

    fn ayah_bytes(&self, v: u16) -> &[u8] {
        &self.corpus[self.starts[v as usize] as usize..self.starts[v as usize + 1] as usize]
    }

    fn scan(&self, needle: &[u8], ayahs: Range<u16>) -> Vec<u16> {
        let finder = memmem::Finder::new(needle);
        let end = self.starts[ayahs.end as usize] as usize;
        let mut cursor = self.starts[ayahs.start as usize] as usize;
        let mut out = Vec::new();
        while cursor < end {
            let Some(rel) = finder.find(&self.corpus[cursor..end]) else { break };
            let v = self.ayah_at(cursor + rel);
            out.push(v);
            cursor = self.starts[v as usize + 1] as usize;
        }
        out
    }

    /// How often each normalized word occurs (for spelling suggestions).
    fn vocabulary(&self) -> std::collections::HashMap<&[u8], u32> {
        let mut vocab: std::collections::HashMap<&[u8], u32> = std::collections::HashMap::new();
        for w in self.corpus.split(|b| *b == b' ').filter(|w| !w.is_empty()) {
            *vocab.entry(w).or_default() += 1;
        }
        vocab
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Scope {
    #[default]
    All,
    Surah(SurahId),
    Juz(Juz),
}

impl Scope {
    pub fn range(self) -> Range<u16> {
        match self {
            Scope::All => 0..AYAH_COUNT,
            Scope::Surah(s) => s.range(),
            Scope::Juz(j) => j.range(),
        }
    }

    pub fn label(self) -> String {
        match self {
            Scope::All => "Whole Quran".into(),
            Scope::Surah(s) => s.name().into(),
            Scope::Juz(j) => format!("Juz {}", j.0),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Term {
    /// Normalized words.
    pub words: Vec<String>,
    /// The last word must match a whole word (otherwise it is a prefix).
    pub whole_last: bool,
    pub phrase: bool,
    /// The first word may start inside a word: Arabic joins its particles (و، ف، ب، ل، ك) and the
    /// article to the word after them ("بالصبر" holds "الصبر").
    pub infix: bool,
}

impl Term {
    fn needle(&self) -> Vec<u8> {
        let mut n = Vec::with_capacity(self.words.iter().map(|w| w.len() + 1).sum::<usize>() + 1);
        for (i, w) in self.words.iter().enumerate() {
            if i > 0 || !self.infix {
                n.push(b' ');
            }
            n.extend_from_slice(w.as_bytes());
        }
        if self.whole_last {
            n.push(b' ');
        }
        n
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Query {
    pub terms: Vec<Term>,
    pub excluded: Vec<Term>,
}

impl Query {
    /// Parses user input: bare words (all must occur, as word prefixes unless `whole_words`),
    /// `"quoted phrases"` (words in order; the last may be a prefix unless `whole_words`), and
    /// `-excluded`.
    pub fn parse(input: &str, whole_words: bool) -> Self {
        let mut q = Query::default();
        let mut rest = input.trim_start();
        while !rest.is_empty() {
            let (negated, body) = match rest.strip_prefix('-') {
                Some(b) if !b.starts_with(char::is_whitespace) && !b.is_empty() => (true, b),
                _ => (false, rest),
            };
            let (term, next) = if let Some(b) = body.strip_prefix(['"', '“', '”', '«', '»']) {
                match b.find(['"', '“', '”', '«', '»']) {
                    Some(end) => {
                        let close_len = b[end..].chars().next().map_or(1, char::len_utf8);
                        (make_term(&b[..end], true, whole_words), &b[end + close_len..])
                    }
                    None => (make_term(b, true, whole_words), ""),
                }
            } else {
                let end = body.find(char::is_whitespace).unwrap_or(body.len());
                (make_term(&body[..end], false, whole_words || negated), &body[end..])
            };
            if let Some(term) = term {
                let term =
                    if term.phrase || term.words.len() == 1 { term } else { Term { phrase: true, ..term } };
                if negated { q.excluded.push(term) } else { q.terms.push(term) }
            }
            rest = next.trim_start();
        }
        q
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// Whether the query is written in Arabic script (then it searches the Arabic text).
    pub fn is_arabic(input: &str) -> bool {
        input.chars().any(is_arabic)
    }

    /// Byte ranges in `text` (an ayah's text) that match any term, for highlighting.
    pub fn match_ranges(&self, text: &str) -> Vec<Range<usize>> {
        let words = words_with_ranges(text);
        let mut out: Vec<Range<usize>> = Vec::new();
        for term in &self.terms {
            let n = term.words.len();
            if n == 0 || n > words.len() {
                continue;
            }
            for i in 0..=words.len() - n {
                let ok = term.words.iter().enumerate().all(|(k, w)| {
                    let (ref norm, _) = words[i + k];
                    let last = k + 1 == n;
                    match (k == 0 && term.infix, last && !term.whole_last) {
                        (true, true) => norm.contains(w.as_str()),
                        (true, false) => norm.ends_with(w.as_str()),
                        (false, true) => norm.starts_with(w.as_str()),
                        (false, false) => norm == w,
                    }
                });
                if ok {
                    out.push(words[i].1.start..words[i + n - 1].1.end);
                }
            }
        }
        out.sort_by_key(|r| r.start);
        let mut merged: Vec<Range<usize>> = Vec::with_capacity(out.len());
        for r in out {
            match merged.last_mut() {
                Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
                _ => merged.push(r),
            }
        }
        merged
    }
}

fn make_term(raw: &str, phrase: bool, whole_last: bool) -> Option<Term> {
    let mut buf = Vec::new();
    normalize_into(raw, &mut buf);
    let s = String::from_utf8(buf).ok()?;
    let words: Vec<String> = s.split(' ').filter(|w| !w.is_empty()).map(str::to_string).collect();
    let infix = !whole_last && raw.chars().any(is_arabic);
    (!words.is_empty()).then_some(Term { words, whole_last, phrase, infix })
}

/// Splits text into (normalized word, byte range) pairs using the same rules as the corpus.
pub fn words_with_ranges(text: &str) -> Vec<(String, Range<usize>)> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    let mut end = 0;
    let mut cur = String::new();
    let mut iter = text.char_indices().peekable();
    while let Some((i, c)) = iter.next() {
        if is_ignorable(c) {
            if start.is_some() {
                end = i + c.len_utf8();
            }
            continue;
        }
        if c.is_alphanumeric() {
            if start.is_none() {
                start = Some(i);
            }
            fold_char(c, &mut cur);
            end = i + c.len_utf8();
        } else if is_apostrophe(c) && start.is_some() && iter.peek().is_some_and(|(_, n)| n.is_alphanumeric())
        {
            // joined: "Allah’s" → "allahs"
        } else if let Some(s) = start.take() {
            out.push((std::mem::take(&mut cur), s..end));
        }
    }
    if let Some(s) = start {
        out.push((cur, s..end));
    }
    out
}

fn is_apostrophe(c: char) -> bool {
    matches!(c, '\'' | '’' | 'ʼ' | '‘' | 'ʿ' | 'ʾ')
}

/// Characters that are dropped without separating words: Arabic harakat and Quranic marks,
/// tatweel, Latin combining accents.
fn is_ignorable(c: char) -> bool {
    matches!(c,
        '\u{0610}'..='\u{061A}'
        | '\u{064B}'..='\u{065F}'
        | '\u{0670}'
        | '\u{06D6}'..='\u{06ED}'
        | '\u{0640}'
        | '\u{08D3}'..='\u{08FF}'
        | '\u{0300}'..='\u{036F}'
        | '\u{200C}'..='\u{200F}')
}

fn push_space(out: &mut Vec<u8>) {
    if out.last() != Some(&b' ') {
        out.push(b' ');
    }
}

/// Appends the normalized form of `text` (see module docs). Never emits two spaces in a row.
pub fn normalize_into(text: &str, out: &mut Vec<u8>) {
    let mut tmp = String::new();
    let mut chars = text.chars().peekable();
    let mut prev_alnum = false;
    while let Some(c) = chars.next() {
        if is_ignorable(c) {
            continue;
        }
        if c.is_alphanumeric() {
            tmp.clear();
            fold_char(c, &mut tmp);
            out.extend_from_slice(tmp.as_bytes());
            prev_alnum = true;
        } else if is_apostrophe(c) && prev_alnum && chars.peek().is_some_and(|n| n.is_alphanumeric()) {
            // skip: joins the two halves
        } else {
            prev_alnum = false;
            push_space(out);
        }
    }
}

pub fn normalize(text: &str) -> String {
    let mut v = Vec::new();
    normalize_into(text, &mut v);
    String::from_utf8(v).unwrap_or_default().trim().to_string()
}

fn fold_char(c: char, out: &mut String) {
    if c.is_ascii() {
        out.push(c.to_ascii_lowercase());
        return;
    }
    let folded = match c {
        // Arabic: one alef, yaa for alef maqsura, haa for taa marbuta, bare waw and yaa for
        // their hamza seats; Persian/Urdu forms of kaf and yaa.
        'آ' | 'أ' | 'إ' | 'ٱ' | 'ٲ' | 'ٳ' => "ا",
        'ى' | 'ئ' | 'ی' | 'ې' => "ي",
        'ة' => "ه",
        'ؤ' => "و",
        'ک' => "ك",
        'ہ' | 'ە' | 'ۀ' => "ه",
        '\u{0660}'..='\u{0669}' => {
            out.push(char::from(b'0' + (c as u32 - 0x0660) as u8));
            return;
        }
        '\u{06F0}'..='\u{06F9}' => {
            out.push(char::from(b'0' + (c as u32 - 0x06F0) as u8));
            return;
        }
        'À'..='Å' | 'à'..='å' | 'Ā' | 'ā' | 'Ă' | 'ă' | 'Ą' | 'ą' => "a",
        'Æ' | 'æ' => "ae",
        'Ç' | 'ç' | 'Ć' | 'ć' | 'Č' | 'č' => "c",
        'È'..='Ë' | 'è'..='ë' | 'Ē' | 'ē' | 'Ė' | 'ė' | 'Ę' | 'ę' | 'Ě' | 'ě' => "e",
        'Ì'..='Ï' | 'ì'..='ï' | 'Ī' | 'ī' | 'Į' | 'į' => "i",
        'Ñ' | 'ñ' | 'Ń' | 'ń' | 'Ň' | 'ň' => "n",
        'Ò'..='Ö' | 'Ø' | 'ò'..='ö' | 'ø' | 'Ō' | 'ō' | 'Ő' | 'ő' => "o",
        'Œ' | 'œ' => "oe",
        'Ù'..='Ü' | 'ù'..='ü' | 'Ū' | 'ū' | 'Ů' | 'ů' | 'Ű' | 'ű' => "u",
        'Ý' | 'ý' | 'ÿ' | 'Ÿ' => "y",
        'ß' => "ss",
        'Š' | 'š' | 'Ś' | 'ś' | 'Ṣ' | 'ṣ' => "s",
        'Ž' | 'ž' | 'Ź' | 'ź' | 'Ż' | 'ż' | 'Ẓ' | 'ẓ' => "z",
        'Ł' | 'ł' => "l",
        'Ḥ' | 'ḥ' => "h",
        'Ḍ' | 'ḍ' => "d",
        'Ṭ' | 'ṭ' => "t",
        _ => {
            out.extend(c.to_lowercase());
            return;
        }
    };
    out.push_str(folded);
}

/// A spelling suggestion for a search that found nothing: the query with one word replaced by a
/// near word of the text itself, the one that then finds ayahs (fewest edits first, then the
/// commonest word). Returns the suggested query, how many ayahs it finds, and the first.
pub fn suggest(
    index: &SearchIndex,
    input: &str,
    scope: Scope,
    whole_words: bool,
) -> Option<(String, usize, u16)> {
    let words: Vec<&str> = input.split_whitespace().collect();
    if words.is_empty() || input.contains(['"', '“']) || words.iter().any(|w| w.starts_with('-')) {
        return None;
    }
    let vocab = index.vocabulary();
    let mut best: Option<(usize, u32, String, usize, u16)> = None;
    for (i, w) in words.iter().enumerate() {
        let norm: Vec<char> = normalize(w).chars().filter(|c| *c != ' ').collect();
        let max = match norm.len() {
            0..=3 => 1,
            4..=6 => 2,
            _ => 3,
        };
        let mut near: Vec<(usize, u32, &[u8])> = vocab
            .iter()
            .filter_map(|(v, n)| {
                let v_chars: Vec<char> = std::str::from_utf8(v).ok()?.chars().collect();
                if v_chars.len().abs_diff(norm.len()) > max || v_chars == norm {
                    return None;
                }
                let d = edit_distance(&norm, &v_chars, max)?;
                Some((d * 1000 / norm.len().max(1), *n, *v))
            })
            .collect();
        near.sort_by_key(|(d, n, _)| (*d, std::cmp::Reverse(*n)));
        for (d, n, v) in near.into_iter().take(24) {
            let replacement = String::from_utf8_lossy(v).into_owned();
            let mut q: Vec<String> = words.iter().map(|w| w.to_string()).collect();
            q[i] = replacement;
            let q = q.join(" ");
            let hits = index.search(&Query::parse(&q, whole_words), scope);
            if let Some(first) = hits.first()
                && best
                    .as_ref()
                    .is_none_or(|(bd, bn, ..)| (d, std::cmp::Reverse(n)) < (*bd, std::cmp::Reverse(*bn)))
            {
                best = Some((d, n, q, hits.len(), *first));
            }
        }
    }
    best.map(|(_, _, q, n, first)| (q, n, first))
}

/// Levenshtein distance, or `None` when it exceeds `max`.
fn edit_distance(a: &[char], b: &[char], max: usize) -> Option<usize> {
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

/// The Arabic text's index (simple script, the Basmala headings left out), built once.
#[cfg(feature = "bundled")]
pub fn arabic_index() -> &'static SearchIndex {
    static I: std::sync::OnceLock<SearchIndex> = std::sync::OnceLock::new();
    I.get_or_init(|| {
        let q = crate::text::quran();
        SearchIndex::build_from((0..AYAH_COUNT).map(|i| q.simple(AyahRef::from_index(i).unwrap())))
    })
}

pub struct SearchOutcome {
    pub ayahs: Vec<u16>,
    pub elapsed: Duration,
}

pub fn run(index: &SearchIndex, input: &str, scope: Scope, whole_words: bool) -> SearchOutcome {
    let start = Instant::now();
    let q = Query::parse(input, whole_words);
    let ayahs = index.search(&q, scope);
    SearchOutcome { ayahs, elapsed: start.elapsed() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index(texts: &[(u16, &str)]) -> SearchIndex {
        let mut all = vec![""; AYAH_COUNT as usize];
        for (i, t) in texts {
            all[*i as usize] = t;
        }
        SearchIndex::build_from(all.into_iter())
    }

    #[test]
    fn terms_phrases_exclusions_scopes() {
        let i = index(&[
            (0, "In the name of Allah, the Entirely Merciful, the Especially Merciful."),
            (1, "[All] praise is [due] to Allah, Lord of the worlds -"),
            (261, "Allah - there is no deity except Him, the Ever-Living, the Sustainer of [all] existence."),
            (6235, "From among the jinn and mankind."),
        ]);
        let hits = |q: &str, s: Scope| run(&i, q, s, false).ayahs;
        assert_eq!(hits("merci", Scope::All), [0]);
        assert_eq!(hits("allah", Scope::All), [0, 1, 261]);
        assert_eq!(hits("allah", Scope::Surah(SurahId(2))), [261]);
        assert_eq!(hits("allah -lord", Scope::All), [0, 261]);
        assert_eq!(hits("allah", Scope::Juz(Juz(3))), [261]);
        assert_eq!(hits("\"ever living\"", Scope::All), [261]);
        assert_eq!(hits("\"lord of the\"", Scope::All), [1]);
        assert!(run(&i, "merci", Scope::All, true).ayahs.is_empty());
        assert_eq!(hits("jinn", Scope::Juz(Juz(30))), [6235]);
    }

    #[test]
    fn arabic_is_folded() {
        let i = index(&[(261, "ٱللَّهُ لَآ إِلَٰهَ إِلَّا هُوَ ٱلْحَىُّ ٱلْقَيُّومُ"), (7, "الم")]);
        assert_eq!(run(&i, "الله", Scope::All, false).ayahs, [261]);
        assert_eq!(run(&i, "القيوم", Scope::All, false).ayahs, [261]);
        assert_eq!(run(&i, "ٱلْقَيُّومُ", Scope::All, true).ayahs, [261]);
        assert_eq!(run(&i, "اله", Scope::All, true).ayahs, [261]);
        // Particles joined to the word: "الحي" inside "والحي", but not as a whole word.
        let j = index(&[(44, "وَٱسْتَعِينُوا۟ بِٱلصَّبْرِ وَٱلصَّلَوٰةِ")]);
        assert_eq!(run(&j, "الصبر", Scope::All, false).ayahs, [44]);
        assert!(run(&j, "الصبر", Scope::All, true).ayahs.is_empty());
        let r = Query::parse("الصبر", false).match_ranges("بِٱلصَّبْرِ");
        assert_eq!(r.len(), 1);
        assert!(Query::is_arabic("ٱللَّهُ") && !Query::is_arabic("allah"));
        let q = Query::parse("القيوم", false);
        let text = "ٱلْحَىُّ ٱلْقَيُّومُ";
        let r = q.match_ranges(text);
        assert_eq!(&text[r[0].clone()], "ٱلْقَيُّومُ");
    }

    #[test]
    fn suggestions() {
        let i = index(&[(0, "the Entirely Merciful"), (1, "merciful and mighty")]);
        let (q, n, first) = suggest(&i, "mercyful", Scope::All, false).unwrap();
        assert_eq!((q.as_str(), n, first), ("merciful", 2, 0));
    }
}
