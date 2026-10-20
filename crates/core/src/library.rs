//! The installed translations behind one interface.
//!
//! * **Bundled** — compiled into the binary (Sahih International, Pickthall, Yusuf Ali, the
//!   transliteration and Jalandhry's Urdu).
//! * **Installed** — downloaded from the Tanzil catalogue or imported from a file, stored in
//!   `<data dir>/translations/<id>.qtr`.
//!
//! Translations decode lazily (once, a few milliseconds) and can be warmed on a background thread;
//! each builds its search index the first time it is searched.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use anyhow::{Context, Result, bail};

use crate::search::SearchIndex;
use crate::translation::{Translation, TranslationMeta};

pub enum Source {
    Bundled(&'static [u8]),
    File(PathBuf),
}

pub struct Entry {
    pub meta: TranslationMeta,
    pub source: Source,
    text: OnceLock<std::result::Result<Arc<Translation>, String>>,
    index: OnceLock<Arc<SearchIndex>>,
}

impl Entry {
    fn new(meta: TranslationMeta, source: Source) -> Self {
        Self { meta, source, text: OnceLock::new(), index: OnceLock::new() }
    }

    /// Decodes (once) and returns the translation.
    pub fn load(&self) -> Result<Arc<Translation>> {
        let r = self.text.get_or_init(|| {
            let bytes: std::borrow::Cow<'_, [u8]> = match &self.source {
                Source::Bundled(b) => std::borrow::Cow::Borrowed(*b),
                Source::File(p) => match std::fs::read(p) {
                    Ok(b) => std::borrow::Cow::Owned(b),
                    Err(e) => return Err(format!("reading {}: {e}", p.display())),
                },
            };
            Translation::decode(&bytes)
                .map(|mut t| {
                    t.meta.bundled = self.meta.bundled;
                    Arc::new(t)
                })
                .map_err(|e| format!("{e:#}"))
        });
        r.clone().map_err(anyhow::Error::msg)
    }

    pub fn loaded(&self) -> Option<Arc<Translation>> {
        self.text.get().and_then(|r| r.as_ref().ok().cloned())
    }

    /// The search index, built on first use (tens of milliseconds).
    pub fn index(&self) -> Result<Arc<SearchIndex>> {
        if let Some(i) = self.index.get() {
            return Ok(i.clone());
        }
        let t = self.load()?;
        Ok(self.index.get_or_init(|| Arc::new(SearchIndex::build(t.lines()))).clone())
    }

    pub fn is_bundled(&self) -> bool {
        matches!(self.source, Source::Bundled(_))
    }
}

type Notify = Arc<dyn Fn() + Send + Sync>;

pub struct Library {
    data_dir: PathBuf,
    entries: Vec<Arc<Entry>>,
    notify: Notify,
}

#[cfg(feature = "bundled")]
pub static BUNDLED: [(&str, &[u8]); 5] = [
    ("en.sahih", include_bytes!("../../../data/translations/en.sahih.zst")),
    ("en.pickthall", include_bytes!("../../../data/translations/en.pickthall.zst")),
    ("en.yusufali", include_bytes!("../../../data/translations/en.yusufali.zst")),
    ("en.transliteration", include_bytes!("../../../data/translations/en.transliteration.zst")),
    ("ur.jalandhry", include_bytes!("../../../data/translations/ur.jalandhry.zst")),
];

/// The translation a first launch reads.
pub const DEFAULT_ID: &str = "en.sahih";
pub const TRANSLITERATION_ID: &str = "en.transliteration";

pub fn translations_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("translations")
}

const EXT: &str = "qtr";

