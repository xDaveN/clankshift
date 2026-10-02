//! Embeds the app icon and version info into the Windows executable.

#[cfg(windows)]
#[path = "src/icon.rs"]
mod icon;

fn main() {
    println!("cargo:rerun-if-changed=src/icon.rs");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let ico = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("clankshift.ico");
        std::fs::write(&ico, ico_file(&[16, 20, 24, 32, 40, 48, 64, 256])).unwrap();
        let mut res = winresource::WindowsResource::new();
        res.set_icon(ico.to_str().unwrap())
            .set("ProductName", "ClankShift")
            .set("FileDescription", "ClankShift"); // shown by Task Manager
        res.compile().expect("embedding the Windows icon failed");
    }
}

/// A .ico with one uncompressed 32-bit image per size.
#[cfg(windows)]
fn ico_file(sizes: &[u32]) -> Vec<u8> {
    let images: Vec<Vec<u8>> = sizes.iter().map(|&s| dib(s)).collect();
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (&s, img) in sizes.iter().zip(&images) {
        let dim = if s >= 256 { 0 } else { s as u8 };
        out.extend_from_slice(&[dim, dim, 0, 0, 1, 0, 32, 0]);
        out.extend_from_slice(&(img.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += img.len() as u32;
    }
    images.iter().for_each(|img| out.extend_from_slice(img));
    out
}

/// BITMAPINFOHEADER + bottom-up BGRA pixels + an empty AND mask.
#[cfg(windows)]
fn dib(size: u32) -> Vec<u8> {
    let rgba = icon::rgba(size);
    let mask_row = size.div_ceil(32) * 4;
    let mut out = Vec::new();
    for v in [40, size, size * 2] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&[0; 24]);
    for row in rgba.chunks(size as usize * 4).rev() {
        for px in row.chunks(4) {
            out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
        }
    }
    out.resize(out.len() + (mask_row * size) as usize, 0);
    out
}
