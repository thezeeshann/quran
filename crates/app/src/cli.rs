//! Command-line interface (headless): read, search, import and export without opening a window.
//!
//! ```text
//! quran-rs read "2:255" [--translation en.pickthall] [--arabic-only]
//! quran-rs search "mercy" [--translation en.sahih] [--surah 2 | --juz 30] [--limit 20]
//! quran-rs list
//! quran-rs catalog
//! quran-rs download <id>
//! quran-rs import <file.txt> [--id xx.name] [--abbrev Name] [--name "…"] [--language …] [--rtl]
//! quran-rs remove <id>
//! quran-rs export --markdown|--json [-o file]
//! quran-rs import-notes <export.json>
//! ```
//!
//! Ported from bible-rs (MIT).

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use quran_core::library::{DEFAULT_ID, Library};
use quran_core::meta::{AyahRef, Juz, SurahId};
use quran_core::reference::{self, Target};
use quran_core::search::{self, Query, Scope};
use quran_core::study::StudyStore;
use quran_core::translation::{Translation, TranslationMeta};

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn has(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

fn positional(args: &[String]) -> Vec<&String> {
    let mut out = Vec::new();
    let mut skip = false;
    for a in args {
        if skip {
            skip = false;
            continue;
        }
        if a.starts_with("--") || a == "-o" {
            skip = !matches!(a.as_str(), "--json" | "--markdown" | "--rtl" | "--arabic-only");
            continue;
        }
        out.push(a);
    }
    out
}

/// Runs a CLI command if `args` names one; returns `None` to start the GUI.
pub fn run(args: &[String], data_dir: &Path) -> Option<Result<()>> {
    let cmd = args.first()?.as_str();
    let rest = &args[1..];
    Some(match cmd {
        "read" => read(rest, data_dir),
        "search" => search_cmd(rest, data_dir),
        "list" => list(data_dir),
        "catalog" => catalog(),
        "download" => download(rest, data_dir),
        "import" => import(rest, data_dir),
        "remove" => remove(rest, data_dir),
        "export" => export(rest, data_dir),
        "import-notes" => import_notes(rest, data_dir),
        "--version" | "-V" => {
            println!("quran-rs {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        "--help" | "-h" | "help" => {
            println!(
                "{}",
                include_str!("cli.rs")
                    .lines()
                    .skip(3)
                    .take(10)
                    .map(|l| l.trim_start_matches("//! "))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
            Ok(())
        }
        // macOS passes `-psn_…` to apps opened from Finder; anything else unknown opens the GUI.
        _ => return None,
    })
}

fn library(data_dir: &Path) -> Library {
    Library::open(data_dir, Arc::new(|| {}))
}

fn read(args: &[String], data_dir: &Path) -> Result<()> {
    let input = positional(args).iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" ");
    let target = reference::parse(&input).with_context(|| format!("no surah or ayah matches {input:?}"))?;
    let (from, to) = match target {
        Target::Ayah { at, end } => (at, AyahRef::new(at.surah, end.unwrap_or(at.ayah))),
        Target::Surah(s) => (s.first_ayah(), s.last_ayah()),
        Target::Juz(j) => (j.start(), j.end()),
        t @ (Target::Page(_) | Target::Hizb(_)) => {
            let s = t.start();
            (s, s.surah_id().last_ayah())
        }
    };
    let lib = library(data_dir);
    let id = flag(args, "--translation").unwrap_or_else(|| DEFAULT_ID.into());
    let t = if has(args, "--arabic-only") { None } else { Some(lib.load(&id)?) };
    let q = quran_core::text::quran();
    println!("{}", reference::format_range(from, to));
    for i in from.index()..=to.index() {
        let r = AyahRef::from_index(i).expect("in range");
        if r.ayah == 1 {
            println!("\n{} · {} · {}", r.surah_id().name(), r.surah_id().arabic(), r.surah_id().english());
        }
        println!("\n{} ﴿{}﴾", q.arabic(r), quran_core::text::arabic_number(r.ayah as u32));
        if let Some(t) = &t {
            println!("{}:{} {}", r.surah, r.ayah, t.get(r));
        }
    }
    Ok(())
}

fn search_cmd(args: &[String], data_dir: &Path) -> Result<()> {
    let query = positional(args).iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" ");
    if query.trim().is_empty() {
        bail!("usage: quran-rs search <words> [--translation id] [--surah n | --juz n]");
    }
    let lib = library(data_dir);
    let id = flag(args, "--translation").unwrap_or_else(|| DEFAULT_ID.into());
    let scope = if let Some(s) = flag(args, "--surah") {
        Scope::Surah(SurahId::new(s.parse()?).context("no such surah")?)
    } else if let Some(j) = flag(args, "--juz") {
        Scope::Juz(Juz::new(j.parse()?).context("no such juz")?)
    } else {
        Scope::All
    };
    let limit: usize = flag(args, "--limit").and_then(|s| s.parse().ok()).unwrap_or(20);
    let arabic = Query::is_arabic(&query);
    let t = lib.load(&id)?;
    let out = if arabic {
        search::run(search::arabic_index(), &query, scope, false)
    } else {
        let index = lib.get(&id).context("no translation")?.index()?;
        search::run(&index, &query, scope, false)
    };
    println!("{} ayahs ({:.2} ms)", out.ayahs.len(), out.elapsed.as_secs_f64() * 1000.0);
    let q = quran_core::text::quran();
    for i in out.ayahs.iter().take(limit) {
        let r = AyahRef::from_index(*i).expect("in range");
        if arabic {
            println!("{}  {}", r.long(), q.arabic(r));
        } else {
            println!("{}  {}", r.long(), t.get(r));
        }
    }
    Ok(())
}

fn list(data_dir: &Path) -> Result<()> {
    let lib = library(data_dir);
    for e in lib.entries() {
        let m = &e.meta;
        let kind = if e.is_bundled() { "built in" } else { "added" };
        println!("{:<22} {:<12} {:<11} {} ({kind})", m.id, m.abbrev, m.language, m.name);
    }
    Ok(())
}

fn catalog() -> Result<()> {
    for c in quran_core::catalog::fetch_list()? {
        println!("{:<22} {:<11} {} — {}", c.identifier, c.language_name(), c.english_name, c.name);
    }
    Ok(())
}

fn download(args: &[String], data_dir: &Path) -> Result<()> {
    let id = positional(args).first().map(|s| s.to_string()).context("usage: quran-rs download <id>")?;
    let mut lib = library(data_dir);
    let t = quran_core::catalog::download(&id)?;
    let name = t.meta.name.clone();
    lib.install(t)?;
    println!("Added {name} ({id})");
    Ok(())
}

/// A translation from a text file: Tanzil's `sura|aya|text` lines (its `#` header gives the
/// name, translator and language) or 6,236 lines, one per ayah.
pub fn import_file(path: &Path, abbrev: Option<&str>, name: Option<&str>) -> Result<Translation> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let header = |key: &str| {
        text.lines()
            .filter_map(|l| l.strip_prefix('#'))
            .find_map(|l| l.trim().strip_prefix(&format!("{key}:")).map(|v| v.trim().to_string()))
    };
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("imported").to_string();
    let id = header("ID").unwrap_or_else(|| {
        let clean: String = stem
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '.' { c.to_ascii_lowercase() } else { '-' })
            .collect();
        if clean.contains('.') { clean } else { format!("xx.{clean}") }
    });
    let language = header("Language").unwrap_or_else(|| "Other".into());
    let rtl = matches!(
        language.as_str(),
        "Urdu" | "Arabic" | "Persian" | "Pashto" | "Sindhi" | "Uyghur" | "Divehi" | "Kurdish"
    );
    let translator = header("Translator").unwrap_or_default();
    let full = name.map(str::to_string).or_else(|| header("Name")).unwrap_or_else(|| stem.clone());
    let meta = TranslationMeta {
        abbrev: abbrev
            .map(str::to_string)
            .unwrap_or_else(|| quran_core::translation::default_abbrev(&id, &full)),
        name: full,
        translator,
        language,
        direction: if rtl { "rtl".into() } else { "ltr".into() },
        source: path.file_name().and_then(|n| n.to_str()).unwrap_or("file").to_string(),
        id,
        ..Default::default()
    };
    Translation::from_text(meta, &text)
}

