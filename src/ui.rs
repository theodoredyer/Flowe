//! The app window: pause/resume, usage stats, recording history.
//! Hidden by default (left-click the tray icon opens it); closing it only hides it.

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::SystemServices::{SS_ENDELLIPSIS, SS_LEFT};
use windows_sys::Win32::UI::Controls::*;
use windows_sys::Win32::UI::HiDpi::GetDpiForSystem;
use windows_sys::Win32::UI::WindowsAndMessaging::*;
use windows_sys::core::PCWSTR;

use crate::history::{Entry, History, fmt_duration};
use crate::win::wide;

pub const ID_PAUSE: usize = 10;
pub const ID_CLEAR: usize = 11;
pub const ID_LIST: usize = 12;
const MAX_ROWS: usize = 1000;
const TEXT_COL: usize = 3;

pub struct Ui {
    pub hwnd: HWND,
    status: HWND,
    stats: HWND,
    pause: HWND,
    hint: HWND,
    clear: HWND,
    list: HWND,
    font: HFONT,
    bold: HFONT,
    s: f32,
}

fn px(s: f32, v: i32) -> i32 {
    (v as f32 * s).round() as i32
}

unsafe fn font(s: f32, pt: i32, weight: i32) -> HFONT {
    unsafe {
        CreateFontW(
            -px(s, pt * 96 / 72),
            0,
            0,
            0,
            weight,
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
            InitCommonControlsEx(&INITCOMMONCONTROLSEX { dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32, dwICC: ICC_LISTVIEW_CLASSES });
            let hinst = GetModuleHandleW(core::ptr::null());
            let s = GetDpiForSystem() as f32 / 96.0;
            let class = wide("parakey-main");
            // Icon resource 1 is embedded by build.rs; null just means the default icon.
            let icon = LoadImageW(hinst, 1 as PCWSTR, IMAGE_ICON, 0, 0, LR_DEFAULTSIZE);
            let icon_sm = LoadImageW(hinst, 1 as PCWSTR, IMAGE_ICON, px(s, 16), px(s, 16), 0);
            let wc = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: wndproc,
                hInstance: hinst,
                hIcon: icon,
                hIconSm: icon_sm,
                hCursor: LoadCursorW(core::ptr::null_mut(), IDC_ARROW),
                hbrBackground: (COLOR_WINDOW + 1) as usize as HBRUSH,
                lpszClassName: class.as_ptr(),
                ..core::mem::zeroed()
            };
            RegisterClassExW(&wc);
            let hwnd = CreateWindowExW(
                0,
                class.as_ptr(),
                wide("parakey").as_ptr(),
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                px(s, 680),
                px(s, 480),
                core::ptr::null_mut(),
                core::ptr::null_mut(),
                hinst,
                core::ptr::null(),
            );
            if hwnd.is_null() {
                return None;
            }
            let child = |class: &str, text: &str, style: u32, id: usize| -> HWND {
                CreateWindowExW(
                    0,
                    wide(class).as_ptr(),
                    wide(text).as_ptr(),
                    WS_CHILD | WS_VISIBLE | style,
                    0,
                    0,
                    10,
                    10,
                    hwnd,
                    id as HMENU,
                    hinst,
                    core::ptr::null(),
                )
            };
            let status = child("STATIC", "", (SS_LEFT | SS_ENDELLIPSIS) as u32, 0);
            let pause = child("BUTTON", "Pause", WS_TABSTOP | BS_PUSHBUTTON as u32, ID_PAUSE);
            let stats = child("STATIC", "", SS_LEFT as u32, 0);
            let hint = child("STATIC", "History  ·  double-click a row to copy it again", (SS_LEFT | SS_ENDELLIPSIS) as u32, 0);
            let clear = child("BUTTON", "Clear history", WS_TABSTOP | BS_PUSHBUTTON as u32, ID_CLEAR);
            let list = CreateWindowExW(
                0,
                WC_LISTVIEWW,
                core::ptr::null(),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | (LVS_REPORT | LVS_SINGLESEL | LVS_SHOWSELALWAYS | LVS_NOSORTHEADER) as u32,
                0,
                0,
                10,
                10,
                hwnd,
                ID_LIST as HMENU,
                hinst,
                core::ptr::null(),
            );
            SendMessageW(list, LVM_SETEXTENDEDLISTVIEWSTYLE, 0, (LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER) as LPARAM);
            for (i, (name, width, right)) in [("Time", 110, false), ("Length", 64, true), ("Words", 56, true), ("Text", 360, false)].iter().enumerate() {
                let name = wide(name);
                let col = LVCOLUMNW {
                    mask: LVCF_TEXT | LVCF_WIDTH | LVCF_FMT,
                    fmt: if *right { LVCFMT_RIGHT } else { LVCFMT_LEFT },
                    cx: px(s, *width),
                    pszText: name.as_ptr() as *mut u16,
                    ..core::mem::zeroed()
                };
                SendMessageW(list, LVM_INSERTCOLUMNW, i, &col as *const _ as LPARAM);
            }

            let font = font(s, 9, FW_NORMAL as i32);
            let bold = font_(s);
            for (h, f) in [(status, bold), (pause, font), (stats, font), (hint, font), (clear, font), (list, font)] {
                SendMessageW(h, WM_SETFONT, f as usize, 1);
            }
            let ui = Self { hwnd, status, stats, pause, hint, clear, list, font, bold, s };
            ui.layout();
            Some(ui)
        }
    }

    pub fn layout(&self) {
        unsafe {
            let mut r: RECT = core::mem::zeroed();
            GetClientRect(self.hwnd, &mut r);
            let (w, h, s) = (r.right, r.bottom, self.s);
            let m = px(s, 16);
            let (bw, bh) = (px(s, 104), px(s, 30));
            MoveWindow(self.status, m, px(s, 18), w - 2 * m - bw - px(s, 12), px(s, 26), 1);
            MoveWindow(self.pause, w - m - bw, px(s, 15), bw, bh, 1);
            MoveWindow(self.stats, m, px(s, 54), w - 2 * m, px(s, 40), 1);
            let cw = px(s, 104);
            MoveWindow(self.hint, m, px(s, 108), w - 2 * m - cw - px(s, 12), px(s, 20), 1);
            MoveWindow(self.clear, w - m - cw, px(s, 102), cw, px(s, 28), 1);
            let top = px(s, 138);
            MoveWindow(self.list, m, top, w - 2 * m, (h - top - m).max(px(s, 60)), 1);
            // The text column takes whatever width is left.
            let used: i32 = (0..TEXT_COL).map(|i| SendMessageW(self.list, LVM_GETCOLUMNWIDTH, i, 0) as i32).sum();
            let avail = w - 2 * m - used - GetSystemMetrics(SM_CXVSCROLL) - px(s, 4);
            SendMessageW(self.list, LVM_SETCOLUMNWIDTH, TEXT_COL, avail.max(px(s, 120)) as LPARAM);
        }
    }

    pub fn min_size(&self) -> (i32, i32) {
        (px(self.s, 520), px(self.s, 340))
    }

    pub fn show(&self) {
        unsafe {
            ShowWindow(self.hwnd, if IsIconic(self.hwnd) != 0 { SW_RESTORE } else { SW_SHOW });
            SetForegroundWindow(self.hwnd);
        }
    }

    pub fn set_status(&self, text: &str, pause_label: &str) {
        unsafe {
            SetWindowTextW(self.status, wide(text).as_ptr());
            SetWindowTextW(self.pause, wide(pause_label).as_ptr());
        }
    }

    pub fn set_stats(&self, h: &History) {
        unsafe { SetWindowTextW(self.stats, wide(&h.summary()).as_ptr()) };
    }

    /// Refill the list, newest first.
    pub fn rebuild(&self, h: &History) {
        unsafe { SendMessageW(self.list, LVM_DELETEALLITEMS, 0, 0) };
        for (i, e) in h.entries.iter().rev().take(MAX_ROWS).enumerate() {
            self.insert(e, i);
        }
        self.set_stats(h);
    }

    pub fn prepend(&self, e: &Entry) {
        self.insert(e, 0);
        unsafe { SendMessageW(self.list, LVM_DELETEITEM, MAX_ROWS, 0) };
    }

    fn insert(&self, e: &Entry, at: usize) {
        unsafe {
            let when = wide(&fmt_when(&e.when));
            let item = LVITEMW { mask: LVIF_TEXT, iItem: at as i32, pszText: when.as_ptr() as *mut u16, ..core::mem::zeroed() };
            let idx = SendMessageW(self.list, LVM_INSERTITEMW, 0, &item as *const _ as LPARAM);
            if idx < 0 {
                return;
            }
            for (col, text) in [(1, fmt_duration(e.audio_ms as u64)), (2, e.words.to_string()), (3, e.text.clone())] {
                let t = wide(&text);
                let sub = LVITEMW { mask: LVIF_TEXT, iSubItem: col, pszText: t.as_ptr() as *mut u16, ..core::mem::zeroed() };
                SendMessageW(self.list, LVM_SETITEMTEXTW, idx as usize, &sub as *const _ as LPARAM);
            }
        }
    }

    pub fn selected_text(&self) -> Option<String> {
        unsafe {
            let i = SendMessageW(self.list, LVM_GETNEXTITEM, usize::MAX, LVNI_SELECTED as LPARAM);
            if i < 0 {
                return None;
            }
            let mut buf = vec![0u16; 8192];
            let item = LVITEMW { iSubItem: TEXT_COL as i32, pszText: buf.as_mut_ptr(), cchTextMax: buf.len() as i32, ..core::mem::zeroed() };
            let n = SendMessageW(self.list, LVM_GETITEMTEXTW, i as usize, &item as *const _ as LPARAM);
            Some(String::from_utf16_lossy(&buf[..n.max(0) as usize]))
        }
    }

    /// WM_CTLCOLORSTATIC: draw labels on the window background instead of grey.
    pub fn static_brush(&self, hdc: HDC) -> HBRUSH {
        unsafe {
            SetBkColor(hdc, GetSysColor(COLOR_WINDOW));
            GetSysColorBrush(COLOR_WINDOW)
        }
    }
}

unsafe fn font_(s: f32) -> HFONT {
    unsafe { font(s, 11, FW_SEMIBOLD as i32) }
}

impl Drop for Ui {
    fn drop(&mut self) {
        unsafe {
            DeleteObject(self.font);
            DeleteObject(self.bold);
        }
    }
}

/// "2026-10-07 15:40:12" -> "15:40" today, "10-07 15:40" otherwise.
fn fmt_when(when: &str) -> String {
    if when.len() < 16 {
        return when.to_string();
    }
    let now = crate::win::local_time();
    if when.starts_with(&now[..10]) { when[11..16].to_string() } else { when[5..16].to_string() }
}
