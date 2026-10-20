//! Reading history: how long each surah was read and how far, saved as the reader moves on.
//!
//! Time counts while the window has focus, between frames up to a couple of minutes apart (egui
//! only draws when something changes, and reading a screenful changes nothing). A session shorter
//! than a few seconds — paging past a surah — isn't kept.
//!
//! Ported from bible-rs (MIT).

use quran_core::AyahRef;
use quran_core::study::{ReadingSession, StudyStore, now};

/// Sessions shorter than this aren't history.
const MIN_SECONDS: f64 = 8.0;
/// Gaps between frames longer than this aren't counted as reading.
const MAX_GAP: f64 = 120.0;
/// Save an open session this often.
const SAVE_EVERY: f64 = 20.0;

#[derive(Default)]
pub struct Tracker {
    session: Option<ReadingSession>,
    seconds: f64,
    last: Option<f64>,
    saved_at: f64,
}

impl Tracker {
    /// Called once a frame with where the reader is (`furthest`: the furthest ayah in view).
    pub fn tick(
        &mut self,
        time: f64,
        focused: bool,
        at: AyahRef,
        furthest: u16,
        translation: &str,
        study: &StudyStore,
    ) {
        let surah = at.surah_id();
        let same = self.session.as_ref().is_some_and(|s| s.surah == surah && s.translation == translation);
        if !same {
            self.finish(study);
            self.session = Some(ReadingSession {
                started: now(),
                surah,
                translation: translation.to_string(),
                seconds: 0,
                furthest: 0,
                ayahs: surah.ayahs(),
            });
            self.seconds = 0.0;
            self.saved_at = time;
        }
        if let Some(last) = self.last
            && focused
        {
            let gap = time - last;
            if gap > 0.0 && gap < MAX_GAP {
                self.seconds += gap;
            }
        }
        self.last = Some(time);
        if let Some(s) = &mut self.session {
            s.furthest = s.furthest.max(furthest);
            s.seconds = self.seconds as u32;
        }
        if time - self.saved_at > SAVE_EVERY {
            self.save(study);
            self.saved_at = time;
        }
    }

    fn save(&self, study: &StudyStore) {
        if let Some(s) = &self.session
            && self.seconds >= MIN_SECONDS
        {
            let _ = study.record_reading(s);
        }
    }

    /// Saves the open session (on leaving a surah, and on quit).
    pub fn finish(&mut self, study: &StudyStore) {
        self.save(study);
        self.session = None;
    }
}
