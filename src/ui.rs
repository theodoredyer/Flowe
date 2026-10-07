//! The dashboard window: pause/resume, usage stats, recording history.
//! Shown on launch; closing it only hides it (left-click the tray icon to bring it back).

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
const GREY: u32 = 0x007A_7171; // COLORREF is BGR

pub struct Ui {
    pub hwnd: HWND,
    status: HWND,
    pause: HWND,
    tiles: [(HWND, HWND); 4], // (value, label)
    today: HWND,
    hint: HWND,
    clear: HWND,
    list: HWND,
    fonts: [HFONT; 3], // normal, bold, big
    s: f32,
}

fn px(s: f32, v: i32) -> i32 {
    (v as f32 * s).round() as i32
}

unsafe fn font(s: f32, pt: i32, weight: u32) -> HFONT {
    unsafe {
        CreateFontW(
            -px(s, pt * 96 / 72),
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
                px(s, 720),
                px(s, 540),
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
            let label = |text: &str| child("STATIC", text, (SS_LEFT | SS_ENDELLIPSIS) as u32, 0);
            let status = label("");
            let pause = child("BUTTON", "Pause", WS_TABSTOP | BS_PUSHBUTTON as u32, ID_PAUSE);
            let tiles = ["recordings", "words dictated", "of speech", "words / min"].map(|l| (label("–"), label(l)));
            let today = label("");
            let hint = label("History  ·  double-click a row to copy it again");
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

            let fonts = [font(s, 9, FW_NORMAL), font(s, 11, FW_SEMIBOLD), font(s, 22, FW_SEMIBOLD)];
            let set_font = |h: HWND, f: HFONT| {
                SendMessageW(h, WM_SETFONT, f as usize, 1);
            };
            set_font(status, fonts[1]);
            for (value, lbl) in tiles {
                set_font(value, fonts[2]);
                set_font(lbl, fonts[0]);
            }
            for h in [pause, today, hint, clear, list] {
                set_font(h, fonts[0]);
            }
            let ui = Self { hwnd, status, pause, tiles, today, hint, clear, list, fonts, s };
            ui.layout();
            Some(ui)
        }
    }

    pub fn layout(&self) {
        unsafe {
            let mut r: RECT = core::mem::zeroed();
            GetClientRect(self.hwnd, &mut r);
            let (w, h, s) = (r.right, r.bottom, self.s);
            let m = px(s, 20);
            let (bw, bh) = (px(s, 104), px(s, 30));
            MoveWindow(self.status, m, px(s, 20), w - 2 * m - bw - px(s, 12), px(s, 26), 1);
            MoveWindow(self.pause, w - m - bw, px(s, 17), bw, bh, 1);

            let gap = px(s, 12);
            let tw = (w - 2 * m - 3 * gap) / 4;
            for (i, (value, lbl)) in self.tiles.iter().enumerate() {
                let x = m + i as i32 * (tw + gap);
                MoveWindow(*value, x, px(s, 62), tw, px(s, 36), 1);
                MoveWindow(*lbl, x, px(s, 98), tw, px(s, 18), 1);
            }
            MoveWindow(self.today, m, px(s, 128), w - 2 * m, px(s, 18), 1);

            let cw = px(s, 104);
            MoveWindow(self.hint, m, px(s, 166), w - 2 * m - cw - px(s, 12), px(s, 20), 1);
            MoveWindow(self.clear, w - m - cw, px(s, 160), cw, px(s, 28), 1);
            let top = px(s, 196);
            MoveWindow(self.list, m, top, w - 2 * m, (h - top - m).max(px(s, 60)), 1);
            // The text column takes whatever width is left.
            let used: i32 = (0..TEXT_COL).map(|i| SendMessageW(self.list, LVM_GETCOLUMNWIDTH, i, 0) as i32).sum();
            let avail = w - 2 * m - used - GetSystemMetrics(SM_CXVSCROLL) - px(s, 4);
            SendMessageW(self.list, LVM_SETCOLUMNWIDTH, TEXT_COL, avail.max(px(s, 120)) as LPARAM);
        }
    }

    pub fn min_size(&self) -> (i32, i32) {
        (px(self.s, 560), px(self.s, 400))
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
        let st = h.stats();
        let values = [
            thousands(st.recordings),
            thousands(st.words),
            fmt_duration(st.audio_ms),
            if st.audio_ms > 0 { format!("{:.0}", st.wpm()) } else { "–".into() },
        ];
        let today = if st.recordings == 0 {
            "No recordings yet. Hold Ctrl+Win and say something.".to_string()
        } else {
            format!(
                "Today: {} recordings, {} words   ·   transcription takes {:.2}s on average, {:.0}× faster than realtime",
                st.today_recordings,
                thousands(st.today_words),
                st.avg_latency_s(),
                st.speed()
            )
        };
        unsafe {
            for ((value, _), text) in self.tiles.iter().zip(values) {
                SetWindowTextW(*value, wide(&text).as_ptr());
            }
            SetWindowTextW(self.today, wide(&today).as_ptr());
        }
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

    /// WM_CTLCOLORSTATIC: labels on the window background, secondary text in grey.
    pub fn static_brush(&self, hdc: HDC, ctl: HWND) -> HBRUSH {
        unsafe {
            SetBkColor(hdc, GetSysColor(COLOR_WINDOW));
            if ctl == self.today || self.tiles.iter().any(|(_, lbl)| *lbl == ctl) {
                SetTextColor(hdc, GREY);
            }
            GetSysColorBrush(COLOR_WINDOW)
        }
    }
}

impl Drop for Ui {
    fn drop(&mut self) {
        unsafe {
            for f in self.fonts {
                DeleteObject(f);
            }
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