fn import(args: &[String], data_dir: &Path) -> Result<()> {
    let path = positional(args).first().map(|s| s.to_string()).context("usage: quran-rs import <file>")?;
    let mut t =
        import_file(Path::new(&path), flag(args, "--abbrev").as_deref(), flag(args, "--name").as_deref())?;
    if let Some(id) = flag(args, "--id") {
        t.meta.id = id;
    }
    if let Some(l) = flag(args, "--language") {
        t.meta.language = l;
    }
    if has(args, "--rtl") {
        t.meta.direction = "rtl".into();
    }
    let mut lib = library(data_dir);
    let name = t.meta.name.clone();
    let id = lib.install(t)?;
    println!("Imported {name} as {id}");
    Ok(())
}

fn remove(args: &[String], data_dir: &Path) -> Result<()> {
    let id = positional(args).first().map(|s| s.to_string()).context("usage: quran-rs remove <id>")?;
    library(data_dir).remove(&id)?;
    println!("Removed {id}");
    Ok(())
}

fn export(args: &[String], data_dir: &Path) -> Result<()> {
    let study = StudyStore::open(&data_dir.join("study.sqlite"))?;
    let lib = library(data_dir);
    let t = lib.load(DEFAULT_ID)?;
    let text = |r: AyahRef| Some(t.get(r).to_string());
    let body = if has(args, "--json") {
        study.export_json(&t.meta.abbrev, text)?
    } else {
        study.export_markdown(&t.meta.abbrev, text)
    };
    match flag(args, "-o") {
        Some(path) => std::fs::write(&path, body).with_context(|| format!("writing {path}"))?,
        None => print!("{body}"),
    }
    Ok(())
}

fn import_notes(args: &[String], data_dir: &Path) -> Result<()> {
    let path =
        positional(args).first().map(|s| s.to_string()).context("usage: quran-rs import-notes <file>")?;
    let mut study = StudyStore::open(&data_dir.join("study.sqlite"))?;
    let (h, n, b) = study.import_json(&std::fs::read_to_string(&path)?)?;
    println!("Imported {h} highlights, {n} notes, {b} bookmarks");
    Ok(())
}
