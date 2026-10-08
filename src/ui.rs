//! The dashboard window, fully owner-drawn (dark): status, stat cards, recording history.
//! Shapes are rasterized with tiny-skia, text is drawn by GDI on top, then blitted
//! double-buffered. Shown on launch; closing it only hides it.

use tiny_skia::Pixmap;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Dwm::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::GetDpiForSystem;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;
use windows_sys::core::PCWSTR;

use crate::draw::{capsule, circle, fill, rect, rounded_rect};
use crate::history::{Entry, History, fmt_duration};
use crate::tray::Status;
use crate::viz::Style;
use crate::win::wide;

pub const TIMER_UI: usize = 3;
const WM_MOUSELEAVE: u32 = 0x02A3;
const MAX_ROWS: usize = 1000;

// Surfaces (RGBA).
const BG: [u8; 4] = [20, 20, 22, 255];
const CARD: [u8; 4] = [28, 28, 32, 255];
const BORDER: [u8; 4] = [42, 42, 48, 255];
const DIVIDER: [u8; 4] = [34, 34, 39, 255];
const ROW_HOVER: [u8; 4] = [38, 38, 44, 255];
const ROW_SELECTED: [u8; 4] = [48, 48, 56, 255];
const BTN: [u8; 4] = [44, 44, 51, 255];
const BTN_HOVER: [u8; 4] = [58, 58, 66, 255];
const THUMB: [u8; 4] = [74, 74, 84, 255];
const SELECTED: [u8; 4] = [236, 236, 240, 255];
const GREEN: [u8; 4] = [34, 197, 94, 255];
const RED: [u8; 4] = [239, 68, 68, 255];
const AMBER: [u8; 4] = [245, 158, 11, 255];
const GREY: [u8; 4] = [113, 113, 122, 255];
// Text (COLORREF is 0x00BBGGRR).
const T_WHITE: u32 = 0x00F0_EDED;
const T_GREY: u32 = 0x0093_8B8B;
const T_DIM: u32 = 0x006A_6262;
const T_AMBER: u32 = 0x000B_9EF5;
const T_GREEN: u32 = 0x005E_C522;
const T_INK: u32 = 0x0018_1414; // dark text on the selected segment
const CAPTION: u32 = 0x0016_1414; // BG as COLORREF

const F_BODY: usize = 0;
const F_SMALL: usize = 1;
const F_TITLE: usize = 2;
const F_STATUS: usize = 3;
const F_BIG: usize = 4;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Hit {
    None,
    Pause,
    Clear,
    Row(usize),
    Thumb,
    Style(usize),
    Sounds,
}

pub enum UiEvent {
    None,
    TogglePause,
    ClearHistory,
    CopyRow,
    SetStyle(Style),
    ToggleSounds,
}

#[derive(Clone, Copy, Default)]
struct R {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

impl R {
    fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && py >= self.y && px < self.x + self.w && py < self.y + self.h
    }
    fn rect(&self) -> RECT {
        RECT { left: self.x, top: self.y, right: self.x + self.w, bottom: self.y + self.h }
    }
    fn inset(&self, d: i32) -> R {
        R { x: self.x + d, y: self.y + d, w: self.w - 2 * d, h: self.h - 2 * d }
    }
}

struct Layout {
    dot: (f32, f32),
    status: R,
    pause: R,
    cards: [R; 4],
    today: R,
    prefs_label: R,
    styles: [R; 4],
    sounds_label: R,
    sounds: R,
    hist: R,
    title: R,
    notice: R,
    clear: R,
    cols_hdr: R,
    list: R,
    row_h: i32,
    col_x: [i32; 4],
    col_w: [i32; 4],
    max_scroll: i32,
    thumb: Option<R>,
}

pub struct Ui {
    pub hwnd: HWND,
    s: f32,
    w: i32,
    h: i32,
    pixmap: Option<Pixmap>,
    memdc: HDC,
    dib: HBITMAP,
    bits: *mut u8,
    fonts: [HFONT; 5],
    status: Status,
    status_text: String,
    button_label: String,
    stats: [String; 4],
    today: String,
    rows: Vec<Entry>,
    selected: Option<usize>,
    hover: Hit,
    tracking: bool,
    scroll: i32,
    drag: Option<(i32, i32)>, // (mouse y at grab, scroll at grab)
    notice: Option<String>,
    clear_armed: bool,
    clear_w: i32,
    style: Style,
    sounds: bool,
}

