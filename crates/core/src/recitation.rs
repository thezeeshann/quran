//! Ayah-by-ayah recitation audio, streamed from EveryAyah (everyayah.com): one MP3 per ayah,
//! named `SSSAAA.mp3`. Nothing is bundled or cached; playing needs an internet connection.

use serde::{Deserialize, Serialize};

use crate::meta::AyahRef;

const BASE: &str = "https://everyayah.com/data/";

/// The reciters to choose from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum Reciter {
    #[default]
    Alafasy,
    YasserAlDosari,
}

impl Reciter {
    pub const ALL: [Reciter; 2] = [Reciter::Alafasy, Reciter::YasserAlDosari];

    /// The reciter's name, as shown in Settings and the player.
    pub fn name(self) -> &'static str {
        match self {
            Reciter::Alafasy => "Mishary Rashid Alafasy",
            Reciter::YasserAlDosari => "Yasser Al-Dosari",
        }
    }

    /// A short name for tight places.
    pub fn short_name(self) -> &'static str {
        match self {
            Reciter::Alafasy => "Alafasy",
            Reciter::YasserAlDosari => "Al-Dosari",
        }
    }

    fn folder(self) -> &'static str {
        match self {
            Reciter::Alafasy => "Alafasy_128kbps",
            Reciter::YasserAlDosari => "Yasser_Ad-Dussary_128kbps",
        }
    }
}

/// The address of an ayah's recitation.
pub fn url(reciter: Reciter, at: AyahRef) -> String {
    format!("{BASE}{}/{:03}{:03}.mp3", reciter.folder(), at.surah, at.ayah)
}

/// Why audio couldn't be fetched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// No connection (or the server couldn't be reached in time).
    Offline,
    /// The server answered, but not with the audio.
    Unavailable(String),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::Offline => f.write_str("Recitation needs an internet connection"),
            FetchError::Unavailable(why) => write!(f, "Couldn't load the recitation ({why})"),
        }
    }
}

impl std::error::Error for FetchError {}

/// Downloads an ayah's MP3 (tens of KB, rarely more than a few MB; call it off the UI thread).
#[cfg(feature = "online")]
pub fn fetch(reciter: Reciter, at: AyahRef) -> Result<Vec<u8>, FetchError> {
    use std::time::Duration;
    if !at.is_valid() {
        return Err(FetchError::Unavailable(format!("no ayah {}:{}", at.surah, at.ayah)));
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(8)))
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .into();
    let mut resp =
        agent.get(&url(reciter, at)).header("User-Agent", "quran-rs").call().map_err(|e| match e {
            ureq::Error::StatusCode(code) => FetchError::Unavailable(format!("HTTP {code}")),
            ureq::Error::Http(_) | ureq::Error::BadUri(_) | ureq::Error::InvalidProxyUrl => {
                FetchError::Unavailable(e.to_string())
            }
            _ => FetchError::Offline,
        })?;
    resp.body_mut().with_config().limit(16 << 20).read_to_vec().map_err(|_| FetchError::Offline)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_are_zero_padded() {
        assert_eq!(
            url(Reciter::Alafasy, AyahRef::new(1, 1)),
            "https://everyayah.com/data/Alafasy_128kbps/001001.mp3"
        );
        assert_eq!(
            url(Reciter::YasserAlDosari, AyahRef::new(114, 6)),
            "https://everyayah.com/data/Yasser_Ad-Dussary_128kbps/114006.mp3"
        );
        assert_eq!(
            url(Reciter::Alafasy, AyahRef::new(2, 255)),
            "https://everyayah.com/data/Alafasy_128kbps/002255.mp3"
        );
    }
}
