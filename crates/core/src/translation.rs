//! Translations: one text per ayah plus a little metadata.
//!
//! On disk (bundled or in the data folder) a translation is a zstd-compressed UTF-8 file: a JSON
//! header line ([`TranslationMeta`]), then 6,236 lines, one per ayah in order. The header comes
//! first so listing translations decodes only a few hundred bytes of each.

use std::io::{BufRead, BufReader, Read};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::meta::AyahRef;
use crate::text::Lines;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct TranslationMeta {
    /// Tanzil's identifier ("en.sahih"); also the file name.
    pub id: String,
    /// A short name for toolbars and columns: "Sahih".
    pub abbrev: String,
    pub name: String,
    pub translator: String,
    pub language: String,
    /// "ltr" or "rtl".
    pub direction: String,
    /// Where it came from: "Tanzil.net", a file name.
    pub source: String,
    /// The provider's notice and terms, shown in Settings.
    pub notice: String,
    /// Bundled with the app (rather than downloaded or imported).
    #[serde(skip)]
    pub bundled: bool,
}

impl TranslationMeta {
    pub fn rtl(&self) -> bool {
        self.direction == "rtl"
    }

    /// Transliteration is shown under the Arabic in its own style.
    pub fn is_transliteration(&self) -> bool {
        self.id.ends_with(".transliteration") || self.id.contains("translit")
    }

    /// "Sahih — Saheeh International".
    pub fn label(&self) -> String {
        if self.abbrev == self.name {
            self.name.clone()
        } else {
            format!("{} — {}", self.abbrev, self.name)
        }
    }
}

pub struct Translation {
    pub meta: TranslationMeta,
    lines: Lines,
}

impl Translation {
    pub fn new(meta: TranslationMeta, lines: Lines) -> Self {
        Self { meta, lines }
    }

    pub fn get(&self, r: AyahRef) -> &str {
        self.lines.get(r)
    }

    pub fn at(&self, index: u16) -> &str {
        self.lines.at(index)
    }

    pub fn lines(&self) -> &Lines {
        &self.lines
    }

    /// The file format: zstd(header JSON + "\n" + lines).
    pub fn encode(&self) -> Result<Vec<u8>> {
        let header = serde_json::to_string(&self.meta)?;
        let body = format!("{header}\n{}", self.lines.as_str());
        Ok(zstd::encode_all(body.as_bytes(), 9)?)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let raw = String::from_utf8(zstd::decode_all(bytes)?)?;
        let (header, body) = raw.split_once('\n').context("no header line")?;
        let meta: TranslationMeta = serde_json::from_str(header).context("bad header")?;
        let lines = Lines::parse(body.to_string())?;
        Ok(Self { meta, lines })
    }

    /// Reads just the header of an encoded translation.
    pub fn read_meta(bytes: impl Read) -> Result<TranslationMeta> {
        let mut r = BufReader::new(zstd::stream::read::Decoder::new(bytes)?);
        let mut line = String::new();
        r.read_line(&mut line)?;
        Ok(serde_json::from_str(line.trim_end())?)
    }

    /// A translation from text a user has: Tanzil's `sura|aya|text` lines (`#` comments are its
    /// notice), or exactly 6,236 plain lines.
    pub fn from_text(mut meta: TranslationMeta, text: &str) -> Result<Self> {
        let mut by_ref: Vec<Option<String>> = vec![None; crate::meta::AYAH_COUNT as usize];
        let mut plain = Vec::new();
        let mut notice = Vec::new();
        let mut piped = 0;
        for line in text.lines() {
            let line = line.trim_end_matches('\r');
            if let Some(c) = line.strip_prefix('#') {
                let c = c.trim();
                if !c.is_empty() && !c.starts_with("---") {
                    notice.push(c.to_string());
                }
                continue;
            }
            if line.trim().is_empty() {
                continue;
            }
            let mut parts = line.splitn(3, '|');
            if let (Some(s), Some(a), Some(t)) = (parts.next(), parts.next(), parts.next())
                && let (Ok(s), Ok(a)) = (s.trim().parse::<u32>(), a.trim().parse::<u32>())
            {
                let r = AyahRef::checked(s, a).with_context(|| format!("no ayah {s}:{a}"))?;
                by_ref[r.index() as usize] = Some(t.trim().to_string());
                piped += 1;
                continue;
            }
            plain.push(line.trim().to_string());
        }
        let lines: Vec<String> = if piped > 0 {
            let missing = by_ref.iter().filter(|l| l.is_none()).count();
            if missing > 0 {
                bail!("{missing} ayahs are missing from the file");
            }
            by_ref.into_iter().map(Option::unwrap_or_default).collect()
        } else if plain.len() == crate::meta::AYAH_COUNT as usize {
            plain
        } else {
            bail!(
                "expected Tanzil's \"sura|aya|text\" lines or 6,236 lines of text, found {} lines",
                plain.len()
            );
        };
        if meta.notice.is_empty() {
            meta.notice = notice.join("\n");
        }
        Ok(Self { meta, lines: Lines::from_vec(&lines)? })
    }
}

/// Short names for the translation ids people mention ("en.sahih" → "Sahih").
pub fn default_abbrev(id: &str, english_name: &str) -> String {
    let last = english_name.split_whitespace().last().unwrap_or(id);
    match id {
        "en.sahih" => "Sahih".into(),
        "en.yusufali" => "Yusuf Ali".into(),
        "en.hilali" => "Hilali-Khan".into(),
        "en.transliteration" => "Translit.".into(),
        _ if last.len() <= 14 => last.to_string(),
        _ => id.split('.').nth(1).unwrap_or(id).to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> String {
        let mut s = String::from("# Name: Test\n");
        for i in 0..crate::meta::AYAH_COUNT {
            let r = AyahRef::from_index(i).unwrap();
            s.push_str(&format!("{}|{}|text of {r}\n", r.surah, r.ayah));
        }
        s
    }

    #[test]
    fn tanzil_text_round_trips() {
        let meta = TranslationMeta { id: "xx.test".into(), abbrev: "T".into(), ..Default::default() };
        let t = Translation::from_text(meta, &sample()).unwrap();
        assert_eq!(t.get(AyahRef::new(2, 255)), "text of 2:255");
        assert_eq!(t.meta.notice, "Name: Test");
        let bytes = t.encode().unwrap();
        assert_eq!(Translation::read_meta(&bytes[..]).unwrap().id, "xx.test");
        let back = Translation::decode(&bytes).unwrap();
        assert_eq!(back.get(AyahRef::new(114, 6)), "text of 114:6");
        assert!(Translation::from_text(TranslationMeta::default(), "1|1|only one").is_err());
    }
}