fn px(s: f32, v: i32) -> i32 {
    (v as f32 * s).round() as i32
}

unsafe fn font(s: f32, pt: f32, weight: u32) -> HFONT {
    unsafe {
        CreateFontW(
            -((pt * 96.0 / 72.0 * s).round() as i32),
            0,
            0,
            0,
            weight as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_DEFAULT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            CLEARTYPE_QUALITY as u32,
            (DEFAULT_PITCH | FF_DONTCARE) as u32,
            wide("Segoe UI").as_ptr(),
        )
    }
}

impl Ui {
    pub fn new(wndproc: WNDPROC) -> Option<Self> {
        unsafe {
            let hinst = GetModuleHandleW(core::ptr::null());
            let s = GetDpiForSystem() as f32 / 96.0;
            let class = wide("flowe-main");
            // Icon resource 1 is embedded by build.rs; null just means the default icon.
            let icon = LoadImageW(hinst, 1 as PCWSTR, IMAGE_ICON, 0, 0, LR_DEFAULTSIZE);
            let icon_sm = LoadImageW(hinst, 1 as PCWSTR, IMAGE_ICON, px(s, 16), px(s, 16), 0);
            let wc = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                style: CS_DBLCLKS,
                lpfnWndProc: wndproc,
                hInstance: hinst,
                hIcon: icon,
                hIconSm: icon_sm,
                hCursor: LoadCursorW(core::ptr::null_mut(), IDC_ARROW),
                hbrBackground: CreateSolidBrush(CAPTION), // no white flash before the first paint
                lpszClassName: class.as_ptr(),
                ..core::mem::zeroed()
            };
            RegisterClassExW(&wc);
            let hwnd = CreateWindowExW(
                0,
                class.as_ptr(),
                wide("Flowe").as_ptr(),
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                px(s, 760),
                px(s, 600),
                core::ptr::null_mut(),
                core::ptr::null_mut(),
                hinst,
                core::ptr::null(),
            );
            if hwnd.is_null() {
                return None;
            }
            // Dark title bar and frame (Windows 11; harmlessly ignored elsewhere).
            let on: i32 = 1;
            DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE as u32, &on as *const i32 as *const _, 4);
            DwmSetWindowAttribute(hwnd, DWMWA_CAPTION_COLOR as u32, &CAPTION as *const u32 as *const _, 4);
            let border: u32 = 0x0030_2A2A;
            DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR as u32, &border as *const u32 as *const _, 4);

            let fonts = [
                font(s, 9.5, FW_NORMAL),
                font(s, 8.0, FW_NORMAL),
                font(s, 10.5, FW_SEMIBOLD),
                font(s, 12.0, FW_SEMIBOLD),
                font(s, 22.0, FW_SEMIBOLD),
            ];
            Some(Self {
                hwnd,
                s,
                w: 0,
                h: 0,
                pixmap: None,
                memdc: CreateCompatibleDC(core::ptr::null_mut()),
                dib: core::ptr::null_mut(),
                bits: core::ptr::null_mut(),
                fonts,
                status: Status::Loading,
                status_text: String::new(),
                button_label: "Pause".into(),
                stats: ["–".into(), "–".into(), "–".into(), "–".into()],
                today: String::new(),
                rows: Vec::new(),
                selected: None,
                hover: Hit::None,
                tracking: false,
                scroll: 0,
                drag: None,
                notice: None,
                clear_armed: false,
                clear_w: px(s, 100),
                style: Style::Waves,
                sounds: true,
            })
        }
    }

    // ----- state setters (each schedules a repaint) -----

    pub fn set_status(&mut self, status: Status, text: &str, button: &str) {
        self.status = status;
        self.status_text = text.into();
        self.button_label = button.into();
        self.repaint();
    }

    pub fn set_prefs(&mut self, style: Style, sounds: bool) {
        self.style = style;
        self.sounds = sounds;
        self.repaint();
    }

    pub fn set_notice(&mut self, notice: Option<&str>) {
        self.notice = notice.map(Into::into);
        self.repaint();
    }

    pub fn set_stats(&mut self, h: &History) {
        let st = h.stats();
        self.stats = [
            thousands(st.recordings),
            thousands(st.words),
            fmt_duration(st.audio_ms),
            if st.audio_ms > 0 { format!("{:.0}", st.wpm()) } else { "–".into() },
        ];
        self.today = if st.recordings == 0 {
            String::new()
        } else {
            format!(
                "Today: {} recordings, {} words   ·   transcription takes {:.2}s on average, {:.0}× faster than realtime",
                st.today_recordings,
                thousands(st.today_words),
                st.avg_latency_s(),
                st.speed()
            )
        };
        self.repaint();
    }

    /// Refill the list, newest first.
    pub fn rebuild(&mut self, h: &History) {
        self.rows = h.entries.iter().rev().take(MAX_ROWS).cloned().collect();
        self.selected = None;
        self.scroll = 0;
        self.set_stats(h);
    }

    pub fn prepend(&mut self, e: &Entry) {
        self.rows.insert(0, e.clone());
        self.rows.truncate(MAX_ROWS);
        self.selected = self.selected.map(|i| i + 1).filter(|&i| i < self.rows.len());
        self.repaint();
    }

    pub fn selected_text(&self) -> Option<String> {
        self.selected.and_then(|i| self.rows.get(i)).map(|e| e.text.clone())
    }

    pub fn min_size(&self) -> (i32, i32) {
        (px(self.s, 640), px(self.s, 480))
    }

    pub fn show(&self) {
        unsafe {
            ShowWindow(self.hwnd, if IsIconic(self.hwnd) != 0 { SW_RESTORE } else { SW_SHOW });
            SetForegroundWindow(self.hwnd);
        }
    }

    pub fn hand_cursor(&self) -> bool {
        matches!(self.hover, Hit::Pause | Hit::Clear | Hit::Row(_) | Hit::Style(_) | Hit::Sounds)
    }

    /// TIMER_UI: the "confirm clear?" state expires.
    pub fn tick(&mut self) {
        unsafe { KillTimer(self.hwnd, TIMER_UI) };
        self.clear_armed = false;
        self.repaint();
    }

    pub fn on_size(&mut self) {
        self.repaint();
    }

    fn repaint(&self) {
        unsafe { InvalidateRect(self.hwnd, core::ptr::null(), 0) };
    }

    // ----- layout -----

    fn layout(&self) -> Layout {
        let s = self.s;
        let p = |v: i32| px(s, v);
        let (w, h) = (self.w, self.h);
        let m = p(20);
        let pause = R { x: w - m - p(104), y: p(16), w: p(104), h: p(32) };
        let status = R { x: m + p(20), y: p(16), w: (pause.x - m - p(20) - p(12)).max(10), h: p(32) };
        let gap = p(12);
        let cw = (w - 2 * m - 3 * gap) / 4;
        let cards = [0, 1, 2, 3].map(|i| R { x: m + i * (cw + gap), y: p(62), w: cw, h: p(78) });
        let today = R { x: m, y: p(152), w: w - 2 * m, h: p(18) };
        // Preferences row: indicator style (segmented) on the left, sounds switch on the right.
        let row_y = p(182);
        let prefs_label = R { x: m, y: row_y, w: p(70), h: p(30) };
        let (seg_w, seg_gap) = (p(86), p(6));
        let styles = [0, 1, 2, 3].map(|i| R { x: m + p(74) + i * (seg_w + seg_gap), y: row_y, w: seg_w, h: p(30) });
        let sounds = R { x: w - m - p(40), y: row_y + p(5), w: p(40), h: p(20) };
        let sounds_label = R { x: sounds.x - p(70), y: row_y, w: p(62), h: p(30) };
        let hist = R { x: m, y: p(226), w: w - 2 * m, h: (h - p(226) - m).max(p(120)) };
        let title = R { x: hist.x + p(14), y: hist.y + p(10), w: p(70), h: p(22) };
        let clear = R { x: hist.x + hist.w - p(14) - self.clear_w, y: hist.y + p(10), w: self.clear_w, h: p(22) };
        let notice = R { x: title.x + title.w, y: title.y, w: (clear.x - title.x - title.w - p(12)).max(10), h: title.h };
        let cols_hdr = R { x: hist.x, y: hist.y + p(42), w: hist.w, h: p(18) };
        let list = R { x: hist.x + 1, y: hist.y + p(62), w: hist.w - 2, h: (hist.h - p(62) - p(6)).max(0) };
        let row_h = p(30);
        let col_x = [hist.x + p(14), hist.x + p(72), hist.x + p(130), hist.x + p(190)];
        let col_w = [p(54), p(52), p(48), (hist.w - p(190) - p(14) - p(10)).max(10)];
        let content = self.rows.len() as i32 * row_h;
        let max_scroll = (content - list.h).max(0);
        let thumb = (max_scroll > 0).then(|| {
            let th = (list.h * list.h / content.max(1)).max(p(24)).min(list.h);
            let ty = list.y + (list.h - th) * self.scroll / max_scroll;
            R { x: list.x + list.w - p(8), y: ty, w: p(4), h: th }
        });
        Layout {
            dot: ((m + p(6)) as f32, (p(16) + p(16)) as f32),
            status,
            pause,
            cards,
            today,
            prefs_label,
            styles,
            sounds_label,
            sounds,
            hist,
            title,
            notice,
            clear,
            cols_hdr,
            list,
            row_h,
            col_x,
            col_w,
            max_scroll,
            thumb,
        }
    }

    fn hit(&self, x: i32, y: i32) -> Hit {
        let l = self.layout();
        if l.pause.contains(x, y) {
            Hit::Pause
        } else if l.clear.contains(x, y) {
            Hit::Clear
        } else if let Some(i) = l.styles.iter().position(|r| r.contains(x, y)) {
            Hit::Style(i)
        } else if l.sounds.contains(x, y) || l.sounds_label.contains(x, y) {
            Hit::Sounds
        } else if l.thumb.is_some_and(|t| t.contains(x, y)) {
            Hit::Thumb
        } else if l.list.contains(x, y) {
            let i = ((y - l.list.y + self.scroll) / l.row_h.max(1)) as usize;
            if i < self.rows.len() { Hit::Row(i) } else { Hit::None }
        } else {
            Hit::None
        }
    }

    fn set_scroll(&mut self, v: i32) {
        let max = self.layout().max_scroll;
        let v = v.clamp(0, max);
        if v != self.scroll {
            self.scroll = v;
            self.repaint();
        }
    }

    // ----- input -----

    pub fn on_mouse(&mut self, msg: u32, wparam: WPARAM, lparam: LPARAM) -> UiEvent {
        let x = (lparam & 0xFFFF) as i16 as i32;
        let y = ((lparam >> 16) & 0xFFFF) as i16 as i32;
        match msg {
            WM_MOUSEMOVE => {
                if !self.tracking {
                    let mut tme = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: self.hwnd, dwHoverTime: 0 };
                    unsafe { TrackMouseEvent(&mut tme) };
                    self.tracking = true;
                }
                if let Some((y0, s0)) = self.drag {
                    let l = self.layout();
                    if let Some(t) = l.thumb {
                        let track = (l.list.h - t.h).max(1);
                        self.set_scroll(s0 + (y - y0) * l.max_scroll / track);
                    }
                } else {
                    let h = self.hit(x, y);
                    if h != self.hover {
                        self.hover = h;
                        self.repaint();
                    }
                }
            }
            WM_MOUSELEAVE => {
                self.tracking = false;
                if self.hover != Hit::None {
                    self.hover = Hit::None;
                    self.repaint();
                }
            }
            WM_LBUTTONDOWN => match self.hit(x, y) {
                Hit::Pause => return UiEvent::TogglePause,
                Hit::Style(i) => return UiEvent::SetStyle(Style::ALL[i]),
                Hit::Sounds => return UiEvent::ToggleSounds,
                Hit::Clear => {
                    if self.clear_armed {
                        self.tick();
                        return UiEvent::ClearHistory;
                    }
                    self.clear_armed = true;
                    unsafe { SetTimer(self.hwnd, TIMER_UI, 3000, None) };
                    self.repaint();
                }
                Hit::Row(i) => {
                    self.selected = Some(i);
                    self.repaint();
                }
                Hit::Thumb => {
                    self.drag = Some((y, self.scroll));
                    unsafe { SetCapture(self.hwnd) };
                }
                Hit::None => {}
            },
            WM_LBUTTONUP => {
                if self.drag.take().is_some() {
                    unsafe { ReleaseCapture() };
                }
            }
            WM_LBUTTONDBLCLK => {
                if let Hit::Row(i) = self.hit(x, y) {
                    self.selected = Some(i);
                    self.repaint();
                    return UiEvent::CopyRow;
                }
            }
            WM_MOUSEWHEEL => {
                let delta = ((wparam >> 16) & 0xFFFF) as i16 as i32;
                let row_h = self.layout().row_h;
                self.set_scroll(self.scroll - delta * 3 * row_h / 120);
            }
            _ => {}
        }
        UiEvent::None
    }

    // ----- painting -----

    pub fn paint(&mut self) {
        unsafe {
            let mut ps: PAINTSTRUCT = core::mem::zeroed();
            let hdc = BeginPaint(self.hwnd, &mut ps);
            if self.ensure_buffer() {
                self.render();
                BitBlt(hdc, 0, 0, self.w, self.h, self.memdc, 0, 0, SRCCOPY);
            }
            EndPaint(self.hwnd, &ps);
        }
    }

    /// (Re)creates the back buffer to match the client size. False when there is nothing to paint.
    fn ensure_buffer(&mut self) -> bool {
        unsafe {
            let mut r: RECT = core::mem::zeroed();
            GetClientRect(self.hwnd, &mut r);
            let (w, h) = (r.right - r.left, r.bottom - r.top);
            if w <= 0 || h <= 0 {
                return false;
            }
            if w == self.w && h == self.h && !self.dib.is_null() {
                return true;
            }
            let mut bmi: BITMAPINFO = core::mem::zeroed();
            bmi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
            bmi.bmiHeader.biWidth = w;
            bmi.bmiHeader.biHeight = -h;
            bmi.bmiHeader.biPlanes = 1;
            bmi.bmiHeader.biBitCount = 32;
            let mut bits = core::ptr::null_mut();
            let dib = CreateDIBSection(self.memdc, &bmi, DIB_RGB_COLORS, &mut bits, core::ptr::null_mut(), 0);
            let Some(pixmap) = Pixmap::new(w as u32, h as u32) else { return false };
            if dib.is_null() || bits.is_null() {
                return false;
            }
            SelectObject(self.memdc, dib);
            if !self.dib.is_null() {
                DeleteObject(self.dib);
            }
            self.dib = dib;
            self.bits = bits as *mut u8;
            self.pixmap = Some(pixmap);
            self.w = w;
            self.h = h;
            true
        }
    }

    fn render(&mut self) {
        let l = self.layout();
        let s = self.s;
        let p = |v: i32| px(s, v) as f32;
        let Some(px_) = self.pixmap.as_mut() else { return };
        let pxm = px_;

        // --- shapes ---
        pxm.fill(tiny_skia::Color::from_rgba8(BG[0], BG[1], BG[2], 255));
        let dot = match self.status {
            Status::Ready => GREEN,
            Status::Recording | Status::Error => RED,
            Status::Paused => AMBER,
            Status::Loading => GREY,
        };
        fill(pxm, circle(l.dot.0, l.dot.1, p(5)), dot);
        let (bx, by, bw, bh) = rf(l.pause);
        fill(pxm, capsule(bx, by, bw, bh), if self.hover == Hit::Pause { BTN_HOVER } else { BTN });
        for c in l.cards {
            card(pxm, c, p(10));
        }
        for (i, r) in l.styles.iter().enumerate() {
            let (x, y, w, h) = rf(*r);
            let color = if Style::ALL[i] == self.style {
                SELECTED
            } else if self.hover == Hit::Style(i) {
                BTN_HOVER
            } else {
                BTN
            };
            fill(pxm, capsule(x, y, w, h), color);
        }
        let (sx, sy, sw, sh) = rf(l.sounds);
        fill(pxm, capsule(sx, sy, sw, sh), if self.sounds { GREEN } else if self.hover == Hit::Sounds { BTN_HOVER } else { BTN });
        let knob_x = if self.sounds { sx + sw - sh / 2.0 } else { sx + sh / 2.0 };
        fill(pxm, circle(knob_x, sy + sh / 2.0, sh / 2.0 - p(3)), [250, 250, 250, 255]);
        card(pxm, l.hist, p(10));
        let (lx, ly, lw, lh) = rf(l.list);
        if l.list.h > 0 {
            let first = (self.scroll / l.row_h.max(1)) as usize;
            for i in first..self.rows.len() {
                let y = l.list.y + i as i32 * l.row_h - self.scroll;
                if y >= l.list.y + l.list.h {
                    break;
                }
                let bg = if self.selected == Some(i) {
                    Some(ROW_SELECTED)
                } else if self.hover == Hit::Row(i) {
                    Some(ROW_HOVER)
                } else {
                    None
                };
                // Clamp to the viewport so partially visible rows don't spill out of the card.
                let top = y.max(l.list.y);
                let bottom = (y + l.row_h).min(l.list.y + l.list.h);
                if let Some(bg) = bg
                    && bottom > top
                {
                    fill(pxm, rounded_rect(lx + p(6), top as f32, lw - p(12) - p(10), (bottom - top) as f32, p(6)), bg);
                }
                let div = y + l.row_h - 1;
                if div >= l.list.y && div < l.list.y + l.list.h && i + 1 < self.rows.len() {
                    fill(pxm, rect(lx + p(14), div as f32, lw - p(28) - p(10), 1.0), DIVIDER);
                }
            }
        }
        if let Some(t) = l.thumb {
            let (tx, ty, tw, th) = rf(t);
            fill(pxm, capsule(tx, ty, tw, th), THUMB);
        }
        let _ = (ly, lh);

        // --- copy to the DIB (opaque, so premultiplied == straight) ---
        let data = pxm.data();
        // SAFETY: `bits` is our w*h*4 DIB, alive while self is.
        let dst = unsafe { core::slice::from_raw_parts_mut(self.bits, data.len()) };
        for (d, s) in dst.chunks_exact_mut(4).zip(data.chunks_exact(4)) {
            d.copy_from_slice(&[s[2], s[1], s[0], 255]);
        }

        // --- text ---
        unsafe {
            let hdc = self.memdc;
            SetBkMode(hdc, TRANSPARENT as i32);
            let f = self.fonts;
            const L: u32 = DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS;
            text(hdc, f[F_STATUS], T_WHITE, &self.status_text, l.status, L);
            text(hdc, f[F_BODY], T_WHITE, &self.button_label, l.pause, L | DT_CENTER);
            for (i, (c, label)) in l.cards.iter().zip(["recordings", "words dictated", "of speech", "words / min"]).enumerate() {
                let inner = c.inset(px(s, 14));
                text(hdc, f[F_BIG], T_WHITE, &self.stats[i], R { h: px(s, 40), ..inner }, DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS);
                text(hdc, f[F_SMALL], T_GREY, label, R { y: inner.y + px(s, 40), h: px(s, 16), ..inner }, L);
            }
            text(hdc, f[F_BODY], T_GREY, &self.today, l.today, L);
            text(hdc, f[F_BODY], T_GREY, "Indicator", l.prefs_label, L);
            for (i, r) in l.styles.iter().enumerate() {
                let color = if Style::ALL[i] == self.style { T_INK } else { T_WHITE };
                text(hdc, f[F_BODY], color, Style::ALL[i].label(), *r, L | DT_CENTER);
            }
            text(hdc, f[F_BODY], T_GREY, "Sounds", l.sounds_label, L | DT_RIGHT);
            text(hdc, f[F_TITLE], T_WHITE, "History", l.title, L);
            match &self.notice {
                Some(n) => text(hdc, f[F_BODY], T_GREEN, n, l.notice, L),
                None => text(hdc, f[F_BODY], T_DIM, "double-click a row to copy it again", l.notice, L),
            }
            let (clear_label, clear_color) = match (self.clear_armed, self.hover == Hit::Clear) {
                (true, _) => ("Click again to clear everything", T_AMBER),
                (false, true) => ("Clear history", T_WHITE),
                (false, false) => ("Clear history", T_GREY),
            };
            self.clear_w = measure(hdc, f[F_BODY], clear_label) + px(s, 4);
            let clear = R { x: l.hist.x + l.hist.w - px(s, 14) - self.clear_w, ..l.clear };
            text(hdc, f[F_BODY], clear_color, clear_label, R { w: self.clear_w, ..clear }, L | DT_RIGHT);
            for (i, h) in ["TIME", "LENGTH", "WORDS", "TEXT"].iter().enumerate() {
                let right = if (1..=2).contains(&i) { DT_RIGHT } else { 0 };
                text(hdc, f[F_SMALL], T_DIM, h, R { x: l.col_x[i], y: l.cols_hdr.y, w: l.col_w[i], h: l.cols_hdr.h }, L | right);
            }
            if self.rows.is_empty() {
                text(hdc, f[F_BODY], T_GREY, "No recordings yet. Hold Ctrl+Win and say something.", l.list, L | DT_CENTER);
            } else if l.list.h > 0 {
                let rgn = CreateRectRgn(l.list.x, l.list.y, l.list.x + l.list.w, l.list.y + l.list.h);
                SelectClipRgn(hdc, rgn);
                let first = (self.scroll / l.row_h.max(1)) as usize;
                for (i, e) in self.rows.iter().enumerate().skip(first) {
                    let y = l.list.y + i as i32 * l.row_h - self.scroll;
                    if y >= l.list.y + l.list.h {
                        break;
                    }
                    let cell = |c: usize| R { x: l.col_x[c], y, w: l.col_w[c], h: l.row_h };
                    text(hdc, f[F_BODY], T_GREY, &fmt_when(&e.when), cell(0), L);
                    text(hdc, f[F_BODY], T_GREY, &fmt_duration(e.audio_ms as u64), cell(1), L | DT_RIGHT);
                    text(hdc, f[F_BODY], T_GREY, &e.words.to_string(), cell(2), L | DT_RIGHT);
                    text(hdc, f[F_BODY], T_WHITE, &e.text, cell(3), L);
                }
                SelectClipRgn(hdc, core::ptr::null_mut());
                DeleteObject(rgn);
            }
        }
    }
}

