//! Bottom-center indicator, click-through, never takes focus.
//! Idle: a tiny dash just above the taskbar meaning "Flowe is listening". Recording: a pill with a
//! state indicator on the left (red dot = recording, amber padlock = locked) and the chosen
//! visualization (see viz.rs) on the right. The drawing lives in pill.rs; this file puts it on
//! screen. Static states are drawn once and cost nothing; only the animated ones run a timer.

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::GetDpiForSystem;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub use crate::pill::{FPS, Idle, View};
use crate::pill::{Pill, demo_level};
use crate::viz::Style;
use crate::win::wide;

/// Length of the preview shown when a style is picked in the dashboard.
const DEMO_FRAMES: u32 = (FPS * 2.5) as u32;

pub struct Overlay {
    hwnd: HWND,
    s: f32,
    pill: Pill,
    memdc: HDC,
    dib: HBITMAP,
    bits: *mut u8,
    pub view: View,
    /// What to fall back to when a recording ends.
    idle: View,
    tick: u32,
    /// Frames left in a dashboard preview (0 = real recording or not animating).
    demo: u32,
}

impl Overlay {
    pub fn new(style: Style) -> Option<Self> {
        unsafe {
            let s = GetDpiForSystem() as f32 / 96.0;
            let pill = Pill::new(s, style)?;
            let (w, h) = (pill.w, pill.h);
            let class = wide("flowe-overlay");
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
                s,
                pill,
                memdc,
                dib,
                bits: bits as *mut u8,
                view: View::Hidden,
                idle: View::Hidden,
                tick: 0,
                demo: 0,
            })
        }
    }

    pub fn set_style(&mut self, style: Style) {
        self.pill.viz.style = style;
        self.pill.viz.reset();
    }

    /// Plays a short preview of the current style with synthetic speech. The caller starts the timer.
    pub fn demo(&mut self) {
        self.set(View::Recording);
        self.demo = DEMO_FRAMES;
    }

    /// What to show when nothing is happening; `None` (paused) shows nothing at all.
    /// Applies immediately unless a recording or preview is in progress.
    pub fn set_idle(&mut self, idle: Option<Idle>) {
        self.idle = idle.map_or(View::Hidden, View::Idle);
        if matches!(self.view, View::Hidden | View::Idle(_)) {
            self.set(self.idle);
        }
    }

    pub fn back_to_idle(&mut self) {
        self.set(self.idle);
    }

    pub fn set(&mut self, view: View) {
        let was = self.view;
        self.view = view;
        self.tick = 0;
        self.demo = 0;
        unsafe {
            if view == View::Hidden {
                ShowWindow(self.hwnd, SW_HIDE);
                return;
            }
            if view == View::Recording && was != View::Locked {
                self.pill.viz.reset(); // a fresh recording starts calm
            }
            self.render(0.0);
            if was == View::Hidden {
                self.place();
                ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            }
        }
    }

    /// Called by the animation timer. Returns false when nothing animates (timer can stop).
    pub fn tick(&mut self, level: f32) -> bool {
        match self.view {
            View::Recording | View::Locked | View::Transcribing => {
                self.tick += 1;
                let level = if self.demo > 0 {
                    self.demo -= 1;
                    if self.demo == 0 {
                        self.back_to_idle();
                        return false;
                    }
                    demo_level(self.tick as f32 / FPS)
                } else {
                    level
                };
                self.render(level);
                true
            }
            View::Flash if self.tick >= FPS as u32 => {
                self.back_to_idle();
                false
            }
            View::Flash => {
                self.tick += 1;
                true
            }
            View::Hidden | View::Idle(_) => false,
        }
    }

    /// Bottom-center of the primary monitor's work area, just above the taskbar.
    pub fn place(&self) {
        unsafe {
            let mut r: RECT = core::mem::zeroed();
            SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut r as *mut RECT as *mut core::ffi::c_void, 0);
            let x = (r.left + r.right - self.pill.w) / 2;
            let y = r.bottom - self.pill.h - (6.0 * self.s) as i32;
            SetWindowPos(self.hwnd, HWND_TOPMOST, x, y, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }

    fn render(&mut self, level: f32) {
        if self.view == View::Hidden {
            return;
        }
        self.pill.paint(self.view, level, self.tick);
        self.present();
    }

    fn present(&self) {
        let data = self.pill.pixmap.data();
        // SAFETY: `bits` is our w*h*4 DIB, alive as long as self.
        let dst = unsafe { core::slice::from_raw_parts_mut(self.bits, data.len()) };
        for (d, s) in dst.chunks_exact_mut(4).zip(data.chunks_exact(4)) {
            d.copy_from_slice(&[s[2], s[1], s[0], s[3]]); // premultiplied RGBA -> BGRA
        }
        unsafe {
            let size = SIZE { cx: self.pill.w, cy: self.pill.h };
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
