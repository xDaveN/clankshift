//! The app icon, drawn in code so it is sharp at every size and there is no image asset to maintain.
//! Used at runtime (tray, settings window) and by build.rs (the .exe icon on Windows).

/// Straight-alpha RGBA pixels of a `size`×`size` icon: an amber clock face.
pub fn rgba(size: u32) -> Vec<u8> {
    let s = size as f32;
    // Coverage of a shape with signed distance `d` (unit coordinates), anti-aliased over one pixel.
    let cover = |d: f32| (0.5 - d * s).clamp(0.0, 1.0);
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let (px, py) = ((x as f32 + 0.5) / s, (y as f32 + 0.5) / s);
            let r = (px - 0.5).hypot(py - 0.5);
            let face = cover(r - 0.47);
            let rim = cover((r - 0.44).abs() - 0.04);
            let hands = cover(segment(px, py, (0.5, 0.5), (0.5, 0.2)) - 0.05)
                .max(cover(segment(px, py, (0.5, 0.5), (0.71, 0.5)) - 0.05));
            // Vertical gradient from light to deep amber.
            let top = [250.0, 190.0, 80.0];
            let bottom = [226.0, 128.0, 20.0];
            let mut c = [0.0; 3];
            for i in 0..3 {
                let base = top[i] + (bottom[i] - top[i]) * py;
                let with_rim = base + ([176.0, 92.0, 10.0][i] - base) * rim;
                c[i] = with_rim + ([43.0, 30.0, 20.0][i] - with_rim) * hands;
            }
            out.extend_from_slice(&[c[0] as u8, c[1] as u8, c[2] as u8, (face * 255.0) as u8]);
        }
    }
    out
}

/// Distance from point (px, py) to the segment a–b.
fn segment(px: f32, py: f32, a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let t = (((px - a.0) * dx + (py - a.1) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    (px - a.0 - t * dx).hypot(py - a.1 - t * dy)
}
