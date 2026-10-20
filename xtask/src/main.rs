//! Builds the bundled data from the Tanzil Project's text, metadata and translations.
//!
//! ```text
//! cargo run -p xtask -- build-data            # download (digests pinned below) and build
//! cargo run -p xtask -- build-data --src DIR  # build from files already downloaded
//! ```
//!
//! Writes `data/quran/{uthmani,simple}.zst` (6,236 lines each, the text verbatim),
//! `data/translations/<id>.zst` (a JSON header line, then 6,236 lines) and
//! `crates/core/src/meta_gen.rs` (surahs, juz, hizb quarters, rukus, pages, sajdas).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

/// A source file: where it comes from and what it should hash to (empty: not pinned yet).
struct Source {
    file: &'static str,
    url: &'static str,
    sha256: &'static str,
}

const QURAN: [Source; 3] = [
    Source {
        file: "uthmani.txt",
        url: "https://tanzil.net/pub/download/index.php?marks=true&sajdah=true&rub=true&alef=true&quranType=uthmani&outType=txt-2&agree=true",
        sha256: "4b91f9e6e8ac645d039e4ed85b3be492e795232a31cd22d668ac58238722e26f",
    },
    Source {
        file: "simple-clean.txt",
        url: "https://tanzil.net/pub/download/index.php?quranType=simple-clean&outType=txt-2&agree=true",
        sha256: "228df2a717671aeb9d2ff573002bd28d6b3f973f4bc7153554e3a81663d67610",
    },
    Source {
        file: "quran-data.xml",
        url: "https://tanzil.net/res/text/metadata/quran-data.xml",
        sha256: "8867c1d88191472adec9db694b3cd9f135b1a2ef580574d32cf888dcb22c5c7a",
    },
];

/// Bundled translations: Tanzil id, short name, full name, translator, language, direction.
const TRANSLATIONS: [(&str, &str, &str, &str, &str, &str); 5] = [
    ("en.sahih", "Sahih", "Saheeh International", "Saheeh International", "English", "ltr"),
    (
        "en.pickthall",
        "Pickthall",
        "The Meaning of the Glorious Koran",
        "Marmaduke Pickthall",
        "English",
        "ltr",
    ),
    (
        "en.yusufali",
        "Yusuf Ali",
        "The Holy Qur'an: Text and Translation",
        "Abdullah Yusuf Ali",
        "English",
        "ltr",
    ),
    ("en.transliteration", "Translit.", "Transliteration", "Tanzil Project", "English", "ltr"),
    ("ur.jalandhry", "Jalandhry", "Fateh Muhammad Jalandhry", "Fateh Muhammad Jalandhry", "Urdu", "rtl"),
];

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("build-data") => {
            let src = args.iter().position(|a| a == "--src").and_then(|i| args.get(i + 1)).map(PathBuf::from);
            build_data(src)
        }
        _ => {
            eprintln!("usage: cargo run -p xtask -- build-data [--src DIR]");
            std::process::exit(2);
        }
    }
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

fn fetch(dir: &Path, file: &str, url: &str, sha256: &str) -> Result<Vec<u8>> {
    let path = dir.join(file);
    let bytes = if path.exists() {
        std::fs::read(&path)?
    } else {
        eprintln!("downloading {url}");
        let mut resp = ureq::get(url).call().with_context(|| format!("GET {url}"))?;
        let bytes = resp.body_mut().with_config().limit(64 << 20).read_to_vec()?;
        std::fs::create_dir_all(dir)?;
        std::fs::write(&path, &bytes)?;
        bytes
    };
    let digest = format!("{:x}", Sha256::digest(&bytes));
    if !sha256.is_empty() && digest != sha256 {
        bail!("{file}: sha256 {digest}, expected {sha256}");
    }
    eprintln!("{file}: sha256 {digest}");
    Ok(bytes)
}

/// `sura|aya|text` lines → 6,236 texts in order; `#` lines (the notice) are returned apart.
fn parse_lines(src: &str, what: &str) -> Result<(Vec<String>, String)> {
    let mut out = Vec::with_capacity(6236);
    let mut notice = String::new();
    for line in src.lines() {
        if line.starts_with('#') {
            notice.push_str(line);
            notice.push('\n');
            continue;
        }
        if line.trim().is_empty() {
            continue;
        }
        let mut parts = line.splitn(3, '|');
        let (Some(_), Some(_), Some(text)) = (parts.next(), parts.next(), parts.next()) else {
            bail!("{what}: bad line {line:?}");
        };
        out.push(text.trim_end_matches('\r').to_string());
    }
    if out.len() != 6236 {
        bail!("{what}: {} ayahs, expected 6236", out.len());
    }
    Ok((out, notice))
}

