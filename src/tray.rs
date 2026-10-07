//! Notification-area icon: grey = loading, white = ready, red = recording.

use tiny_skia::Pixmap;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::overlay::{capsule, fill, rect};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Loading,
    Ready,
    Recording,
    Error,
    Paused,
}

pub struct Tray {
    hwnd: HWND,
    msg: u32,
    icons: [HICON; 5],
    pub status: Status,
    tip: String,
}

impl Tray {
    pub fn new(hwnd: HWND, msg: u32) -> Self {
        // grey = loading, white = ready, red = recording, amber = error, faint = paused
        let colors = [[113, 113, 122, 255], [228, 228, 231, 255], [239, 68, 68, 255], [245, 158, 11, 255], [113, 113, 122, 110]];
        let mut t = Self { hwnd, msg, icons: colors.map(mic_icon), status: Status::Loading, tip: "parakey - loading model...".into() };
        t.add();
        t
    }

    fn data(&self) -> NOTIFYICONDATAW {
        let mut d: NOTIFYICONDATAW = unsafe { core::mem::zeroed() };
        d.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        d.hWnd = self.hwnd;
        d.uID = 1;
        d.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        d.uCallbackMessage = self.msg;
        d.hIcon = self.icons[self.status as usize];
        for (dst, src) in d.szTip.iter_mut().zip(self.tip.encode_utf16().take(127)) {
            *dst = src;
        }
        d
    }

    /// Also used to re-add the icon after Explorer restarts.
    pub fn add(&mut self) {
        unsafe { Shell_NotifyIconW(NIM_ADD, &self.data()) };
    }

    pub fn set(&mut self, status: Status, tip: Option<&str>) {
        if let Some(t) = tip {
            self.tip = t.into();
        }
        self.status = status;
        unsafe { Shell_NotifyIconW(NIM_MODIFY, &self.data()) };
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        unsafe {
            Shell_NotifyIconW(NIM_DELETE, &self.data());
            for i in self.icons {
                DestroyIcon(i);
            }
        }
    }
}

fn mic_icon(c: [u8; 4]) -> HICON {
    unsafe {
        let n = GetSystemMetrics(SM_CXSMICON).max(16);
        let s = n as f32 / 16.0;
        let Some(mut px) = Pixmap::new(n as u32, n as u32) else { return core::ptr::null_mut() };
        fill(&mut px, capsule(5.5 * s, 1.0 * s, 5.0 * s, 9.0 * s), c); // head
        let mut pb = tiny_skia::PathBuilder::new(); // U-shaped holder
        pb.move_to(3.5 * s, 7.0 * s);
        pb.cubic_to(3.5 * s, 13.0 * s, 12.5 * s, 13.0 * s, 12.5 * s, 7.0 * s);
        if let Some(path) = pb.finish() {
            let mut paint = tiny_skia::Paint::default();
            paint.set_color_rgba8(c[0], c[1], c[2], c[3]);
            paint.anti_alias = true;
            let stroke = tiny_skia::Stroke { width: 1.5 * s, line_cap: tiny_skia::LineCap::Round, ..Default::default() };
            px.stroke_path(&path, &paint, &stroke, tiny_skia::Transform::identity(), None);
        }
        fill(&mut px, rect(7.25 * s, 11.5 * s, 1.5 * s, 3.0 * s), c); // stem
        fill(&mut px, capsule(4.5 * s, 14.0 * s, 7.0 * s, 1.5 * s), c); // base

        let mut bmi: BITMAPINFO = core::mem::zeroed();
        bmi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = n;
        bmi.bmiHeader.biHeight = -n;
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        let mut bits = core::ptr::null_mut();
        let color = CreateDIBSection(core::ptr::null_mut(), &bmi, DIB_RGB_COLORS, &mut bits, core::ptr::null_mut(), 0);
        if color.is_null() || bits.is_null() {
            return core::ptr::null_mut();
        }
        let dst = core::slice::from_raw_parts_mut(bits as *mut u8, (n * n * 4) as usize);
        for (d, p) in dst.chunks_exact_mut(4).zip(px.pixels()) {
            let p = p.demultiply();
            d.copy_from_slice(&[p.blue(), p.green(), p.red(), p.alpha()]);
        }
        let mask_bits = vec![0u8; (n * n / 8) as usize + 64];
        let mask = CreateBitmap(n, n, 1, 1, mask_bits.as_ptr().cast());
        let info = ICONINFO { fIcon: 1, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&info);
        // The icon keeps its own copies.
        DeleteObject(color);
        DeleteObject(mask);
        icon
    }
}
