//! Ctrl+Win push-to-talk via a low-level keyboard hook.
//!
//! Hold Ctrl+Win to record. Press Space while holding to lock (hands-free);
//! press Ctrl+Win again to stop. Esc cancels a locked recording. Any other key
//! pressed during a hold cancels, so Ctrl+Win+Arrow etc keep working.

use std::sync::Mutex;
use std::sync::atomic::{AtomicPtr, AtomicU32, Ordering};

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::win;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Start = 1,
    Lock,
    Stop,
    Cancel,
}

impl Action {
    pub fn from_wparam(w: WPARAM) -> Option<Self> {
        [Action::Start, Action::Lock, Action::Stop, Action::Cancel]
            .into_iter()
            .find(|a| *a as usize == w)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    Idle,
    Hold,
    Locked,
}

#[derive(Default, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub action: Option<Action>,
    pub swallow: bool,
    /// Tap a dummy key so releasing Win doesn't open the Start menu.
    pub mask: bool,
}

pub struct Combo {
    state: State,
    ctrl: bool,
    win: bool,
    armed: bool,
    swallowing: [bool; 256],
}

const VK_LCONTROL: u32 = 0xA2;
const VK_RCONTROL: u32 = 0xA3;
const VK_CONTROL: u32 = 0x11;
const VK_LWIN: u32 = 0x5B;
const VK_RWIN: u32 = 0x5C;
const VK_SPACE: u32 = 0x20;
const VK_ESCAPE: u32 = 0x1B;

fn is_ctrl(vk: u32) -> bool {
    matches!(vk, VK_CONTROL | VK_LCONTROL | VK_RCONTROL)
}
fn is_win(vk: u32) -> bool {
    matches!(vk, VK_LWIN | VK_RWIN)
}

impl Combo {
    pub const fn new() -> Self {
        Self { state: State::Idle, ctrl: false, win: false, armed: true, swallowing: [false; 256] }
    }

    /// Back to idle (e.g. recording auto-stopped). Won't re-trigger until the combo is released.
    pub fn reset(&mut self) {
        self.state = State::Idle;
        self.armed = !(self.ctrl && self.win);
    }

    pub fn handle(&mut self, vk: u32, down: bool) -> Outcome {
        let mut out = Outcome::default();
        let idx = (vk & 0xFF) as usize;
        if self.swallowing[idx] {
            if !down {
                self.swallowing[idx] = false;
            }
            out.swallow = true;
            return out;
        }

        let was_combo = self.ctrl && self.win;
        if is_ctrl(vk) {
            self.ctrl = down;
        } else if is_win(vk) {
            self.win = down;
        }
        let combo = self.ctrl && self.win;
        let pressed = combo && !was_combo;
        if !combo {
            self.armed = true;
        }

        match self.state {
            State::Idle => {
                if pressed && self.armed {
                    self.state = State::Hold;
                    out.mask = true;
                    out.action = Some(Action::Start);
                }
            }
            State::Hold => {
                if !combo {
                    self.state = State::Idle;
                    out.action = Some(Action::Stop);
                } else if down && vk == VK_SPACE {
                    self.state = State::Locked;
                    self.swallowing[idx] = true;
                    out.swallow = true;
                    out.action = Some(Action::Lock);
                } else if down && !is_ctrl(vk) && !is_win(vk) {
                    self.state = State::Idle;
                    self.armed = false;
                    out.action = Some(Action::Cancel);
                }
            }
            State::Locked => {
                if pressed {
                    self.state = State::Idle;
                    self.armed = false;
                    out.mask = true;
                    out.action = Some(Action::Stop);
                } else if down && vk == VK_ESCAPE {
                    self.state = State::Idle;
                    self.swallowing[idx] = true;
                    out.swallow = true;
                    out.action = Some(Action::Cancel);
                }
            }
        }
        out
    }
}

pub static COMBO: Mutex<Combo> = Mutex::new(Combo::new());
static TARGET: AtomicPtr<core::ffi::c_void> = AtomicPtr::new(core::ptr::null_mut());
static MSG_ID: AtomicU32 = AtomicU32::new(0);
static HOOK_THREAD: AtomicU32 = AtomicU32::new(0);
const WM_SET_ENABLED: u32 = WM_APP + 10;

/// Installs the hook on its own thread (so a busy UI thread can never lag the keyboard)
/// and posts `msg` with an `Action` as wparam to `target`.
pub fn install(target: HWND, msg: u32) {
    TARGET.store(target, Ordering::SeqCst);
    MSG_ID.store(msg, Ordering::SeqCst);
    std::thread::Builder::new()
        .name("hook".into())
        .spawn(|| unsafe {
            HOOK_THREAD.store(GetCurrentThreadId(), Ordering::SeqCst);
            let mut hook = hook_on();
            let mut m: MSG = core::mem::zeroed();
            while GetMessageW(&mut m, core::ptr::null_mut(), 0, 0) > 0 {
                if m.hwnd.is_null() && m.message == WM_SET_ENABLED {
                    // Paused = hook fully removed, so Windows never even calls us.
                    if m.wParam != 0 && hook.is_null() {
                        hook = hook_on();
                    } else if m.wParam == 0 && !hook.is_null() {
                        UnhookWindowsHookEx(hook);
                        hook = core::ptr::null_mut();
                    }
                    if let Ok(mut c) = COMBO.lock() {
                        *c = Combo::new();
                    }
                    continue;
                }
                DispatchMessageW(&m);
            }
            if !hook.is_null() {
                UnhookWindowsHookEx(hook);
            }
        })
        .expect("spawn hook thread");
}