impl Library {
    pub fn open(data_dir: &Path, notify: Notify) -> Self {
        let mut lib = Library { data_dir: data_dir.to_path_buf(), entries: Vec::new(), notify };
        #[cfg(feature = "bundled")]
        for (_, bytes) in &BUNDLED {
            if let Ok(mut meta) = Translation::read_meta(*bytes) {
                meta.bundled = true;
                lib.entries.push(Arc::new(Entry::new(meta, Source::Bundled(bytes))));
            }
        }
        if let Ok(dir) = std::fs::read_dir(translations_dir(data_dir)) {
            let mut paths: Vec<PathBuf> =
                dir.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == EXT)).collect();
            paths.sort();
            for p in paths {
                let Ok(f) = std::fs::File::open(&p) else { continue };
                if let Ok(meta) = Translation::read_meta(f)
                    && !lib.contains(&meta.id)
                {
                    lib.entries.push(Arc::new(Entry::new(meta, Source::File(p))));
                }
            }
        }
        lib
    }

    pub fn entries(&self) -> &[Arc<Entry>] {
        &self.entries
    }

    /// Translations proper: everything but the transliteration.
    pub fn translations(&self) -> impl Iterator<Item = &Arc<Entry>> {
        self.entries.iter().filter(|e| !e.meta.is_transliteration())
    }

    pub fn get(&self, id: &str) -> Option<&Arc<Entry>> {
        self.entries.iter().find(|e| e.meta.id == id)
    }

    pub fn contains(&self, id: &str) -> bool {
        self.get(id).is_some()
    }

    pub fn load(&self, id: &str) -> Result<Arc<Translation>> {
        self.get(id).with_context(|| format!("no translation {id}"))?.load()
    }

    /// The translation if it has been decoded already (never blocks).
    pub fn loaded(&self, id: &str) -> Option<Arc<Translation>> {
        self.get(id)?.loaded()
    }

    /// Decodes translations on a background thread, then repaints.
    pub fn warm(&self, ids: &[String]) {
        let entries: Vec<Arc<Entry>> =
            ids.iter().filter_map(|id| self.get(id)).filter(|e| e.loaded().is_none()).cloned().collect();
        if entries.is_empty() {
            return;
        }
        let notify = self.notify.clone();
        std::thread::spawn(move || {
            for e in entries {
                let _ = e.load();
            }
            notify();
        });
    }

    /// Saves a translation into the data folder and lists it (replacing one with the same id).
    pub fn install(&mut self, t: Translation) -> Result<String> {
        let id = t.meta.id.clone();
        if id.is_empty() || id.contains(['/', '\\']) || id.starts_with('.') {
            bail!("invalid translation id {id:?}");
        }
        if self.get(&id).is_some_and(|e| e.is_bundled()) {
            bail!("{id} is already built in");
        }
        let dir = translations_dir(&self.data_dir);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{id}.{EXT}"));
        let tmp = dir.join(format!(".{id}.tmp"));
        std::fs::write(&tmp, t.encode()?)?;
        std::fs::rename(&tmp, &path)?;
        self.entries.retain(|e| e.meta.id != id);
        let entry = Entry::new(t.meta.clone(), Source::File(path));
        let _ = entry.text.set(Ok(Arc::new(t)));
        self.entries.push(Arc::new(entry));
        Ok(id)
    }

    /// Deletes an installed translation (bundled ones stay).
    pub fn remove(&mut self, id: &str) -> Result<()> {
        let e = self.get(id).with_context(|| format!("no translation {id}"))?;
        match &e.source {
            Source::Bundled(_) => bail!("{id} is built in and can't be removed"),
            Source::File(p) => {
                std::fs::remove_file(p).with_context(|| format!("removing {}", p.display()))?
            }
        }
        self.entries.retain(|e| e.meta.id != id);
        Ok(())
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }
}

#[cfg(all(test, feature = "bundled"))]
mod tests {
    use super::*;
    use crate::meta::AyahRef;

    #[test]
    fn bundled_translations_load() {
        let dir = std::env::temp_dir().join(format!("quran-rs-lib-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut lib = Library::open(&dir, Arc::new(|| {}));
        assert_eq!(lib.entries().len(), 5);
        let sahih = lib.load(DEFAULT_ID).unwrap();
        assert_eq!(sahih.meta.abbrev, "Sahih");
        assert!(sahih.get(AyahRef::new(1, 1)).starts_with("In the name of Allah"));
        assert!(lib.get("ur.jalandhry").unwrap().meta.rtl());
        let translit = lib.load(TRANSLITERATION_ID).unwrap();
        assert!(
            translit.get(AyahRef::new(1, 1)).starts_with("Bismi Allāhi"),
            "{}",
            translit.get(AyahRef::new(1, 1))
        );

        // Install, reopen, remove.
        let meta = TranslationMeta { id: "xx.copy".into(), abbrev: "Copy".into(), ..sahih.meta.clone() };
        let copy = Translation::new(meta, sahih.lines().clone());
        lib.install(copy).unwrap();
        let lib2 = Library::open(&dir, Arc::new(|| {}));
        assert_eq!(lib2.load("xx.copy").unwrap().get(AyahRef::new(2, 255)), sahih.get(AyahRef::new(2, 255)));
        let mut lib2 = lib2;
        lib2.remove("xx.copy").unwrap();
        assert!(lib2.remove(DEFAULT_ID).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