fn rf(r: R) -> (f32, f32, f32, f32) {
    (r.x as f32, r.y as f32, r.w as f32, r.h as f32)
}

/// Rounded card: 1px border around a filled surface.
fn card(pxm: &mut Pixmap, r: R, radius: f32) {
    let (x, y, w, h) = rf(r);
    fill(pxm, rounded_rect(x, y, w, h, radius), BORDER);
    fill(pxm, rounded_rect(x + 1.0, y + 1.0, w - 2.0, h - 2.0, radius - 1.0), CARD);
}

unsafe fn text(hdc: HDC, font: HFONT, color: u32, s: &str, r: R, flags: u32) {
    if r.w <= 0 || r.h <= 0 || s.is_empty() {
        return;
    }
    unsafe {
        SelectObject(hdc, font);
        SetTextColor(hdc, color);
        let mut t: Vec<u16> = s.encode_utf16().collect();
        let mut rc = r.rect();
        DrawTextW(hdc, t.as_mut_ptr(), t.len() as i32, &mut rc, flags);
    }
}

unsafe fn measure(hdc: HDC, font: HFONT, s: &str) -> i32 {
    unsafe {
        SelectObject(hdc, font);
        let mut t: Vec<u16> = s.encode_utf16().collect();
        let mut rc = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        DrawTextW(hdc, t.as_mut_ptr(), t.len() as i32, &mut rc, DT_SINGLELINE | DT_NOPREFIX | DT_CALCRECT);
        rc.right - rc.left
    }
}

impl Drop for Ui {
    fn drop(&mut self) {
        unsafe {
            for f in self.fonts {
                DeleteObject(f);
            }
            if !self.dib.is_null() {
                DeleteObject(self.dib);
            }
            DeleteDC(self.memdc);
        }
    }
}

fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// "2026-10-07 15:40:12" -> "15:40" today, "10-07 15:40" otherwise.
fn fmt_when(when: &str) -> String {
    if when.len() < 16 {
        return when.to_string();
    }
    let now = crate::win::local_time();
    if when.starts_with(&now[..10]) { when[11..16].to_string() } else { when[5..16].to_string() }
}

#[cfg(test)]
mod tests {
    #[test]
    fn thousands() {
        assert_eq!(super::thousands(0), "0");
        assert_eq!(super::thousands(999), "999");
        assert_eq!(super::thousands(1234567), "1,234,567");
    }
}
