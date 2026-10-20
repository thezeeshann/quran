//! Formatting ayahs for the clipboard: Arabic, translation and reference.

use crate::meta::AyahRef;
use crate::reference::format_range;
use crate::translation::Translation;

/// What a copy carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CopyParts {
    pub arabic: bool,
    pub translation: bool,
}

impl Default for CopyParts {
    fn default() -> Self {
        Self { arabic: true, translation: true }
    }
}

/// One ayah:
///
/// ```text
/// ٱللَّهُ لَآ إِلَٰهَ إِلَّا هُوَ … ﴿٢٥٥﴾
/// “Allah - there is no deity except Him…” — Al-Baqarah 2:255 (Sahih)
/// ```
///
/// Several: the reference line, then the Arabic with its ayah markers, then the numbered
/// translation.
pub fn format_ayahs(
    arabic: impl Fn(AyahRef) -> String,
    t: Option<&Translation>,
    refs: &[AyahRef],
    parts: CopyParts,
) -> Option<String> {
    let mut refs: Vec<AyahRef> = refs.iter().copied().filter(|r| r.is_valid()).collect();
    refs.sort();
    refs.dedup();
    let first = *refs.first()?;
    let last = *refs.last()?;
    let abbrev = t.map(|t| t.meta.abbrev.as_str()).unwrap_or("");
    let suffix = if abbrev.is_empty() || !parts.translation { String::new() } else { format!(" ({abbrev})") };
    let marker = |r: AyahRef| format!("﴿{}﴾", crate::text::arabic_number(r.ayah as u32));
    let ar: Vec<String> = refs.iter().map(|&r| format!("{} {}", arabic(r), marker(r))).collect();
    if refs.len() == 1 {
        let mut out = String::new();
        if parts.arabic {
            out.push_str(&ar[0]);
            out.push('\n');
        }
        match t.filter(|_| parts.translation) {
            Some(t) => out.push_str(&format!("“{}” — {}{suffix}", t.get(first), first.long())),
            None => out.push_str(&format!("— {}", first.long())),
        }
        return Some(out);
    }
    let mut out = format!("{}{suffix}\n", format_range(first, last));
    if parts.arabic {
        out.push_str(&ar.join(" "));
        out.push('\n');
    }
    if let Some(t) = t.filter(|_| parts.translation) {
        let body: Vec<String> = refs.iter().map(|r| format!("{} {}", r.ayah, t.get(*r))).collect();
        out.push_str(&body.join(" "));
    }
    Some(out.trim_end().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_and_range() {
        let arabic = |r: AyahRef| format!("A{}", r.ayah);
        let one = format_ayahs(arabic, None, &[AyahRef::new(112, 1)], CopyParts::default()).unwrap();
        assert_eq!(one, "A1 ﴿١﴾\n— Al-Ikhlas 112:1");
        let two =
            format_ayahs(arabic, None, &[AyahRef::new(112, 2), AyahRef::new(112, 1)], CopyParts::default())
                .unwrap();
        assert_eq!(two, "Al-Ikhlas 112:1–2\nA1 ﴿١﴾ A2 ﴿٢﴾");
        assert!(format_ayahs(arabic, None, &[], CopyParts::default()).is_none());
    }
}
