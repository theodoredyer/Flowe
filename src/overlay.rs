//! Tiny pill near the bottom of the screen. Click-through, never takes focus.
//! Only redraws (on a 30 fps timer) while visible; hidden = zero work.

use tiny_skia::{FillRule, Paint, Path, PathBuilder, Pixmap, Transform};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::GetDpiForSystem;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::win::wide;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    Hidden,
    Recording,
    Locked,
    Transcribing,
    /// Nothing was heard; shown briefly.
    Nothing,
}

const BARS: usize = 9;
const BG: [u8; 4] = [24, 24, 27, 240];
const FG: [u8; 4] = [250, 250, 250, 255];
const RED: [u8; 4] = [239, 68, 68, 255];
const AMBER: [u8; 4] = [245, 158, 11, 255];
const DIM: [u8; 4] = [113, 113, 122, 255];

pub struct Overlay {
    hwnd: HWND,
    w: i32,
    h: i32,
    s: f32,
    pixmap: Pixmap,
    memdc: HDC,
    dib: HBITMAP,
    bits: *mut u8,
    pub view: View,
    tick: u32,
    levels: [f32; BARS],
}

impl Overlay {
    pub fn new() -> Option<Self> {
        unsafe {
            let s = GetDpiForSystem() as f32 / 96.0;
            let (w, h) = ((128.0 * s) as i32, (34.0 * s) as i32);
            let class = wide("parakey-overlay");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(DefWindowProcW),
                hInstance: GetModuleHandleW(core::ptr::null()),
                lpszClassName: class.as_ptr(),
                ..core::mem::zeroed()
            };
            RegisterClassW(&wc);
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
                class.as_ptr(),
                core::ptr::null(),
                WS_POPUP,
                0,
                0,
                w,
                h,
                core::ptr::null_mut(),
                core::ptr::null_mut(),
                wc.hInstance,
                core::ptr::null(),
            );
            if hwnd.is_null() {
                return None;
            }
            // One DIB for the lifetime of the app (no per-frame GDI allocations).
            let screen = GetDC(core::ptr::null_mut());
            let memdc = CreateCompatibleDC(screen);
            ReleaseDC(core::ptr::null_mut(), screen);
            let mut bmi: BITMAPINFO = core::mem::zeroed();
            bmi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
            bmi.bmiHeader.biWidth = w;
            bmi.bmiHeader.biHeight = -h; // top-down
            bmi.bmiHeader.biPlanes = 1;
            bmi.bmiHeader.biBitCount = 32;
            let mut bits = core::ptr::null_mut();
            let dib = CreateDIBSection(memdc, &bmi, DIB_RGB_COLORS, &mut bits, core::ptr::null_mut(), 0);
            if dib.is_null() || bits.is_null() {
                return None;
            }
            SelectObject(memdc, dib);
            Some(Self {
                hwnd,
                w,
                h,
                s,
                pixmap: Pixmap::new(w as u32, h as u32)?,
                memdc,
                dib,
                bits: bits as *mut u8,
                view: View::Hidden,
                tick: 0,
                levels: [0.0; BARS],
            })
        }
    }

    pub fn set(&mut self, view: View) {
        let was = self.view;
        self.view = view;
        self.tick = 0;
        unsafe {
            if view == View::Hidden {
                ShowWindow(self.hwnd, SW_HIDE);
            } else if was == View::Hidden {
                self.levels = [0.0; BARS];
                self.place();
                self.render(0.0);
                ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            } else {
                self.render(0.0);
            }
        }
    }

    /// Called by the animation timer. Returns false once hidden (timer can stop).
    pub fn tick(&mut self, level: f32) -> bool {
        if self.view == View::Hidden {
            return false;
        }
        self.tick += 1;
        if self.view == View::Nothing && self.tick > 30 {
            self.set(View::Hidden);
            return false;
        }
        self.render(level);
        true
    }

    /// Bottom-center of the monitor the mouse is on.
    unsafe fn place(&self) {
        unsafe {
            let mut pt = POINT { x: 0, y: 0 };
            GetCursorPos(&mut pt);
            let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTOPRIMARY);
            let mut mi: MONITORINFO = core::mem::zeroed();
            mi.cbSize = size_of::<MONITORINFO>() as u32;
            GetMonitorInfoW(mon, &mut mi);
            let r = mi.rcWork;
            let x = r.left + (r.right - r.left - self.w) / 2;
            let y = r.bottom - self.h - (48.0 * self.s) as i32;
            SetWindowPos(self.hwnd, HWND_TOPMOST, x, y, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }

    fn render(&mut self, level: f32) {
        let (w, h, s) = (self.w as f32, self.h as f32, self.s);
        let px = &mut self.pixmap;
        px.fill(tiny_skia::Color::TRANSPARENT);
        fill(px, capsule(0.0, 0.0, w, h), BG);
        let (cy, r) = (h / 2.0, h / 2.0);

        match self.view {
            View::Recording | View::Locked => {
                let dot = if self.view == View::Recording { RED } else { AMBER };
                fill(px, circle(r, cy, 5.0 * s), dot);
                self.levels.rotate_left(1);
                self.levels[BARS - 1] = (level * 12.0).min(1.0);
                let (bw, gap) = (4.0 * s, 3.0 * s);
                let mut x = r + 14.0 * s;
                for lv in self.levels {
                    let bh = (lv * (h - 14.0 * s)).max(4.0 * s);
                    fill(px, capsule(x, cy - bh / 2.0, bw, bh), FG);
                    x += bw + gap;
                }
                if self.view == View::Locked {
                    let lx = w - r - 1.0 * s;
                    fill(px, capsule(lx - 3.6 * s, cy - 8.0 * s, 7.2 * s, 10.0 * s), AMBER); // shackle
                    fill(px, capsule(lx - 2.0 * s, cy - 6.4 * s, 4.0 * s, 8.0 * s), BG);
                    fill(px, rect(lx - 5.0 * s, cy - 2.5 * s, 10.0 * s, 8.0 * s), AMBER); // body
                }
            }
            View::Transcribing => {
                for i in 0..3 {
                    let on = ((self.tick / 6) as i32 - i).rem_euclid(3) == 0;
                    fill(px, circle(w / 2.0 + (i - 1) as f32 * 14.0 * s, cy, 3.5 * s), if on { FG } else { DIM });
                }
            }
            View::Nothing => fill(px, capsule(w / 2.0 - 14.0 * s, cy - 1.5 * s, 28.0 * s, 3.0 * s), DIM),
            View::Hidden => return,
        }
        self.present();
    }

    fn present(&self) {
        let data = self.pixmap.data();
        // SAFETY: `bits` is our w*h*4 DIB, alive as long as self.
        let dst = unsafe { core::slice::from_raw_parts_mut(self.bits, data.len()) };
        for (d, s) in dst.chunks_exact_mut(4).zip(data.chunks_exact(4)) {
            d.copy_from_slice(&[s[2], s[1], s[0], s[3]]); // premultiplied RGBA -> BGRA
        }
        unsafe {
            let size = SIZE { cx: self.w, cy: self.h };
            let src = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
            UpdateLayeredWindow(self.hwnd, core::ptr::null_mut(), core::ptr::null(), &size, self.memdc, &src, 0, &blend, ULW_ALPHA);
        }
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        unsafe {
            DeleteDC(self.memdc);
            DeleteObject(self.dib);
            DestroyWindow(self.hwnd);
        }
    }
}