unsafe fn hook_on() -> HHOOK {
    let hook = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), core::ptr::null_mut(), 0) };
    if hook.is_null() {
        win::log("failed to install keyboard hook");
    }
    hook
}

/// Turn the hotkey off (pause) or back on. Safe to call from any thread.
pub fn set_enabled(enabled: bool) {
    let tid = HOOK_THREAD.load(Ordering::SeqCst);
    if tid != 0 {
        unsafe { PostThreadMessageW(tid, WM_SET_ENABLED, enabled as usize, 0) };
    }
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: for WH_KEYBOARD_LL with HC_ACTION, lparam points to a KBDLLHOOKSTRUCT.
        let kb = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        if kb.flags & LLKHF_INJECTED == 0 {
            let down = matches!(wparam as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
            let out = match COMBO.lock() {
                Ok(mut c) => c.handle(kb.vkCode, down),
                Err(_) => Outcome::default(),
            };
            if out.mask {
                win::tap_mask_key();
            }
            if let Some(a) = out.action {
                unsafe {
                    PostMessageW(TARGET.load(Ordering::SeqCst), MSG_ID.load(Ordering::SeqCst), a as usize, 0)
                };
            }
            if out.swallow {
                return 1;
            }
        }
    }
    unsafe { CallNextHookEx(core::ptr::null_mut(), code, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Action::*;

    const C: u32 = VK_LCONTROL;
    const W: u32 = VK_LWIN;
    const SP: u32 = VK_SPACE;
    const ESC: u32 = VK_ESCAPE;
    const A: u32 = 0x41;
    const LEFT: u32 = 0x25;

    fn run(seq: &[(u32, bool)]) -> (Vec<Action>, Vec<bool>) {
        let mut c = Combo::new();
        let outs: Vec<_> = seq.iter().map(|&(vk, d)| c.handle(vk, d)).collect();
        (outs.iter().filter_map(|o| o.action).collect(), outs.iter().map(|o| o.swallow).collect())
    }

    #[test]
    fn hold_and_release_either_order() {
        assert_eq!(run(&[(C, true), (W, true), (W, true), (C, true), (W, false), (C, false)]).0, [Start, Stop]);
        assert_eq!(run(&[(W, true), (C, true), (C, false), (W, false)]).0, [Start, Stop]);
    }

    #[test]
    fn space_locks_and_is_swallowed_including_repeat() {
        let (a, s) = run(&[(C, true), (W, true), (SP, true), (SP, true), (SP, false), (W, false), (C, false)]);
        assert_eq!(a, [Start, Lock]);
        assert_eq!(s, [false, false, true, true, true, false, false]);
    }

    #[test]
    fn locked_stops_on_combo_again_not_on_ctrl_alone() {
        let lock = [(C, true), (W, true), (SP, true), (SP, false), (W, false), (C, false)];
        let mut seq = lock.to_vec();
        seq.extend([(C, true), (A, true), (A, false), (C, false)]);
        assert_eq!(run(&seq).0, [Start, Lock]);
        let mut seq = lock.to_vec();
        seq.extend([(C, true), (W, true), (W, false), (C, false)]);
        assert_eq!(run(&seq).0, [Start, Lock, Stop]);
    }

    #[test]
    fn esc_cancels_locked_and_is_swallowed() {
        let (a, s) = run(&[(C, true), (W, true), (SP, true), (SP, false), (W, false), (C, false), (ESC, true), (ESC, false)]);
        assert_eq!(a, [Start, Lock, Cancel]);
        assert_eq!(&s[6..], [true, true]);
    }

    #[test]
    fn other_shortcuts_cancel_and_pass_through() {
        let (a, s) = run(&[(C, true), (W, true), (LEFT, true), (LEFT, false), (LEFT, true), (W, false), (C, false)]);
        assert_eq!(a, [Start, Cancel]);
        assert!(s.iter().all(|x| !x));
    }

    #[test]
    fn no_restart_while_still_held_after_locked_stop() {
        let (a, _) = run(&[
            (C, true), (W, true), (SP, true), (SP, false), (W, false), (C, false),
            (C, true), (W, true), (C, true), (W, true), (W, false), (C, false),
            (C, true), (W, true),
        ]);
        assert_eq!(a, [Start, Lock, Stop, Start]);
    }

    #[test]
    fn plain_keys_untouched() {
        assert_eq!(run(&[(SP, true), (SP, false)]), (vec![], vec![false, false]));
        assert_eq!(run(&[(C, true), (C, false), (W, true), (W, false)]).0, []);
    }
}
