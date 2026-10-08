//! Bottom-center indicator, click-through, never takes focus.
//! Idle: a tiny dash just above the taskbar meaning "Flowe is listening". Recording: a pill
//! filled with liquid that rises and sloshes with your voice. Static states are drawn once and
//! cost nothing; only the animated ones run a timer.

use tiny_skia::{Mask, Path, PathBuilder, Pixmap, Transform};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::GetDpiForSystem;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::draw::{capsule, circle, fill, fill_masked, rect};
use crate::win::wide;

/// Animation timer rate (see TIMER_ANIM in main.rs).
const FPS: f32 = 60.0;

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
const FG: [u8; 4] = [250, 250, 250, 255];
const DIM: [u8; 4] = [113, 113, 122, 255];
const AMBER: [u8; 4] = [245, 158, 11, 255];

/// Liquid layers, back to front: (wavelength px, speed rad/s, phase, amplitude x, y offset px).
const LAYERS: [(f32, f32, f32, f32, f32); 3] = [(104.0, 2.2, 0.0, 1.0, 0.0), (76.0, -3.0, 1.7, 0.8, 2.0), (140.0, 3.6, 3.1, 0.6, 4.0)];
const RECORDING: [[u8; 4]; 3] = [[150, 20, 38, 210], [226, 52, 62, 215], [255, 104, 98, 225]];
const LOCKED: [[u8; 4]; 3] = [[150, 82, 6, 210], [232, 142, 10, 215], [252, 190, 66, 225]];

pub struct Overlay {
    hwnd: HWND,
    w: i32,
    h: i32,
    s: f32,
    pixmap: Pixmap,
    /// The full-size capsule the liquid is clipped to.
    mask: Mask,
    memdc: HDC,
    dib: HBITMAP,
    bits: *mut u8,
    pub view: View,
    /// What to fall back to when a recording ends.
    idle: View,
    tick: u32,
    /// Smoothed loudness 0..1 and animation clock (seconds) for the liquid.
    smooth: f32,
    t: f32,
}

impl Overlay {
    pub fn new() -> Option<Self> {
        unsafe {
            let s = GetDpiForSystem() as f32 / 96.0;
            let (w, h) = ((208.0 * s) as i32, (44.0 * s) as i32);
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
            let mut mask = Mask::new(w as u32, h as u32)?;
            mask.fill_path(&capsule(0.0, 0.0, w as f32, h as f32), tiny_skia::FillRule::Winding, true, Transform::identity());
            Some(Self {
                hwnd,
                w,
                h,
                s,
                pixmap: Pixmap::new(w as u32, h as u32)?,
                mask,
                memdc,
                dib,
                bits: bits as *mut u8,
                view: View::Hidden,
                idle: View::Hidden,
                tick: 0,
                smooth: 0.0,
                t: 0.0,
            })
        }
    }

    /// What to show when nothing is happening; `None` (paused) shows nothing at all.
    /// Applies immediately unless a recording is in progress.
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
        unsafe {
            if view == View::Hidden {
                ShowWindow(self.hwnd, SW_HIDE);
                return;
            }
            if view == View::Recording {
                self.smooth = 0.0; // a fresh recording starts with a calm surface
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
                fill(px, capsule(0.0, 0.0, w, h), BG);
                // Loudness: quick to rise, slow to settle, so the liquid surges and then sloshes back.
                let target = (level * 10.0).min(1.0);
                let k = if target > self.smooth { 0.35 } else { 0.06 };
                self.smooth += (target - self.smooth) * k;
                self.t += 1.0 / FPS;
                let colors = if self.view == View::Locked { LOCKED } else { RECORDING };
                let base = h * (0.74 - 0.40 * self.smooth);
                let amp = s * (1.2 + 9.0 * self.smooth);
                for (i, &(wavelength, speed, phase, amp_x, dy)) in LAYERS.iter().enumerate() {
                    let path = wave(w, h, base + dy * s, amp * amp_x, wavelength * s, speed * self.t + phase);
                    fill_masked(px, path, colors[i], Some(&self.mask));
                }
                if self.view == View::Locked {
                    let (lx, cy) = (w - h / 2.0 - 1.0 * s, h / 2.0);
                    let hole = [30, 30, 34, 255];
                    fill(px, capsule(lx - 3.6 * s, cy - 8.0 * s, 7.2 * s, 10.0 * s), FG); // shackle
                    fill(px, capsule(lx - 2.0 * s, cy - 6.4 * s, 4.0 * s, 8.0 * s), hole);
                    fill(px, rect(lx - 5.0 * s, cy - 2.5 * s, 10.0 * s, 8.0 * s), FG); // body
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

/// Liquid body: the area under a wavy surface (a main sine plus a smaller faster harmonic for
/// a less mechanical look), spanning the full width and down to the bottom.
fn wave(w: f32, h: f32, base: f32, amp: f32, wavelength: f32, phase: f32) -> Path {
    let k = std::f32::consts::TAU / wavelength;
    let y = |x: f32| base + amp * ((k * x + phase).sin() + 0.18 * (1.7 * k * x - 0.9 * phase).sin());
    let mut pb = PathBuilder::new();
    pb.move_to(0.0, y(0.0));
    let mut x = 2.0;
    while x < w {
        pb.line_to(x, y(x));
        x += 2.0;
    }
    pb.line_to(w, y(w));
    pb.line_to(w, h);
    pb.line_to(0.0, h);
    pb.close();
    pb.finish().unwrap_or_else(|| rect(0.0, 0.0, w, h))
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
