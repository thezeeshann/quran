//! Draws the app icon — an emerald tile with the eight-pointed star of the hizb in white around a
//! gold centre — as `assets/icon.png` (1024 px, the macOS icon grid's 824 px tile with its
//! shadow) and `assets/icon-256.png` (the window icon), from signed distances, antialiased.
//!
//! ```text
//! cargo run -p quran-rs --example make_icon
//! ```

use image::{Rgba, RgbaImage};

fn sd_box(px: f32, py: f32, hx: f32, hy: f32, r: f32) -> f32 {
    let qx = px.abs() - hx + r;
    let qy = py.abs() - hy + r;
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    outside + qx.max(qy).min(0.0) - r
}

fn cover(d: f32) -> f32 {
    (0.5 - d).clamp(0.0, 1.0)
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// Paints `src` (straight alpha) over `dst`.
fn over(dst: [f32; 4], src: [f32; 3], a: f32) -> [f32; 4] {
    let out_a = a + dst[3] * (1.0 - a);
    if out_a <= 0.0 {
        return [0.0; 4];
    }
    let c = |i: usize| (src[i] * a + dst[i] * dst[3] * (1.0 - a)) / out_a;
    [c(0), c(1), c(2), out_a]
}

fn render(size: u32) -> RgbaImage {
    let s = size as f32 / 1024.0;
    let mut img = RgbaImage::new(size, size);
    let top = [38.0, 132.0, 106.0];
    let bottom = [14.0, 64.0, 51.0];
    let white = [255.0, 255.0, 255.0];
    let gold = [241.0, 222.0, 166.0];
    let (cx, cy) = (512.0 * s, 500.0 * s);
    let half = 412.0 * s;
    let radius = 185.0 * s;
    for y in 0..size {
        for x in 0..size {
            let (px, py) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            let mut c = [0.0f32; 4];
            // Shadow: soft, a little below the tile.
            let ds = sd_box(px, py - 12.0 * s, half, half, radius);
            let shadow = 0.32 * (-(ds.max(0.0)) / (28.0 * s)).exp() * (1.0 - cover(ds));
            c = over(c, [0.0, 0.0, 0.0], shadow);
            // The tile, its gradient running down and slightly right.
            let d = sd_box(px, py, half, half, radius);
            let t = ((py * 0.94 + px * 0.34) / (2.0 * half) + 0.5).clamp(0.0, 1.0);
            let mut tile = mix(top, bottom, t);
            // A faint glow behind the star.
            let glow = (1.0 - (px * px + py * py).sqrt() / (360.0 * s)).max(0.0);
            tile = mix(tile, [70.0, 170.0, 140.0], glow * 0.35);
            c = over(c, tile, cover(d));
            // A hairline of light along the top edge.
            let rim = (d + 2.0 * s).abs() - 1.5 * s;
            c = over(c, white, cover(rim) * 0.18 * (1.0 - t));
            if cover(d) > 0.0 {
                // The star: two squares, one turned 45°, outlined in white.
                let star_r = 262.0 * s;
                let stroke = 22.0 * s;
                let h = star_r / std::f32::consts::SQRT_2;
                let (rx, ry) = ((px + py) / std::f32::consts::SQRT_2, (py - px) / std::f32::consts::SQRT_2);
                let sq1 = sd_box(px, py, h, h, 6.0 * s).abs() - stroke / 2.0;
                let sq2 = sd_box(rx, ry, h, h, 6.0 * s).abs() - stroke / 2.0;
                c = over(c, white, cover(sq1.min(sq2)) * cover(d));
                // A gold ring and dot at the centre.
                let r = (px * px + py * py).sqrt();
                let ring = (r - 96.0 * s).abs() - 9.0 * s;
                c = over(c, gold, cover(ring) * cover(d));
                c = over(c, gold, cover(r - 34.0 * s) * cover(d));
            }
            img.put_pixel(
                x,
                y,
                Rgba([
                    c[0].round() as u8,
                    c[1].round() as u8,
                    c[2].round() as u8,
                    (c[3] * 255.0).round() as u8,
                ]),
            );
        }
    }
    img
}

fn main() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
    render(1024).save(dir.join("icon.png")).expect("icon.png");
    render(256).save(dir.join("icon-256.png")).expect("icon-256.png");
    println!("wrote {}", dir.display());
}
