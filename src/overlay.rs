//! Bottom-center indicator, click-through, never takes focus.
//! Idle: a tiny dash just above the taskbar meaning "Flowe is listening". Recording: a pill with a
//! state indicator on the left (red dot = recording, amber padlock = locked) and the chosen
//! visualization (see viz.rs) on the right. Static states are drawn once and cost nothing; only
//! the animated ones run a timer.

use tiny_skia::{Mask, Pixmap, Transform};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::GetDpiForSystem;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::draw::{capsule, circle, fill, rect};
use crate::viz::{Area, Style, Viz};
use crate::win::wide;

/// Animation timer rate (see TIMER_ANIM in main.rs).
pub const FPS: f32 = 60.0;
/// Length of the preview shown when a style is picked in the dashboard.
const DEMO_FRAMES: u32 = (FPS * 2.5) as u32;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Idle {
    Loading,
    Ready,
    Error,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    /// Paused: nothing on screen.
    Hidden,
    /// The small dash.
    Idle(Idle),
    Recording,
    Locked,
    Transcribing,
    /// Nothing was heard: the dash flashes amber, then returns to idle.
    Flash,
}

const BG: [u8; 4] = [20, 20, 23, 245];
const WELL: [u8; 4] = [10, 10, 12, 255];
const FG: [u8; 4] = [250, 250, 250, 255];
const DIM: [u8; 4] = [113, 113, 122, 255];
const RED: [u8; 4] = [239, 68, 68, 255];
const AMBER: [u8; 4] = [245, 158, 11, 255];

pub struct Overlay {
    hwnd: HWND,
    w: i32,
    h: i32,
    s: f32,
    pixmap: Pixmap,
    /// The inset the visualization draws into, and its rounded clip.
    area: Area,
    mask: Mask,
    memdc: HDC,
    dib: HBITMAP,
    bits: *mut u8,
    pub view: View,
    /// What to fall back to when a recording ends.
    idle: View,
    tick: u32,
    viz: Viz,
    /// Frames left in a dashboard preview (0 = real recording or not animating).
    demo: u32,
}

impl Overlay {
    pub fn new(style: Style) -> Option<Self> {
        unsafe {
            let s = GetDpiForSystem() as f32 / 96.0;
            let (w, h) = ((220.0 * s) as i32, (44.0 * s) as i32);
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
            // Left: a square slot for the state indicator. Right: the visualization inset.
            let (wf, hf) = (w as f32, h as f32);
            let pad = 7.0 * s;
            let ax = hf * 0.92;
            let area = Area { x: ax, y: pad, w: wf - ax - pad, h: hf - 2.0 * pad };
            let mut mask = Mask::new(w as u32, h as u32)?;
            mask.fill_path(&capsule(area.x, area.y, area.w, area.h), tiny_skia::FillRule::Winding, true, Transform::identity());
            Some(Self {
                hwnd,
                w,
                h,
                s,
                pixmap: Pixmap::new(w as u32, h as u32)?,
                area,
                mask,
                memdc,
                dib,
                bits: bits as *mut u8,
                view: View::Hidden,
                idle: View::Hidden,
                tick: 0,
                viz: Viz::new(style),
                demo: 0,
            })
        }
    }

    pub fn set_style(&mut self, style: Style) {
        self.viz.style = style;
        self.viz.reset();
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
                self.viz.reset(); // a fresh recording starts calm
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
            let x = (r.left + r.right - self.w) / 2;
            let y = r.bottom - self.h - (6.0 * self.s) as i32;
            SetWindowPos(self.hwnd, HWND_TOPMOST, x, y, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }

    fn render(&mut self, level: f32) {
        let (w, h, s) = (self.w as f32, self.h as f32, self.s);
        self.pixmap.fill(tiny_skia::Color::TRANSPARENT);
        let px = &mut self.pixmap;

        match self.view {
            View::Hidden => return,
            View::Idle(_) | View::Flash => {
                let color = match self.view {
                    View::Flash => AMBER,
                    View::Idle(Idle::Ready) => [250, 250, 250, 150],
                    View::Idle(Idle::Loading) => [113, 113, 122, 150],
                    _ => [245, 158, 11, 200], // Idle(Error)
                };
                fill(px, capsule(w / 2.0 - 18.0 * s, h - 5.0 * s, 36.0 * s, 5.0 * s), color);
            }
            View::Recording | View::Locked => {
                if self.view == View::Recording || self.view == View::Locked {
                    self.viz.update(level, 1.0 / FPS);
                }
                fill(px, capsule(0.0, 0.0, w, h), BG);
                let a = self.area;
                fill(px, capsule(a.x, a.y, a.w, a.h), WELL);
                self.viz.render(px, &self.mask, a, s);
                // State indicator, centred in the left slot.
                let (cx, cy) = (a.x / 2.0 + 1.0 * s, h / 2.0);
                if self.view == View::Recording {
                    let halo = 5.0 * s + 5.0 * s * self.viz.energy;
                    fill(px, circle(cx, cy, halo), [239, 68, 68, (40.0 + 60.0 * self.viz.energy) as u8]);
                    fill(px, circle(cx, cy, 5.0 * s), RED);
                } else {
                    let hole = [20, 20, 23, 255];
                    fill(px, capsule(cx - 3.6 * s, cy - 8.0 * s, 7.2 * s, 10.0 * s), AMBER); // shackle
                    fill(px, capsule(cx - 2.0 * s, cy - 6.4 * s, 4.0 * s, 8.0 * s), hole);
                    fill(px, rect(cx - 5.0 * s, cy - 2.5 * s, 10.0 * s, 8.0 * s), AMBER); // body
                }
            }
            View::Transcribing => {
                let (pw, ph) = (128.0 * s, 34.0 * s);
                let (x0, y0) = ((w - pw) / 2.0, (h - ph) / 2.0);
                fill(px, capsule(x0, y0, pw, ph), BG);
                for i in 0..3 {
                    let on = ((self.tick / 12) as i32 - i).rem_euclid(3) == 0;
                    fill(px, circle(w / 2.0 + (i - 1) as f32 * 14.0 * s, h / 2.0, 3.5 * s), if on { FG } else { DIM });
                }
            }
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

/// Speech-like loudness for previews: bursts of syllables with pauses between phrases.
pub fn demo_level(t: f32) -> f32 {
    let syllables = (t * 9.0).sin().abs();
    let phrase = (0.5 + 0.5 * (t * 1.7).sin()).powi(2);
    0.005 + 0.09 * syllables * phrase
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
