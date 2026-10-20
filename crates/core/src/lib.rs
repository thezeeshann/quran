//! Core of quran-rs: the Quran's text and structure (surahs, ayahs, juz, hizb, pages), the
//! translations and their library, full-text search in Arabic and translation, reference
//! parsing, reading plans and local study storage. No GUI code lives here.

pub mod catalog;
pub mod copy;
pub mod library;
pub mod meta;
pub mod plans;
pub mod reference;
pub mod search;
pub mod study;
pub mod text;
pub mod translation;

pub use meta::{AYAH_COUNT, AyahRef, JUZ_COUNT, Juz, PAGE_COUNT, SURAH_COUNT, SurahId};
pub use translation::{Translation, TranslationMeta};
