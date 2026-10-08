//! Small Win32 helpers: synthetic keys, clipboard, logging, single instance.

use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::DataExchange::*;
use windows_sys::Win32::System::Memory::*;
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;

const CF_UNICODETEXT: u32 = 13;
const VK_MASK: u16 = 0xE8; // unassigned; tapping it while Win is down stops the Start menu opening

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

pub fn data_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("Fleow")
}

/// "YYYY-MM-DD HH:MM:SS" in local time.
pub fn local_time() -> String {
    let mut t: SYSTEMTIME = unsafe { core::mem::zeroed() };
    unsafe { GetLocalTime(&mut t) };
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

pub fn log(msg: &str) {
    let path = data_dir().join("fleow.log");
    // Keep the log from growing forever.
    if std::fs::metadata(&path).map(|m| m.len() > 1 << 20).unwrap_or(false) {
        let _ = std::fs::remove_file(&path);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{} {msg}", local_time());
    }
}

/// Returns false if another Fleow is already running. The mutex lives until the process exits.
pub fn single_instance() -> bool {
    let name = wide("fleow-single-instance");
    unsafe {
        CreateMutexW(core::ptr::null(), 0, name.as_ptr());
        GetLastError() != ERROR_ALREADY_EXISTS
    }
}

fn send_keys(events: &[(u16, bool)]) {
    let inputs: Vec<INPUT> = events
        .iter()
        .map(|&(vk, down)| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: 0,
                    dwFlags: if down { 0 } else { KEYEVENTF_KEYUP },
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        })
        .collect();
    unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), size_of::<INPUT>() as i32) };
}

pub fn tap_mask_key() {
    send_keys(&[(VK_MASK, true), (VK_MASK, false)]);
}

pub fn send_paste() {
    send_keys(&[(VK_LCONTROL, true), (VK_V, true), (VK_V, false), (VK_LCONTROL, false)]);
}

fn is_down(vk: u16) -> bool {
    unsafe { GetAsyncKeyState(vk as i32) as u16 & 0x8000 != 0 }
}

/// Pasting while Win is still held would fire Win+V etc, so wait until the user lets go.
pub fn wait_modifiers_released() {
    let end = Instant::now() + Duration::from_secs(3);
    let keys = [VK_LCONTROL, VK_RCONTROL, VK_LWIN, VK_RWIN, VK_SPACE];
    while Instant::now() < end && keys.iter().any(|&k| is_down(k)) {
        std::thread::sleep(Duration::from_millis(10));
    }
}

pub fn set_clipboard(owner: HWND, text: &str) -> bool {
    let utf16 = wide(text);
    let bytes = utf16.len() * 2;
    unsafe {
        // Another app may briefly hold the clipboard.
        let mut opened = false;
        for _ in 0..20 {
            if OpenClipboard(owner) != 0 {
                opened = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        if !opened {
            return false;
        }
        EmptyClipboard();
        let mut ok = false;
        let h = GlobalAlloc(GMEM_MOVEABLE, bytes);
        if !h.is_null() {
            let p = GlobalLock(h) as *mut u16;
            if !p.is_null() {
                core::ptr::copy_nonoverlapping(utf16.as_ptr(), p, utf16.len());
                GlobalUnlock(h);
                // On success the system owns the memory; otherwise we must free it.
                ok = !SetClipboardData(CF_UNICODETEXT, h as HANDLE).is_null();
            }
            if !ok {
                GlobalFree(h);
            }
        }
        CloseClipboard();
        ok
    }
}
