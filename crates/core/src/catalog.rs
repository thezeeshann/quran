//! More translations: the catalogue of the Tanzil Project's translations (listed and served by
//! the alquran.cloud API), downloaded once and kept in the data folder.

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::meta::{AYAH_COUNT, AyahRef};
use crate::text::Lines;
use crate::translation::{Translation, TranslationMeta, default_abbrev};

pub const LIST_URL: &str = "https://api.alquran.cloud/v1/edition?format=text&type=translation";
pub const BASE: &str = "https://api.alquran.cloud/v1/quran/";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CatalogEntry {
    pub identifier: String,
    pub language: String,
    /// The name in its own language.
    pub name: String,
    #[serde(rename = "englishName")]
    pub english_name: String,
    #[serde(default)]
    pub direction: Option<String>,
}

impl CatalogEntry {
    pub fn rtl(&self) -> bool {
        self.direction.as_deref() == Some("rtl")
    }

    /// The language's English name, for grouping ("ur" → "Urdu").
    pub fn language_name(&self) -> &'static str {
        language_name(&self.language)
    }
}

pub fn language_name(code: &str) -> &'static str {
    match code {
        "en" => "English",
        "ur" => "Urdu",
        "ar" => "Arabic",
        "fa" => "Persian",
        "tr" => "Turkish",
        "id" => "Indonesian",
        "ms" => "Malay",
        "bn" => "Bengali",
        "hi" => "Hindi",
        "fr" => "French",
        "de" => "German",
        "es" => "Spanish",
        "it" => "Italian",
        "nl" => "Dutch",
        "pt" => "Portuguese",
        "ru" => "Russian",
        "sq" => "Albanian",
        "az" => "Azerbaijani",
        "bs" => "Bosnian",
        "bg" => "Bulgarian",
        "zh" => "Chinese",
        "cs" => "Czech",
        "dv" => "Divehi",
        "ha" => "Hausa",
        "ja" => "Japanese",
        "ko" => "Korean",
        "ku" => "Kurdish",
        "ml" => "Malayalam",
        "no" => "Norwegian",
        "pl" => "Polish",
        "ro" => "Romanian",
        "sd" => "Sindhi",
        "so" => "Somali",
        "sv" => "Swedish",
        "sw" => "Swahili",
        "ta" => "Tamil",
        "tg" => "Tajik",
        "th" => "Thai",
        "tt" => "Tatar",
        "ug" => "Uyghur",
        "uz" => "Uzbek",
        "si" => "Sinhala",
        "my" => "Burmese",
        "am" => "Amharic",
        "ber" => "Amazigh",
        "ps" => "Pashto",
        "ce" => "Chechen",
        "ba" => "Bashkir",
        _ => "Other",
    }
}

#[derive(Deserialize)]
struct ListResponse {
    data: Vec<CatalogEntry>,
}

#[derive(Deserialize)]
struct QuranResponse {
    data: QuranData,
}

#[derive(Deserialize)]
struct QuranData {
    surahs: Vec<SurahData>,
    edition: CatalogEntry,
}

#[derive(Deserialize)]
struct SurahData {
    number: u8,
    ayahs: Vec<AyahData>,
}

#[derive(Deserialize)]
struct AyahData {
    #[serde(rename = "numberInSurah")]
    number: u16,
    text: String,
}

/// Parses the catalogue listing.
pub fn parse_list(json: &str) -> Result<Vec<CatalogEntry>> {
    let mut list = serde_json::from_str::<ListResponse>(json).context("unexpected catalogue")?.data;
    list.sort_by(|a, b| (a.language_name(), &a.english_name).cmp(&(b.language_name(), &b.english_name)));
    Ok(list)
}

