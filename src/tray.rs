//! Notification-area icon: the "F" badge. Dark = ready, red = recording, grey = loading,
//! amber = model error, faint = paused.

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::icon;

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
    icons: [HICON; 5], // indexed by Status
    pub status: Status,
    tip: String,
}

impl Tray {
    pub fn new(hwnd: HWND, msg: u32) -> Self {
        let n = unsafe { GetSystemMetrics(SM_CXSMICON).max(16) } as u32;
        let styles: [([u8; 4], u8); 5] = [
            ([113, 113, 122, 255], 255), // loading
            (icon::INK, 255),            // ready
            ([239, 68, 68, 255], 255),   // recording
            ([245, 158, 11, 255], 255),  // error
            ([113, 113, 122, 255], 120), // paused
        ];
        let icons = styles.map(|(bg, alpha)| icon::badge(n, bg, alpha).map_or(core::ptr::null_mut(), hicon));
        let mut t = Self { hwnd, msg, icons, status: Status::Loading, tip: "Fleow - loading model...".into() };
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

fn hicon(px: tiny_skia::Pixmap) -> HICON {
    unsafe {
        let n = px.width() as i32;
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
