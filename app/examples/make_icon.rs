//! Draws Gamut's icon (a gradient "G" ring on a dark tile) into app/assets: gamut.png and gamut.ico.
//! `cargo run -p gamut --example make_icon`
use std::f32::consts::PI;

const ACCENT: [f32; 3] = [110.0, 140.0, 255.0];
const ACCENT_2: [f32; 3] = [176.0, 112.0, 255.0];
/// The G's opening, clockwise from east (screen coords, y down).
const GAP: f32 = 55.0 * PI / 180.0;

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// Color and coverage (0/1) at a point in unit space (0..1).
fn sample(x: f32, y: f32) -> [f32; 4] {
    // Rounded tile.
    let (cr, half) = (0.22, 0.5);
    let (qx, qy) = ((x - 0.5).abs() - (half - cr), (y - 0.5).abs() - (half - cr));
    let d = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - cr;
    if d > 0.0 {
        return [0.0; 4];
    }
    let bg = mix([30.0, 34.0, 52.0], [12.0, 14.0, 21.0], y);
    let mut c = if d > -0.012 { mix(bg, [70.0, 80.0, 115.0], 0.6) } else { bg };

    let (dx, dy) = (x - 0.5, y - 0.5);
    let (r, th) = ((dx * dx + dy * dy).sqrt(), 0.065);
    let ring_r = 0.29;
    // Angle clockwise from east, 0..2π; the arc runs from 0 (east) around to 2π - GAP.
    let a = dy.atan2(dx).rem_euclid(2.0 * PI);
    let span = 2.0 * PI - GAP;
    let on_arc = (r - ring_r).abs() < th && a <= span;
    // Round cap at the arc's open (upper right) end.
    let end = (span.cos() * ring_r, span.sin() * ring_r);
    let cap = ((dx - end.0).powi(2) + (dy - end.1).powi(2)).sqrt() < th;
    // Crossbar from the ring's east point inward, rounded at its inner end.
    let bar = dy.abs() < th && dx > 0.04 && dx < ring_r + th;
    let bar_cap = ((dx - 0.04).powi(2) + dy * dy).sqrt() < th;
    // Soft glow under the ring.
    let glow = ((r - ring_r).abs() - th).max(0.0);
    if glow < 0.08 && !(on_arc || cap || bar || bar_cap) {
        let g = (1.0 - glow / 0.08).powi(2) * 0.22 * if a <= span || dx > 0.0 { 1.0 } else { 0.0 };
        c = mix(c, mix(ACCENT, ACCENT_2, 0.5), g);
    }
    if on_arc || cap {
        c = mix(ACCENT, ACCENT_2, if on_arc { a / span } else { 1.0 });
    } else if bar || bar_cap {
        c = ACCENT;
    }
    [c[0], c[1], c[2], 255.0]
}

fn render(n: u32) -> image::RgbaImage {
    let ss = 6;
    image::RgbaImage::from_fn(n, n, |px, py| {
        let mut acc = [0.0f32; 4];
        for sy in 0..ss {
            for sx in 0..ss {
                let x = (px as f32 + (sx as f32 + 0.5) / ss as f32) / n as f32;
                let y = (py as f32 + (sy as f32 + 0.5) / ss as f32) / n as f32;
                let s = sample(x, y);
                // Premultiplied accumulation keeps edges clean.
                for i in 0..3 {
                    acc[i] += s[i] * s[3] / 255.0;
                }
                acc[3] += s[3];
            }
        }
        let k = (ss * ss) as f32;
        let a = acc[3] / k;
        let un = |v: f32| if a > 0.0 { (v / k * 255.0 / a).round().clamp(0.0, 255.0) as u8 } else { 0 };
        image::Rgba([un(acc[0]), un(acc[1]), un(acc[2]), a.round() as u8])
    })
}

fn png(img: &image::RgbaImage) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

fn main() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("gamut.png"), png(&render(256))).unwrap();
    // ICO with PNG-compressed entries (Vista+).
    let sizes = [16u32, 20, 24, 32, 40, 48, 64, 128, 256];
    let imgs: Vec<Vec<u8>> = sizes.iter().map(|&s| png(&render(s))).collect();
    let mut ico = vec![0, 0, 1, 0, sizes.len() as u8, 0];
    let mut off = 6 + 16 * sizes.len() as u32;
    for (s, data) in sizes.iter().zip(&imgs) {
        let b = if *s >= 256 { 0 } else { *s as u8 };
        ico.extend_from_slice(&[b, b, 0, 0, 1, 0, 32, 0]);
        ico.extend_from_slice(&(data.len() as u32).to_le_bytes());
        ico.extend_from_slice(&off.to_le_bytes());
        off += data.len() as u32;
    }
    for d in &imgs {
        ico.extend_from_slice(d);
    }
    std::fs::write(dir.join("gamut.ico"), ico).unwrap();
    println!("wrote {}", dir.display());
}