fn write_zst(path: &Path, text: &str) -> Result<()> {
    std::fs::create_dir_all(path.parent().unwrap())?;
    let bytes = zstd::encode_all(text.as_bytes(), 19)?;
    std::fs::write(path, &bytes)?;
    eprintln!("wrote {} ({} KB → {} KB)", path.display(), text.len() / 1024, bytes.len() / 1024);
    Ok(())
}

fn build_data(src: Option<PathBuf>) -> Result<()> {
    let root = root();
    let cache = src.clone().unwrap_or_else(|| root.join("target/xtask-cache"));
    let mut files = BTreeMap::new();
    for s in &QURAN {
        let pin = if src.is_some() { "" } else { s.sha256 };
        files.insert(s.file, String::from_utf8(fetch(&cache, s.file, s.url, pin)?)?);
    }
    let data = root.join("data");
    let (uthmani, notice) = parse_lines(&files["uthmani.txt"], "uthmani")?;
    let (simple, _) = parse_lines(&files["simple-clean.txt"], "simple-clean")?;
    write_zst(&data.join("quran/uthmani.zst"), &uthmani.join("\n"))?;
    write_zst(&data.join("quran/simple.zst"), &simple.join("\n"))?;
    std::fs::write(data.join("quran/NOTICE.txt"), notice)?;

    for (id, abbrev, name, translator, language, direction) in TRANSLATIONS {
        let url = format!("https://tanzil.net/trans/?transID={id}&type=txt-2");
        let raw = String::from_utf8(fetch(&cache, &format!("{id}.txt"), &url, "")?)?;
        let (mut lines, notice) = parse_lines(&raw, id)?;
        if id == "en.transliteration" {
            for l in &mut lines {
                *l = transliteration(l);
            }
        }
        let header = serde_json::json!({
            "id": id,
            "abbrev": abbrev,
            "name": name,
            "translator": translator,
            "language": language,
            "direction": direction,
            "source": "Tanzil.net",
            "notice": notice.lines().map(|l| l.trim_start_matches('#').trim()).filter(|l| !l.is_empty() && !l.starts_with("---")).collect::<Vec<_>>().join("\n"),
        });
        let body = format!("{header}\n{}", lines.join("\n"));
        write_zst(&data.join(format!("translations/{id}.zst")), &body)?;
    }

    let meta = metadata(&files["quran-data.xml"])?;
    let out = root.join("crates/core/src/meta_gen.rs");
    std::fs::write(&out, meta)?;
    eprintln!("wrote {}", out.display());
    Ok(())
}

/// Tanzil's transliteration marks long vowels and emphatic consonants with `<u>…</U>` and
/// assimilated letters with `<b>…</B>`; `AA` stands for ʿayn. Set as plain Unicode instead.
pub fn transliteration(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut underline = false;
    let mut rest = s;
    while let Some(c) = rest.chars().next() {
        if c == '<'
            && let Some(end) = rest.find('>')
        {
            {
                let tag = rest[1..end].to_ascii_lowercase();
                underline = match tag.as_str() {
                    "u" => true,
                    "/u" => false,
                    _ => underline,
                };
                rest = &rest[end + 1..];
                continue;
            }
        }
        if rest.starts_with("AA") {
            out.push('ʿ');
            rest = &rest[2..];
            continue;
        }
        let mapped = if underline {
            match c {
                'a' => 'ā',
                'i' => 'ī',
                'u' => 'ū',
                'e' => 'ē',
                'o' => 'ō',
                'h' => 'ḥ',
                's' => 'ṣ',
                'd' => 'ḍ',
                't' => 'ṭ',
                'z' => 'ẓ',
                'A' => 'Ā',
                'I' => 'Ī',
                'U' => 'Ū',
                'H' => 'Ḥ',
                'S' => 'Ṣ',
                'D' => 'Ḍ',
                'T' => 'Ṭ',
                'Z' => 'Ẓ',
                c => c,
            }
        } else {
            c
        };
        out.push(mapped);
        rest = &rest[c.len_utf8()..];
    }
    out
}

fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let key = format!(" {name}=\"");
    let start = tag.find(&key)? + key.len();
    let end = tag[start..].find('"')? + start;
    Some(&tag[start..end])
}

fn tags<'a>(xml: &'a str, element: &str) -> Vec<&'a str> {
    let open = format!("<{element} ");
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find(&open) {
        let end = rest[i..].find("/>").map(|e| i + e).unwrap_or(rest.len());
        out.push(&rest[i..end]);
        rest = &rest[end..];
    }
    out
}

fn metadata(xml: &str) -> Result<String> {
    let mut o = String::from(
        "// @generated by `cargo run -p xtask -- build-data` from Tanzil's quran-data.xml\n\
         // (C) 2008-2009 Tanzil.info, CC BY 3.0. Do not edit by hand.\n\n",
    );
    let suras = tags(xml, "sura");
    if suras.len() != 114 {
        bail!("{} suras", suras.len());
    }
    let num = |t: &str, a: &str| -> Result<u32> {
        attr(t, a)
            .with_context(|| format!("{a} in {t}"))?
            .parse::<u32>()
            .with_context(|| format!("{a} in {t}"))
    };
    let list = |name: &str, ty: &str, items: Vec<String>| {
        format!(
            "pub(crate) const {name}: [{ty}; {}] = [\n    {},\n];\n\n",
            items.len(),
            items.join(",\n    ")
        )
    };
    let mut ayahs = Vec::new();
    let mut start = Vec::new();
    let mut arabic = Vec::new();
    let mut tname = Vec::new();
    let mut ename = Vec::new();
    let mut medinan = Vec::new();
    let mut order = Vec::new();
    let mut rukus = Vec::new();
    for s in &suras {
        ayahs.push(num(s, "ayas")?.to_string());
        start.push(num(s, "start")?.to_string());
        arabic.push(format!("{:?}", attr(s, "name").unwrap_or_default()));
        tname.push(format!("{:?}", attr(s, "tname").unwrap_or_default()));
        ename.push(format!("{:?}", attr(s, "ename").unwrap_or_default()));
        medinan.push((attr(s, "type") == Some("Medinan")).to_string());
        order.push(num(s, "order")?.to_string());
        rukus.push(num(s, "rukus")?.to_string());
    }
    o += &list("SURAH_AYAHS", "u16", ayahs);
    o += &list("SURAH_START", "u16", start);
    o += &list("SURAH_ARABIC", "&str", arabic);
    o += &list("SURAH_TANZIL_NAME", "&str", tname);
    o += &list("SURAH_ENGLISH", "&str", ename);
    o += &list("SURAH_MEDINAN", "bool", medinan);
    o += &list("SURAH_ORDER", "u8", order);
    o += &list("SURAH_RUKUS", "u8", rukus);
    for (element, name, expect) in [
        ("juz", "JUZ_START", 30),
        ("quarter", "HIZB_QUARTER_START", 240),
        ("manzil", "MANZIL_START", 7),
        ("ruku", "RUKU_START", 556),
        ("page", "PAGE_START", 604),
    ] {
        let items = tags(xml, element)
            .iter()
            .map(|t| Ok(format!("({}, {})", num(t, "sura")?, num(t, "aya")?)))
            .collect::<Result<Vec<_>>>()?;
        if items.len() != expect {
            bail!("{element}: {} entries, expected {expect}", items.len());
        }
        o += &list(name, "(u8, u16)", items);
    }
    let sajdas = tags(xml, "sajda")
        .iter()
        .map(|t| {
            Ok(format!(
                "({}, {}, {})",
                num(t, "sura")?,
                num(t, "aya")?,
                attr(t, "type") == Some("obligatory")
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    o += &list("SAJDA", "(u8, u16, bool)", sajdas);
    let _ = writeln!(o);
    Ok(o.trim_end().to_string() + "\n")
}

#[cfg(test)]
mod tests {
    #[test]
    fn transliteration_marks() {
        assert_eq!(
            super::transliteration("Bismi All<u>a</U>hi a<b>l</B>rra<u>h</U>m<u>a</U>ni"),
            "Bismi Allāhi alrraḥmāni"
        );
        assert_eq!(super::transliteration("naAAbudu"), "naʿbudu");
    }
}