pub fn fill(px: &mut Pixmap, path: Path, c: [u8; 4]) {
    let mut paint = Paint::default();
    paint.set_color_rgba8(c[0], c[1], c[2], c[3]);
    paint.anti_alias = true;
    px.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
}

pub fn circle(x: f32, y: f32, r: f32) -> Path {
    PathBuilder::from_circle(x, y, r).unwrap_or_else(|| rect(x, y, 1.0, 1.0))
}

pub fn rect(x: f32, y: f32, w: f32, h: f32) -> Path {
    PathBuilder::from_rect(tiny_skia::Rect::from_xywh(x, y, w.max(0.1), h.max(0.1)).unwrap())
}

/// Rectangle with fully rounded ends (radius = half the short side).
pub fn capsule(x: f32, y: f32, w: f32, h: f32) -> Path {
    let r = w.min(h) / 2.0;
    let k = 0.5523 * r;
    let (x1, y1) = (x + w, y + h);
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x1 - r, y);
    pb.cubic_to(x1 - r + k, y, x1, y + r - k, x1, y + r);
    pb.line_to(x1, y1 - r);
    pb.cubic_to(x1, y1 - r + k, x1 - r + k, y1, x1 - r, y1);
    pb.line_to(x + r, y1);
    pb.cubic_to(x + r - k, y1, x, y1 - r + k, x, y1 - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish().unwrap_or_else(|| rect(x, y, w, h))
}
