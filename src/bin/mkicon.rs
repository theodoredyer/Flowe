//! Regenerates assets/flowe.ico from the same badge drawing the app uses for its tray icon:
//! `cargo run --release --bin mkicon`
#![allow(dead_code)] // draw.rs has helpers only the main app uses

#[path = "../draw.rs"]
mod draw;
#[path = "../icon.rs"]
mod icon;

fn main() {
    let sizes = [16u32, 20, 24, 32, 40, 48, 64, 128, 256];
    let header_len = 6 + 16 * sizes.len();
    let (mut dir, mut data) = (Vec::new(), Vec::new());
    for &n in &sizes {
        let px = icon::badge(n, icon::INK, 255).expect("pixmap");
        let mask_row = n.div_ceil(32) * 4; // 1-bit AND mask rows are padded to 32 bits
        let mut img = Vec::new();
        // BITMAPINFOHEADER; height is doubled because it covers the XOR (color) and AND masks.
        img.extend_from_slice(&40u32.to_le_bytes());
        img.extend_from_slice(&(n as i32).to_le_bytes());
        img.extend_from_slice(&(2 * n as i32).to_le_bytes());
        img.extend_from_slice(&1u16.to_le_bytes());
        img.extend_from_slice(&32u16.to_le_bytes());
        img.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
        img.extend_from_slice(&(n * n * 4 + mask_row * n).to_le_bytes());
        img.extend_from_slice(&[0u8; 16]);
        for y in (0..n).rev() {
            for x in 0..n {
                let p = px.pixel(x, y).unwrap().demultiply(); // ICO wants straight alpha, BGRA, bottom-up
                img.extend_from_slice(&[p.blue(), p.green(), p.red(), p.alpha()]);
            }
        }
        img.resize(img.len() + (mask_row * n) as usize, 0); // AND mask: fully opaque = all zero
        let dim = |v: u32| if v >= 256 { 0u8 } else { v as u8 };
        dir.extend_from_slice(&[dim(n), dim(n), 0, 0]);
        dir.extend_from_slice(&1u16.to_le_bytes());
        dir.extend_from_slice(&32u16.to_le_bytes());
        dir.extend_from_slice(&(img.len() as u32).to_le_bytes());
        dir.extend_from_slice(&((header_len + data.len()) as u32).to_le_bytes());
        data.extend_from_slice(&img);
    }
    let mut out = Vec::with_capacity(header_len + data.len());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    out.extend_from_slice(&dir);
    out.extend_from_slice(&data);
    std::fs::write("assets/flowe.ico", &out).expect("write assets/flowe.ico");
    println!("wrote assets/flowe.ico ({} bytes, {} sizes)", out.len(), sizes.len());
}
