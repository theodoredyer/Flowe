//! The "F" badge: a rounded square with a bold white F. Used for the tray icon, the window
//! icon and (via `cargo run --bin mkicon`) the exe icon, so they all match.

use tiny_skia::Pixmap;
use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::Graphics::Gdi::*;

use crate::draw::{fill, rounded_rect};

pub const INK: [u8; 4] = [24, 24, 27, 255];

/// Rounded square of `bg` with a white F. `alpha` fades the whole badge (the "paused" look).
pub fn badge(size: u32, bg: [u8; 4], alpha: u8) -> Option<Pixmap> {
    let mut px = Pixmap::new(size, size)?;
    let s = size as f32;
    fill(&mut px, rounded_rect(0.0, 0.0, s, s, s * 0.24), bg);
    let mask = glyph_mask("F", size as i32, (s * 0.80).round() as i32);
    let data = px.data_mut();
    for (i, &a) in mask.iter().enumerate() {
        if a == 0 {
            continue;
        }
        // Premultiplied white (a,a,a,a) over the badge.
        let a = a as u32;
        for c in &mut data[i * 4..i * 4 + 4] {
            *c = (a + *c as u32 * (255 - a) / 255).min(255) as u8;
        }
    }
    if alpha < 255 {
        for c in px.data_mut() {
            *c = (*c as u32 * alpha as u32 / 255) as u8;
        }
    }
    Some(px)
}

/// Coverage mask (size x size) of `text` in Segoe UI Bold at `font_px`, with the ink centered.
/// tiny-skia has no text, so GDI draws white-on-black and we read the result back.
pub fn glyph_mask(text: &str, size: i32, font_px: i32) -> Vec<u8> {
    let n = (size * size) as usize;
    let mut mask = vec![0u8; n];
    unsafe {
        let dc = CreateCompatibleDC(core::ptr::null_mut());
        let mut bmi: BITMAPINFO = core::mem::zeroed();
        bmi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = size;
        bmi.bmiHeader.biHeight = -size;
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        let mut bits: *mut core::ffi::c_void = core::ptr::null_mut();
        let bmp = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, core::ptr::null_mut(), 0);
        if bmp.is_null() || bits.is_null() {
            DeleteDC(dc);
            return mask;
        }
        let old_bmp = SelectObject(dc, bmp);
        let r = RECT { left: 0, top: 0, right: size, bottom: size };
        FillRect(dc, &r, GetStockObject(BLACK_BRUSH));
        let face: Vec<u16> = "Segoe UI".encode_utf16().chain(Some(0)).collect();
        let font = CreateFontW(
            -font_px,
            0,
            0,
            0,
            FW_BOLD as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_TT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            ANTIALIASED_QUALITY as u32,
            (DEFAULT_PITCH | FF_DONTCARE) as u32,
            face.as_ptr(),
        );
        let old_font = SelectObject(dc, font);
        SetBkMode(dc, TRANSPARENT as i32);
        SetTextColor(dc, 0x00FF_FFFF);
        let mut t: Vec<u16> = text.encode_utf16().collect();
        let mut rr = r;
        DrawTextW(dc, t.as_mut_ptr(), t.len() as i32, &mut rr, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
        GdiFlush();
        let px = core::slice::from_raw_parts(bits as *const u8, n * 4);
        for (m, p) in mask.iter_mut().zip(px.chunks_exact(4)) {
            *m = p[1];
        }
        SelectObject(dc, old_font);
        DeleteObject(font);
        SelectObject(dc, old_bmp);
        DeleteObject(bmp);
        DeleteDC(dc);
    }
    center_ink(&mut mask, size as usize);
    mask
}

/// DrawText centers the line box (ascent+descent), which leaves a capital sitting high; recenter on the ink.
fn center_ink(mask: &mut [u8], s: usize) {
    let (mut x0, mut y0, mut x1, mut y1) = (s, s, 0, 0);
    for y in 0..s {
        for x in 0..s {
            if mask[y * s + x] > 0 {
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            }
        }
    }
    if x1 < x0 {
        return;
    }
    let dx = (s as i32 - (x0 + x1 + 1) as i32) / 2;
    let dy = (s as i32 - (y0 + y1 + 1) as i32) / 2;
    if dx == 0 && dy == 0 {
        return;
    }
    let src = mask.to_vec();
    mask.fill(0);
    for y in 0..s {
        for x in 0..s {
            let (nx, ny) = (x as i32 + dx, y as i32 + dy);
            if (0..s as i32).contains(&nx) && (0..s as i32).contains(&ny) {
                mask[ny as usize * s + nx as usize] = src[y * s + x];
            }
        }
    }
}
