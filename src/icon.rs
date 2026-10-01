//! The app icon, drawn in code so it is sharp at every size and there is no image asset to maintain.
//! Used at runtime (tray, settings window) and by build.rs (the .exe icon on Windows).

/// Straight-alpha RGBA pixels of a `size`×`size` icon: a robot head in an amber hard hat.
pub fn rgba(size: u32) -> Vec<u8> {
    const STEEL: [f32; 3] = [85.0, 96.0, 111.0];
    const CYAN: [f32; 3] = [94.0, 230.0, 240.0];
    const INK: [f32; 3] = [43.0, 30.0, 20.0];
    const AMBER: [f32; 3] = [245.0, 165.0, 36.0];
    const DEEP_AMBER: [f32; 3] = [217.0, 130.0, 26.0];
    // Shapes are laid out on a 64×64 grid; a pixel is 64/size grid units.
    let unit = 64.0 / size as f32;
    // Coverage of a shape with signed distance `d` (grid units), anti-aliased over one pixel.
    let cover = |d: f32| (0.5 - d / unit).clamp(0.0, 1.0);
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let p = ((x as f32 + 0.5) * unit, (y as f32 + 0.5) * unit);
            // Back to front.
            let dome = ((p.0 - 32.0) / 18.0).hypot((p.1 - 27.0) / 17.0) - 1.0;
            let layers = [
                (rounded_box(p, (12.0, 26.0), (52.0, 58.0), 8.0), STEEL), // head
                (
                    circle(p, (24.0, 41.0), 5.0).min(circle(p, (40.0, 41.0), 5.0)),
                    CYAN,
                ), // eyes
                (rounded_box(p, (25.0, 50.0), (39.0, 53.5), 1.75), INK),  // mouth
                ((dome * 17.0).max(p.1 - 27.0), AMBER),                   // hat dome
                (rounded_box(p, (29.0, 10.0), (35.0, 26.0), 3.0), DEEP_AMBER), // hat ridge
                (rounded_box(p, (6.0, 24.0), (58.0, 30.0), 3.0), DEEP_AMBER), // hat brim
            ];
            // Premultiplied "over" compositing.
            let (mut c, mut a) = ([0.0f32; 3], 0.0f32);
            for (d, color) in layers {
                let k = cover(d);
                for i in 0..3 {
                    c[i] = color[i] * k + c[i] * (1.0 - k);
                }
                a = k + a * (1.0 - k);
            }
            let px = |v: f32| if a > 0.0 { (v / a).round() as u8 } else { 0 };
            out.extend_from_slice(&[px(c[0]), px(c[1]), px(c[2]), (a * 255.0).round() as u8]);
        }
    }
    out
}

fn circle(p: (f32, f32), center: (f32, f32), r: f32) -> f32 {
    (p.0 - center.0).hypot(p.1 - center.1) - r
}

/// Signed distance to the box from `min` to `max` with corner radius `r`.
fn rounded_box(p: (f32, f32), min: (f32, f32), max: (f32, f32), r: f32) -> f32 {
    let qx = (p.0 - (min.0 + max.0) / 2.0).abs() - (max.0 - min.0) / 2.0 + r;
    let qy = (p.1 - (min.1 + max.1) / 2.0).abs() - (max.1 - min.1) / 2.0 + r;
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
}
