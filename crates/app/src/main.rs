//! quran-rs: a fast, native Quran reader.
//!
//! Environment:
//! - `QURAN_RS_HOME` — data directory override (study database, translations, backups)
//! - `QURAN_RS_RENDERER=glow|wgpu` — force a renderer (default: wgpu/Metal on macOS, glow elsewhere)
//! - `QURAN_RS_STARTUP_PROBE=1` — print startup timings as JSON after the first frame and exit
//! - `QURAN_RS_SCROLL_PROBE=1` — scroll through the Quran programmatically, print frame timings, exit
//! - `QURAN_RS_BACKGROUND=1` — macOS: open the window without taking focus (screenshots, benchmarks)

fn main() -> eframe::Result {
    quran_rs::run()
}
