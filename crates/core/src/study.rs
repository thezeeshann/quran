//! Local study data: highlights, notes, bookmarks, reading history, plans and settings, in one
//! SQLite file.
//!
//! Everything is keyed by ayah (not by translation), so a note written while reading Sahih
//! International shows beside Pickthall too. The tables are mirrored in memory — they hold
//! hundreds of rows, not millions — so the reader can look them up per ayah every frame; writes go
//! straight through to SQLite (WAL mode, a few hundred microseconds each).
//!
//! Ported from bible-rs (MIT).

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::meta::{AyahRef, SurahId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HighlightColor {
    Yellow,
    Green,
    Blue,
    Pink,
    Purple,
    Orange,
}

impl HighlightColor {
    /// In key order: 1–6 apply these, 0 clears. The stored value is the index, so new colours
    /// only ever go at the end.
    pub const ALL: [HighlightColor; 6] = [
        HighlightColor::Yellow,
        HighlightColor::Green,
        HighlightColor::Blue,
        HighlightColor::Pink,
        HighlightColor::Purple,
        HighlightColor::Orange,
    ];

    pub fn name(self) -> &'static str {
        match self {
            HighlightColor::Yellow => "yellow",
            HighlightColor::Green => "green",
            HighlightColor::Blue => "blue",
            HighlightColor::Pink => "pink",
            HighlightColor::Purple => "purple",
            HighlightColor::Orange => "orange",
        }
    }

    fn from_i64(v: i64) -> Option<Self> {
        Self::ALL.get(usize::try_from(v).ok()?).copied()
    }

    fn to_i64(self) -> i64 {
        Self::ALL.iter().position(|c| *c == self).unwrap() as i64
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub body: String,
    pub created: i64,
    pub updated: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bookmark {
    pub label: String,
    pub created: i64,
}

pub struct StudyStore {
    conn: Connection,
    highlights: HashMap<u32, (HighlightColor, i64)>,
    notes: HashMap<u32, Note>,
    bookmarks: BTreeMap<u32, Bookmark>,
    generation: u64,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS highlights(
    surah INTEGER NOT NULL, ayah INTEGER NOT NULL,
    color INTEGER NOT NULL, created INTEGER NOT NULL,
    PRIMARY KEY(surah, ayah));
CREATE TABLE IF NOT EXISTS notes(
    surah INTEGER NOT NULL, ayah INTEGER NOT NULL,
    body TEXT NOT NULL, created INTEGER NOT NULL, updated INTEGER NOT NULL,
    PRIMARY KEY(surah, ayah));
CREATE TABLE IF NOT EXISTS bookmarks(
    surah INTEGER NOT NULL, ayah INTEGER NOT NULL,
    label TEXT NOT NULL, created INTEGER NOT NULL,
    PRIMARY KEY(surah, ayah));
CREATE TABLE IF NOT EXISTS settings(key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS reading(
    started INTEGER PRIMARY KEY, surah INTEGER NOT NULL,
    translation TEXT NOT NULL, seconds INTEGER NOT NULL, furthest INTEGER NOT NULL,
    ayahs INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS plans(
    id TEXT PRIMARY KEY, started INTEGER NOT NULL, finished INTEGER);
CREATE TABLE IF NOT EXISTS plan_reads(
    plan TEXT NOT NULL, day INTEGER NOT NULL, passage INTEGER NOT NULL, at INTEGER NOT NULL,
    PRIMARY KEY(plan, day, passage));
";

/// A stretch of reading one surah: when it started, for how long, and how far it got.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadingSession {
    /// Unix seconds; also the session's identity.
    pub started: i64,
    pub surah: SurahId,
    pub translation: String,
    /// Time actually spent reading (idle time is not counted).
    pub seconds: u32,
    /// The furthest ayah reached, and how many ayahs the surah has.
    pub furthest: u16,
    pub ayahs: u16,
}

impl ReadingSession {
    /// Reached the surah's last ayah.
    pub fn finished(&self) -> bool {
        self.ayahs > 0 && self.furthest >= self.ayahs
    }

    /// How much of the surah was read, 0–1.
    pub fn progress(&self) -> f32 {
        if self.ayahs == 0 { 0.0 } else { (self.furthest as f32 / self.ayahs as f32).min(1.0) }
    }
}

/// A reading plan under way (or finished): which, and when it began (the start of that day).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanProgress {
    pub id: String,
    pub started: i64,
    pub finished: Option<i64>,
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn ayah_from_row(surah: i64, ayah: i64) -> Option<AyahRef> {
    AyahRef::checked(u32::try_from(surah).ok()?, u32::try_from(ayah).ok()?)
}

impl StudyStore {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch(SCHEMA)?;
        let mut s = Self {
            conn,
            highlights: HashMap::new(),
            notes: HashMap::new(),
            bookmarks: BTreeMap::new(),
            generation: 1,
        };
        s.reload()?;
        Ok(s)
    }

    fn reload(&mut self) -> Result<()> {
        self.highlights.clear();
        self.notes.clear();
        self.bookmarks.clear();
        let mut st = self.conn.prepare("SELECT surah, ayah, color, created FROM highlights")?;
        let rows = st.query_map([], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get(3)?))
        })?;
        for row in rows {
            let (s, a, color, created) = row?;
            if let (Some(r), Some(color)) = (ayah_from_row(s, a), HighlightColor::from_i64(color)) {
                self.highlights.insert(r.key(), (color, created));
            }
        }
        let mut st = self.conn.prepare("SELECT surah, ayah, body, created, updated FROM notes")?;
        let rows = st.query_map([], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?, r.get(3)?, r.get(4)?))
        })?;
        for row in rows {
            let (s, a, body, created, updated) = row?;
            if let Some(r) = ayah_from_row(s, a) {
                self.notes.insert(r.key(), Note { body, created, updated });
            }
        }
        let mut st = self.conn.prepare("SELECT surah, ayah, label, created FROM bookmarks")?;
        let rows = st.query_map([], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?, r.get(3)?))
        })?;
        for row in rows {
            let (s, a, label, created) = row?;
            if let Some(r) = ayah_from_row(s, a) {
                self.bookmarks.insert(r.key(), Bookmark { label, created });
            }
        }
        self.generation += 1;
        Ok(())
    }

    /// Increments whenever study data changes (the reader uses it to invalidate layouts).
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn highlight(&self, r: AyahRef) -> Option<HighlightColor> {
        self.highlights.get(&r.key()).map(|(c, _)| *c)
    }

    pub fn note(&self, r: AyahRef) -> Option<&Note> {
        self.notes.get(&r.key())
    }

    pub fn bookmark(&self, r: AyahRef) -> Option<&Bookmark> {
        self.bookmarks.get(&r.key())
    }

    pub fn is_bookmarked(&self, r: AyahRef) -> bool {
        self.bookmarks.contains_key(&r.key())
    }

    pub fn set_highlight(&mut self, r: AyahRef, color: Option<HighlightColor>) -> Result<()> {
        match color {
            Some(c) => {
                let created = now();
                self.conn.execute(
                    "INSERT INTO highlights(surah, ayah, color, created) VALUES(?1, ?2, ?3, ?4)
                     ON CONFLICT(surah, ayah) DO UPDATE SET color = excluded.color",
                    params![r.surah, r.ayah, c.to_i64(), created],
                )?;
                let created = self.highlights.get(&r.key()).map_or(created, |(_, t)| *t);
                self.highlights.insert(r.key(), (c, created));
            }
            None => {
                self.conn.execute(
                    "DELETE FROM highlights WHERE surah = ?1 AND ayah = ?2",
                    params![r.surah, r.ayah],
                )?;
                self.highlights.remove(&r.key());
            }
        }
        self.generation += 1;
        Ok(())
    }

    /// Saves a note; an empty (whitespace-only) body deletes it.
    pub fn set_note(&mut self, r: AyahRef, body: &str) -> Result<()> {
        if body.trim().is_empty() {
            self.conn
                .execute("DELETE FROM notes WHERE surah = ?1 AND ayah = ?2", params![r.surah, r.ayah])?;
            self.notes.remove(&r.key());
        } else {
            let t = now();
            self.conn.execute(
                "INSERT INTO notes(surah, ayah, body, created, updated) VALUES(?1, ?2, ?3, ?4, ?4)
                 ON CONFLICT(surah, ayah) DO UPDATE SET body = excluded.body, updated = excluded.updated",
                params![r.surah, r.ayah, body, t],
            )?;
            let created = self.notes.get(&r.key()).map_or(t, |n| n.created);
            self.notes.insert(r.key(), Note { body: body.to_string(), created, updated: t });
        }
        self.generation += 1;
        Ok(())
    }

    pub fn set_bookmark(&mut self, r: AyahRef, label: Option<&str>) -> Result<()> {
        match label {
            Some(label) => {
                let t = now();
                self.conn.execute(
                    "INSERT INTO bookmarks(surah, ayah, label, created) VALUES(?1, ?2, ?3, ?4)
                     ON CONFLICT(surah, ayah) DO UPDATE SET label = excluded.label",
                    params![r.surah, r.ayah, label, t],
                )?;
                let created = self.bookmarks.get(&r.key()).map_or(t, |b| b.created);
                self.bookmarks.insert(r.key(), Bookmark { label: label.to_string(), created });
            }
            None => {
                self.conn.execute(
                    "DELETE FROM bookmarks WHERE surah = ?1 AND ayah = ?2",
                    params![r.surah, r.ayah],
                )?;
                self.bookmarks.remove(&r.key());
            }
        }
        self.generation += 1;
        Ok(())
    }

    pub fn toggle_bookmark(&mut self, r: AyahRef) -> Result<bool> {
        let on = !self.is_bookmarked(r);
        self.set_bookmark(r, on.then_some(""))?;
        Ok(on)
    }

    pub fn highlights(&self) -> Vec<(AyahRef, HighlightColor)> {
        let mut v: Vec<_> =
            self.highlights.iter().filter_map(|(k, (c, _))| Some((AyahRef::from_key(*k)?, *c))).collect();
        v.sort_by_key(|(r, _)| *r);
        v
    }

    /// Highlights with when they were made, the most recent first.
    pub fn highlights_dated(&self) -> Vec<(AyahRef, HighlightColor, i64)> {
        let mut v: Vec<_> =
            self.highlights.iter().filter_map(|(k, (c, t))| Some((AyahRef::from_key(*k)?, *c, *t))).collect();
        v.sort_by_key(|(r, _, t)| (std::cmp::Reverse(*t), *r));
        v
    }

    pub fn notes(&self) -> Vec<(AyahRef, &Note)> {
        let mut v: Vec<_> =
            self.notes.iter().filter_map(|(k, n)| Some((AyahRef::from_key(*k)?, n))).collect();
        v.sort_by_key(|(r, _)| *r);
        v
    }

    pub fn bookmarks(&self) -> Vec<(AyahRef, &Bookmark)> {
        let mut v: Vec<_> =
            self.bookmarks.iter().filter_map(|(k, b)| Some((AyahRef::from_key(*k)?, b))).collect();
        v.sort_by_key(|(r, _)| *r);
        v
    }

    pub fn counts(&self) -> (usize, usize, usize) {
        (self.highlights.len(), self.notes.len(), self.bookmarks.len())
    }

    /// Saves a reading session, replacing an earlier save of the same session.
    pub fn record_reading(&self, s: &ReadingSession) -> Result<()> {
        self.conn.execute(
            "INSERT INTO reading(started, surah, translation, seconds, furthest, ayahs)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(started) DO UPDATE SET seconds = excluded.seconds, furthest = excluded.furthest,
               ayahs = excluded.ayahs, translation = excluded.translation",
            params![s.started, s.surah.0, s.translation, s.seconds, s.furthest, s.ayahs],
        )?;
        Ok(())
    }

    /// Reading sessions that started at or after `since` (Unix seconds), newest first.
    pub fn reading_since(&self, since: i64) -> Vec<ReadingSession> {
        let Ok(mut st) = self.conn.prepare_cached(
            "SELECT started, surah, translation, seconds, furthest, ayahs FROM reading
             WHERE started >= ?1 ORDER BY started DESC",
        ) else {
            return Vec::new();
        };
        let rows = st.query_map([since], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, i64>(5)?,
            ))
        });
        let Ok(rows) = rows else { return Vec::new() };
        rows.filter_map(|r| r.ok())
            .filter_map(|(started, surah, translation, seconds, furthest, ayahs)| {
                Some(ReadingSession {
                    started,
                    surah: SurahId::new(u32::try_from(surah).ok()?)?,
                    translation,
                    seconds: u32::try_from(seconds).ok()?,
                    furthest: u16::try_from(furthest).ok()?,
                    ayahs: u16::try_from(ayahs).ok()?,
                })
            })
            .collect()
    }

    /// Forgets reading history from `since` on (all of it for 0).
    pub fn clear_reading(&self, since: i64) -> Result<()> {
        self.conn.execute("DELETE FROM reading WHERE started >= ?1", [since])?;
        Ok(())
    }

    /// Plans started, newest first.
    pub fn plans(&self) -> Vec<PlanProgress> {
        let Ok(mut st) =
            self.conn.prepare_cached("SELECT id, started, finished FROM plans ORDER BY started DESC")
        else {
            return Vec::new();
        };
        st.query_map([], |r| Ok(PlanProgress { id: r.get(0)?, started: r.get(1)?, finished: r.get(2)? }))
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }

    /// Starts (or restarts) a plan on the day containing `started`, forgetting earlier progress.
    pub fn start_plan(&mut self, id: &str, started: i64) -> Result<()> {
        self.conn.execute("DELETE FROM plan_reads WHERE plan = ?1", [id])?;
        self.conn.execute(
            "INSERT OR REPLACE INTO plans(id, started, finished) VALUES(?1, ?2, NULL)",
            params![id, started],
        )?;
        self.generation += 1;
        Ok(())
    }

    pub fn finish_plan(&mut self, id: &str, at: Option<i64>) -> Result<()> {
        self.conn.execute("UPDATE plans SET finished = ?2 WHERE id = ?1", params![id, at])?;
        self.generation += 1;
        Ok(())
    }

    pub fn stop_plan(&mut self, id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM plans WHERE id = ?1", [id])?;
        self.conn.execute("DELETE FROM plan_reads WHERE plan = ?1", [id])?;
        self.generation += 1;
        Ok(())
    }

    /// The passages ticked off in a plan, as (day, passage index) pairs.
    pub fn plan_reads(&self, id: &str) -> std::collections::HashSet<(u16, u8)> {
        let Ok(mut st) = self.conn.prepare_cached("SELECT day, passage FROM plan_reads WHERE plan = ?1")
        else {
            return Default::default();
        };
        st.query_map([id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))
            .map(|rows| {
                rows.filter_map(|r| r.ok())
                    .filter_map(|(d, p)| Some((u16::try_from(d).ok()?, u8::try_from(p).ok()?)))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn set_plan_read(&mut self, id: &str, day: u16, passage: u8, read: bool) -> Result<()> {
        if read {
            self.conn.execute(
                "INSERT OR REPLACE INTO plan_reads(plan, day, passage, at) VALUES(?1, ?2, ?3, ?4)",
                params![id, day, passage, now()],
            )?;
        } else {
            self.conn.execute(
                "DELETE FROM plan_reads WHERE plan = ?1 AND day = ?2 AND passage = ?3",
                params![id, day, passage],
            )?;
        }
        self.generation += 1;
        Ok(())
    }

    pub fn setting(&self, key: &str) -> Option<String> {
        self.conn
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO settings(key, value) VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// The study data as JSON (re-importable), with what `include` asks for. `text` supplies
    /// each ayah's text for context (may return `None`).
    pub fn export_json_with(
        &self,
        translation: &str,
        text: impl Fn(AyahRef) -> Option<String>,
        include: Include,
    ) -> Result<String> {
        let entry = |r: AyahRef| ExportAyah {
            reference: r.long(),
            surah: r.surah,
            ayah: r.ayah,
            arabic: arabic(r),
            text: text(r),
        };
        let mut doc = ExportDoc {
            app: APP.into(),
            format: 1,
            exported_at: rfc3339(now()),
            translation: translation.to_string(),
            history: Vec::new(),
            plans: Vec::new(),
            highlights: self
                .highlights()
                .into_iter()
                .map(|(r, c)| ExportHighlight {
                    ayah: entry(r),
                    color: c,
                    created: rfc3339(self.highlights[&r.key()].1),
                })
                .collect(),
            notes: self
                .notes()
                .into_iter()
                .map(|(r, n)| ExportNote {
                    ayah: entry(r),
                    note: n.body.clone(),
                    created: rfc3339(n.created),
                    updated: rfc3339(n.updated),
                })
                .collect(),
            bookmarks: self
                .bookmarks()
                .into_iter()
                .map(|(r, b)| ExportBookmark {
                    ayah: entry(r),
                    label: b.label.clone(),
                    created: rfc3339(b.created),
                })
                .collect(),
        };
        if !include.highlights {
            doc.highlights.clear();
        }
        if !include.notes {
            doc.notes.clear();
        }
        if !include.bookmarks {
            doc.bookmarks.clear();
        }
        if include.history {
            doc.history = self
                .reading_since(0)
                .into_iter()
                .map(|s| ExportReading {
                    started: rfc3339(s.started),
                    surah: s.surah.0,
                    translation: s.translation,
                    seconds: s.seconds,
                    furthest: s.furthest,
                    ayahs: s.ayahs,
                })
                .collect();
            doc.plans = self
                .plans()
                .into_iter()
                .map(|p| ExportPlan {
                    read: self.plan_reads(&p.id).into_iter().collect(),
                    id: p.id,
                    started: rfc3339(p.started),
                    finished: p.finished.map(rfc3339),
                })
                .collect();
        }
        Ok(serde_json::to_string_pretty(&doc)?)
    }

    pub fn export_json(&self, translation: &str, text: impl Fn(AyahRef) -> Option<String>) -> Result<String> {
        self.export_json_with(translation, text, Include::STUDY)
    }

    /// The study data as readable Markdown, with what `include` asks for.
    pub fn export_markdown_with(
        &self,
        translation: &str,
        text: impl Fn(AyahRef) -> Option<String>,
        include: Include,
    ) -> String {
        use std::fmt::Write;
        let mut md = String::new();
        let _ = writeln!(md, "# Quran study notes\n");
        let _ =
            writeln!(md, "Exported from Quran on {} · translation: {translation}\n", &rfc3339(now())[..10]);
        let bookmarks = if include.bookmarks { self.bookmarks() } else { Vec::new() };
        if !bookmarks.is_empty() {
            let _ = writeln!(md, "## Bookmarks\n");
            for (r, b) in bookmarks {
                if b.label.is_empty() {
                    let _ = writeln!(md, "- **{}**", r.long());
                } else {
                    let _ = writeln!(md, "- **{}** — {}", r.long(), b.label);
                }
            }
            md.push('\n');
        }
        let highlights = if include.highlights { self.highlights() } else { Vec::new() };
        if !highlights.is_empty() {
            let _ = writeln!(md, "## Highlights\n");
            let mut surah = None;
            for (r, c) in highlights {
                if surah != Some(r.surah) {
                    surah = Some(r.surah);
                    let _ = writeln!(md, "### {} ({})\n", r.surah_id().name(), r.surah);
                }
                let t = text(r).unwrap_or_default();
                let _ = writeln!(md, "- **{}** ({}) {t}", r, c.name());
            }
            md.push('\n');
        }
        let notes = if include.notes { self.notes() } else { Vec::new() };
        if !notes.is_empty() {
            let _ = writeln!(md, "## Notes\n");
            for (r, n) in notes {
                let _ = writeln!(md, "### {}\n", r.long());
                if let Some(a) = arabic(r) {
                    let _ = writeln!(md, "> {a}\n>");
                }
                if let Some(t) = text(r) {
                    let _ = writeln!(md, "> {t}\n");
                }
                let _ = writeln!(md, "{}\n", n.body.trim_end());
            }
        }
        let history = if include.history { self.reading_since(0) } else { Vec::new() };
        if !history.is_empty() {
            let _ = writeln!(md, "## Reading history\n");
            for s in history {
                let minutes = (s.seconds + 30) / 60;
                let done = if s.finished() {
                    "finished".to_string()
                } else {
                    format!("{:.0} %", s.progress() * 100.0)
                };
                let _ = writeln!(
                    md,
                    "- {} — {} ({}), {minutes} min, {done}",
                    &rfc3339(s.started)[..10],
                    s.surah.name(),
                    s.translation
                );
            }
            md.push('\n');
        }
        md
    }

    pub fn export_markdown(&self, translation: &str, text: impl Fn(AyahRef) -> Option<String>) -> String {
        self.export_markdown_with(translation, text, Include::STUDY)
    }

    /// Merges a JSON export back in (existing entries for the same ayah are overwritten).
    pub fn import_json(&mut self, json: &str) -> Result<(usize, usize, usize)> {
        let doc: ExportDoc = serde_json::from_str(json).context("not a quran-rs export")?;
        if doc.app != APP {
            bail!("not a quran-rs export");
        }
        let ayah = |e: &ExportAyah| -> Result<AyahRef> {
            AyahRef::checked(e.surah as u32, e.ayah as u32)
                .with_context(|| format!("no ayah {}:{}", e.surah, e.ayah))
        };
        let tx = self.conn.unchecked_transaction()?;
        for h in &doc.highlights {
            let r = ayah(&h.ayah)?;
            tx.execute(
                "INSERT INTO highlights(surah, ayah, color, created) VALUES(?1, ?2, ?3, ?4)
                 ON CONFLICT(surah, ayah) DO UPDATE SET color = excluded.color",
                params![r.surah, r.ayah, h.color.to_i64(), parse_rfc3339(&h.created).unwrap_or_else(now)],
            )?;
        }
        for n in &doc.notes {
            let r = ayah(&n.ayah)?;
            tx.execute(
                "INSERT INTO notes(surah, ayah, body, created, updated) VALUES(?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(surah, ayah) DO UPDATE SET body = excluded.body, updated = excluded.updated",
                params![
                    r.surah,
                    r.ayah,
                    n.note,
                    parse_rfc3339(&n.created).unwrap_or_else(now),
                    parse_rfc3339(&n.updated).unwrap_or_else(now)
                ],
            )?;
        }
        for b in &doc.bookmarks {
            let r = ayah(&b.ayah)?;
            tx.execute(
                "INSERT INTO bookmarks(surah, ayah, label, created) VALUES(?1, ?2, ?3, ?4)
                 ON CONFLICT(surah, ayah) DO UPDATE SET label = excluded.label",
                params![r.surah, r.ayah, b.label, parse_rfc3339(&b.created).unwrap_or_else(now)],
            )?;
        }
        for r in &doc.history {
            if SurahId::new(r.surah as u32).is_none() {
                continue;
            }
            tx.execute(
                "INSERT OR REPLACE INTO reading(started, surah, translation, seconds, furthest, ayahs)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    parse_rfc3339(&r.started).unwrap_or_else(now),
                    r.surah,
                    r.translation,
                    r.seconds,
                    r.furthest,
                    r.ayahs
                ],
            )?;
        }
        for p in &doc.plans {
            tx.execute(
                "INSERT OR REPLACE INTO plans(id, started, finished) VALUES(?1, ?2, ?3)",
                params![
                    p.id,
                    parse_rfc3339(&p.started).unwrap_or_else(now),
                    p.finished.as_deref().and_then(parse_rfc3339)
                ],
            )?;
            for (day, passage) in &p.read {
                tx.execute(
                    "INSERT OR REPLACE INTO plan_reads(plan, day, passage, at) VALUES(?1, ?2, ?3, ?4)",
                    params![p.id, day, passage, now()],
                )?;
            }
        }
        tx.commit()?;
        self.reload()?;
        Ok((doc.highlights.len(), doc.notes.len(), doc.bookmarks.len()))
    }

    /// Erases every highlight, note, bookmark, plan and the reading history.
    pub fn erase_all(&mut self) -> Result<()> {
        self.conn.execute_batch(
            "DELETE FROM highlights; DELETE FROM notes; DELETE FROM bookmarks;
             DELETE FROM plans; DELETE FROM plan_reads; DELETE FROM reading;",
        )?;
        self.reload()
    }
}

const APP: &str = "quran-rs";

#[cfg(feature = "bundled")]
fn arabic(r: AyahRef) -> Option<String> {
    Some(crate::text::quran().arabic(r).to_string())
}

#[cfg(not(feature = "bundled"))]
fn arabic(_: AyahRef) -> Option<String> {
    None
}

#[derive(Serialize, Deserialize)]
struct ExportDoc {
    app: String,
    format: u32,
    exported_at: String,
    translation: String,
    #[serde(default)]
    highlights: Vec<ExportHighlight>,
    #[serde(default)]
    notes: Vec<ExportNote>,
    #[serde(default)]
    bookmarks: Vec<ExportBookmark>,
    /// Reading sessions and plans, when the export includes history.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    history: Vec<ExportReading>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    plans: Vec<ExportPlan>,
}

#[derive(Serialize, Deserialize)]
struct ExportReading {
    started: String,
    surah: u8,
    translation: String,
    seconds: u32,
    furthest: u16,
    ayahs: u16,
}

#[derive(Serialize, Deserialize)]
struct ExportPlan {
    id: String,
    started: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    finished: Option<String>,
    /// Passages ticked, as (day, index).
    read: Vec<(u16, u8)>,
}

/// What an export carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Include {
    pub highlights: bool,
    pub notes: bool,
    pub bookmarks: bool,
    /// Reading history and plans.
    pub history: bool,
}

impl Include {
    /// Highlights, notes and bookmarks.
    pub const STUDY: Include = Include { highlights: true, notes: true, bookmarks: true, history: false };
    pub const EVERYTHING: Include = Include { highlights: true, notes: true, bookmarks: true, history: true };

    pub const fn default_none() -> Include {
        Include { highlights: false, notes: false, bookmarks: false, history: false }
    }
}

#[derive(Serialize, Deserialize)]
struct ExportAyah {
    reference: String,
    surah: u8,
    ayah: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    arabic: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    text: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct ExportHighlight {
    #[serde(flatten)]
    ayah: ExportAyah,
    color: HighlightColor,
    created: String,
}

#[derive(Serialize, Deserialize)]
struct ExportNote {
    #[serde(flatten)]
    ayah: ExportAyah,
    note: String,
    created: String,
    updated: String,
}

#[derive(Serialize, Deserialize)]
struct ExportBookmark {
    #[serde(flatten)]
    ayah: ExportAyah,
    label: String,
    created: String,
}

/// Unix seconds → `YYYY-MM-DDTHH:MM:SSZ` (proleptic Gregorian, UTC).
pub fn rfc3339(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

fn parse_rfc3339(s: &str) -> Option<i64> {
    if s.len() < 19 {
        return None;
    }
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let days = days_from_civil(num(0..4)?, num(5..7)? as u32, num(8..10)? as u32);
    Some(days * 86_400 + num(11..13)? * 3600 + num(14..16)? * 60 + num(17..19)?)
}

// Howard Hinnant's date algorithms.
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kursi() -> AyahRef {
        AyahRef::new(2, 255)
    }

    #[test]
    fn crud_and_persistence() {
        let dir = std::env::temp_dir().join(format!("quran-rs-study-{}", std::process::id()));
        let path = dir.join("study.sqlite");
        let _ = std::fs::remove_dir_all(&dir);
        {
            let mut s = StudyStore::open(&path).unwrap();
            let g = s.generation();
            s.set_highlight(kursi(), Some(HighlightColor::Yellow)).unwrap();
            s.set_highlight(kursi(), Some(HighlightColor::Green)).unwrap();
            s.set_note(kursi(), "Ayat al-Kursi").unwrap();
            assert!(s.toggle_bookmark(AyahRef::new(18, 1)).unwrap());
            s.set_setting("theme", "dark").unwrap();
            assert!(s.generation() > g);
        }
        let mut s = StudyStore::open(&path).unwrap();
        assert_eq!(s.highlight(kursi()), Some(HighlightColor::Green));
        assert_eq!(s.note(kursi()).unwrap().body, "Ayat al-Kursi");
        assert!(s.is_bookmarked(AyahRef::new(18, 1)));
        assert_eq!(s.setting("theme").as_deref(), Some("dark"));
        assert_eq!(s.counts(), (1, 1, 1));
        s.set_note(kursi(), "  ").unwrap();
        s.set_highlight(kursi(), None).unwrap();
        assert!(!s.toggle_bookmark(AyahRef::new(18, 1)).unwrap());
        assert_eq!(s.counts(), (0, 0, 0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reading_sessions_and_plans() {
        let mut s = StudyStore::open_in_memory().unwrap();
        let mut session = ReadingSession {
            started: 1_000,
            surah: SurahId(18),
            translation: "en.sahih".into(),
            seconds: 30,
            furthest: 10,
            ayahs: 110,
        };
        s.record_reading(&session).unwrap();
        session.seconds = 400;
        session.furthest = 110;
        s.record_reading(&session).unwrap();
        s.record_reading(&ReadingSession {
            started: 2_000,
            surah: SurahId(19),
            furthest: 3,
            ..session.clone()
        })
        .unwrap();
        let all = s.reading_since(0);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].surah, SurahId(19), "newest first");
        assert!(all[1].finished() && !all[0].finished());
        s.clear_reading(1_500).unwrap();
        assert_eq!(s.reading_since(0).len(), 1);

        s.start_plan("khatm-30", 1_000).unwrap();
        s.set_plan_read("khatm-30", 1, 0, true).unwrap();
        s.set_plan_read("khatm-30", 2, 0, true).unwrap();
        s.set_plan_read("khatm-30", 2, 0, false).unwrap();
        assert_eq!(s.plan_reads("khatm-30").into_iter().collect::<Vec<_>>(), vec![(1, 0)]);
        s.finish_plan("khatm-30", Some(3_000)).unwrap();
        assert_eq!(s.plans()[0].finished, Some(3_000));
        s.stop_plan("khatm-30").unwrap();
        assert!(s.plans().is_empty());
    }

    #[test]
    fn json_round_trip_and_markdown() {
        let mut a = StudyStore::open_in_memory().unwrap();
        a.set_highlight(kursi(), Some(HighlightColor::Pink)).unwrap();
        a.set_note(kursi(), "line one\nline two").unwrap();
        a.set_bookmark(AyahRef::new(36, 1), Some("Heart of the Quran")).unwrap();
        let text = |r: AyahRef| Some(format!("text of {r}"));
        let json = a.export_json("Sahih", text).unwrap();
        assert!(json.contains("\"reference\": \"Al-Baqarah 2:255\""), "{json}");
        assert!(json.contains("\"color\": \"pink\""));

        let mut b = StudyStore::open_in_memory().unwrap();
        assert_eq!(b.import_json(&json).unwrap(), (1, 1, 1));
        assert_eq!(b.highlight(kursi()), Some(HighlightColor::Pink));
        assert_eq!(b.note(kursi()).unwrap().body, "line one\nline two");
        assert_eq!(b.bookmark(AyahRef::new(36, 1)).unwrap().label, "Heart of the Quran");
        let md = b.export_markdown("Sahih", text);
        assert!(md.contains("## Bookmarks"));
        assert!(md.contains("- **Ya-Sin 36:1** — Heart of the Quran"), "{md}");
        assert!(md.contains("- **2:255** (pink) text of 2:255"));
        assert!(b.import_json("{}").is_err());

        a.record_reading(&ReadingSession {
            started: 1_000,
            surah: SurahId(2),
            translation: "en.sahih".into(),
            seconds: 300,
            furthest: 286,
            ayahs: 286,
        })
        .unwrap();
        a.start_plan("khatm-30", 900).unwrap();
        a.set_plan_read("khatm-30", 1, 0, true).unwrap();
        let all = a.export_json_with("Sahih", text, Include::EVERYTHING).unwrap();
        let mut c = StudyStore::open_in_memory().unwrap();
        c.import_json(&all).unwrap();
        assert_eq!(c.reading_since(0).len(), 1);
        assert_eq!(c.plan_reads("khatm-30").len(), 1);
        c.erase_all().unwrap();
        assert_eq!((c.counts(), c.reading_since(0).len(), c.plans().len()), ((0, 0, 0), 0, 0));
    }

    #[test]
    fn dates() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(1_791_158_400), "2026-10-05T00:00:00Z");
        for t in [0, 951_782_400, 1_791_158_399, 4_102_444_800] {
            assert_eq!(parse_rfc3339(&rfc3339(t)), Some(t));
        }
    }
}