/// Parses a whole translation as alquran.cloud serves it.
pub fn parse_quran(json: &str) -> Result<Translation> {
    let d = serde_json::from_str::<QuranResponse>(json).context("unexpected translation data")?.data;
    let mut lines = vec![String::new(); AYAH_COUNT as usize];
    let mut n = 0;
    for s in &d.surahs {
        for a in &s.ayahs {
            let r = AyahRef::checked(s.number as u32, a.number as u32)
                .with_context(|| format!("no ayah {}:{}", s.number, a.number))?;
            lines[r.index() as usize] = a.text.replace(['\n', '\r'], " ").trim().to_string();
            n += 1;
        }
    }
    if n != AYAH_COUNT as usize {
        bail!("{n} ayahs, expected {AYAH_COUNT}");
    }
    let e = d.edition;
    let meta = TranslationMeta {
        abbrev: default_abbrev(&e.identifier, &e.english_name),
        name: e.english_name.clone(),
        translator: e.english_name.clone(),
        language: e.language_name().to_string(),
        direction: if e.rtl()
            || matches!(e.language.as_str(), "ar" | "ur" | "fa" | "ps" | "sd" | "ug" | "dv" | "ku")
        {
            "rtl".into()
        } else {
            "ltr".into()
        },
        source: "Tanzil.net via alquran.cloud".into(),
        notice: format!(
            "{} ({}). From the Tanzil Project's translation collection, served by alquran.cloud. \
             Provided for non-commercial use; the translation's own rights belong to its translator or publisher.",
            e.english_name, e.name
        ),
        id: e.identifier,
        bundled: false,
    };
    Ok(Translation::new(meta, Lines::from_vec(&lines)?))
}

#[cfg(feature = "online")]
fn get(url: &str) -> Result<String> {
    let mut resp =
        ureq::get(url).header("User-Agent", "quran-rs").call().with_context(|| format!("GET {url}"))?;
    Ok(resp.body_mut().with_config().limit(32 << 20).read_to_string()?)
}

/// Lists the catalogue (a network request; call it off the UI thread).
#[cfg(feature = "online")]
pub fn fetch_list() -> Result<Vec<CatalogEntry>> {
    parse_list(&get(LIST_URL)?)
}

/// Downloads a whole translation (about 1–2 MB; call it off the UI thread).
#[cfg(feature = "online")]
pub fn download(id: &str) -> Result<Translation> {
    if !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_') {
        bail!("invalid translation id {id:?}");
    }
    parse_quran(&get(&format!("{BASE}{id}"))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_listing() {
        let json = r#"{"code":200,"data":[{"identifier":"ur.maududi","language":"ur","name":"مودودی","englishName":"Abul A'ala Maududi","format":"text","type":"translation","direction":"rtl"},{"identifier":"en.asad","language":"en","name":"Asad","englishName":"Muhammad Asad","format":"text","type":"translation","direction":"ltr"}]}"#;
        let l = parse_list(json).unwrap();
        assert_eq!(l[0].identifier, "en.asad");
        assert!(l[1].rtl());
        assert_eq!(l[1].language_name(), "Urdu");
    }

    #[test]
    fn parses_a_translation() {
        let mut surahs = Vec::new();
        for s in crate::meta::SurahId::all() {
            let ayahs: Vec<String> = (1..=s.ayahs())
                .map(|a| format!(r#"{{"number":0,"text":"t {}:{a}","numberInSurah":{a}}}"#, s.0))
                .collect();
            surahs.push(format!(r#"{{"number":{},"ayahs":[{}]}}"#, s.0, ayahs.join(",")));
        }
        let json = format!(
            r#"{{"code":200,"data":{{"surahs":[{}],"edition":{{"identifier":"en.asad","language":"en","name":"Asad","englishName":"Muhammad Asad","format":"text","type":"translation"}}}}}}"#,
            surahs.join(",")
        );
        let t = parse_quran(&json).unwrap();
        assert_eq!(t.get(AyahRef::new(2, 255)), "t 2:255");
        assert_eq!(t.meta.abbrev, "Asad");
        assert!(!t.meta.rtl());
    }
}
