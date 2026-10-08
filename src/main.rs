//! flowe: hold Ctrl+Win, talk, let go -> local Parakeet transcribes, copies and pastes.
#![windows_subsystem = "windows"]

mod asr;
mod audio;
mod draw;
mod history;
mod hotkey;
mod icon;
mod overlay;
mod pill;
mod settings;
mod sound;
mod tray;
mod ui;
mod viz;
mod win;

use std::cell::RefCell;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::UI::HiDpi::*;
use windows_sys::Win32::Graphics::Gdi::ValidateRect;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use history::{Entry, History};
use hotkey::Action;
use overlay::{Idle, Overlay, View};
use settings::Settings;
use tray::{Status, Tray};
use ui::{Ui, UiEvent};
use win::wide;

const WM_ACTION: u32 = WM_APP + 1; // wparam: hotkey::Action
const WM_ASR: u32 = WM_APP + 2; // "there's something on the results channel"
const WM_TRAY: u32 = WM_APP + 3;
const WM_AUTOSTOP: u32 = WM_APP + 4;
const TIMER_ANIM: usize = 1;
const TIMER_FLASH: usize = 2;
const WM_MOUSELEAVE: u32 = 0x02A3; // not re-exported by windows-sys where the other WM_ consts are
const MIN_HOLD: Duration = Duration::from_millis(300);
const ID_QUIT: usize = 1;
const ID_LOG: usize = 2;
const ID_OPEN: usize = 3;
const ID_TOGGLE: usize = 4;
const TIP_READY: &str = "Flowe - Ctrl+Win to talk, +Space to lock";
/// Broadcast by a second instance so the running one shows its dashboard.
const SHOW_MSG_NAME: &str = "flowe-show";

enum AsrMsg {
    Ready,
    LoadFailed,
    Transcribed(Entry),
    Empty,
    Failed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Model {
    Loading,
    Ready,
    Failed,
}

struct App {
    hwnd: HWND,
    recorder: audio::Recorder,
    overlay: Overlay,
    tray: Tray,
    ui: Ui,
    history: History,
    jobs: mpsc::Sender<(Vec<f32>, u32)>,
    results: mpsc::Receiver<AsrMsg>,
    model: Model,
    recording: bool,
    paused: bool,
    started: Instant,
    taskbar_created: u32,
    show_me: u32,
    settings: Settings,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

fn with_app(f: impl FnOnce(&mut App)) {
    APP.with(|a| {
        if let Ok(mut a) = a.try_borrow_mut()
            && let Some(app) = a.as_mut()
        {
            f(app)
        }
    });
}

impl App {
    fn animate(&self) {
        unsafe { SetTimer(self.hwnd, TIMER_ANIM, 16, None) };
    }

    /// Push the current state to the tray icon, tooltip, idle dash and dashboard header.
    fn sync(&mut self) {
        let (status, tip, text, button, idle) = if self.paused {
            (Status::Paused, "Flowe - paused", "Paused. Ctrl+Win does nothing until you resume.", "Resume", None)
        } else if self.recording {
            (Status::Recording, TIP_READY, "Recording…", "Pause", Some(Idle::Ready))
        } else {
            match self.model {
                Model::Loading => (Status::Loading, "Flowe - loading model...", "Loading the speech model…", "Pause", Some(Idle::Loading)),
                Model::Failed => (
                    Status::Error,
                    "Flowe - model failed to load (see log)",
                    "The speech model failed to load. Open the log from the tray menu.",
                    "Pause",
                    Some(Idle::Error),
                ),
                Model::Ready => (Status::Ready, TIP_READY, "Listening. Hold Ctrl+Win to talk; tap Space while holding to lock.", "Pause", Some(Idle::Ready)),
            }
        };
        self.tray.set(status, Some(tip));
        self.ui.set_status(status, text, button);
        self.overlay.set_idle(idle);
    }

    fn set_paused(&mut self, paused: bool) {
        if self.paused == paused {
            return;
        }
        if paused && self.recording {
            self.on_action(Action::Cancel);
        }
        self.paused = paused;
        hotkey::set_enabled(!paused);
        self.sync();
    }

    fn flash(&mut self) {
        self.overlay.set(View::Flash);
        self.animate();
    }

    fn on_action(&mut self, action: Action) {
        match action {
            Action::Start => {
                let hwnd = self.hwnd as usize;
                let on_full = move || unsafe {
                    PostMessageW(hwnd as HWND, WM_AUTOSTOP, 0, 0);
                };
                if let Err(e) = self.recorder.start(sound::start_guard_ms(), on_full) {
                    win::log(&format!("mic error: {e}"));
                    if let Ok(mut c) = hotkey::COMBO.lock() {
                        c.reset();
                    }
                    self.flash();
                    return;
                }
                sound::start();
                self.recording = true;
                self.started = Instant::now();
                self.overlay.set(View::Recording);
                self.animate();
            }
            Action::Lock => {
                win::log("locked hands-free");
                let guard = sound::lock();
                self.recorder.mute(guard);
                self.overlay.set(View::Locked);
            }
            Action::Stop => self.stop(),
            Action::Cancel => {
                self.recording = false;
                drop(self.recorder.stop());
                self.overlay.back_to_idle();
            }
        }
        self.sync();
    }

    fn stop(&mut self) {
        if !self.recording {
            return;
        }
        self.recording = false;
        let (audio, rate) = self.recorder.stop();
        sound::stop();
        win::log(&format!("stop: {:.1}s audio, level {:.3}", audio.len() as f32 / rate as f32, audio::rms(&audio)));
        let too_short = self.started.elapsed() < MIN_HOLD || audio.len() < rate as usize * 3 / 10;
        if self.model == Model::Failed {
            // Nobody will ever consume the job; don't let recordings pile up in memory.
            self.flash();
        } else if too_short || self.jobs.send((audio, rate)).is_err() {
            self.overlay.back_to_idle();
        } else {
            self.overlay.set(View::Transcribing);
        }
        self.sync();
    }

    fn on_asr(&mut self) {
        while let Ok(msg) = self.results.try_recv() {
            match msg {
                AsrMsg::Ready => self.model = Model::Ready,
                AsrMsg::LoadFailed => self.model = Model::Failed,
                AsrMsg::Transcribed(entry) => {
                    self.ui.prepend(&entry);
                    self.history.push(entry);
                    self.ui.set_stats(&self.history);
                    if !self.recording {
                        self.overlay.back_to_idle();
                    }
                }
                AsrMsg::Empty | AsrMsg::Failed => {
                    if !self.recording {
                        self.flash();
                    }
                }
            }
        }
        self.sync();
    }

    fn copy_selected(&mut self) {
        if let Some(text) = self.ui.selected_text()
            && !text.is_empty()
            && win::set_clipboard(self.hwnd, &text)
        {
            self.ui.set_notice(Some("Copied to clipboard"));
            unsafe { SetTimer(self.hwnd, TIMER_FLASH, 1500, None) };
        }
    }

    fn set_style(&mut self, style: viz::Style) {
        self.settings.style = style;
        self.settings.save();
        self.overlay.set_style(style);
        self.ui.set_prefs(style, self.settings.sounds);
        if !self.recording {
            self.overlay.demo(); // show what it looks like
            self.animate();
        }
    }

    fn toggle_sounds(&mut self) {
        self.settings.sounds = !self.settings.sounds;
        self.settings.save();
        sound::set_enabled(self.settings.sounds);
        self.ui.set_prefs(self.settings.style, self.settings.sounds);
        if self.settings.sounds {
            sound::start(); // let them hear it
        }
    }

    fn clear_history(&mut self) {
        self.history.clear();
        self.ui.rebuild(&self.history);
    }
}

/// Right-click tray menu. Runs a modal loop, so it must be called without holding the APP borrow.
fn show_menu(hwnd: HWND, paused: bool) -> usize {
    unsafe {
        let menu = CreatePopupMenu();
        let items = [
            (ID_OPEN, "Open Flowe"),
            (ID_TOGGLE, if paused { "Resume listening" } else { "Pause listening" }),
            (ID_LOG, "Open log"),
            (ID_QUIT, "Quit"),
        ];
        for (i, (id, label)) in items.iter().enumerate() {
            if i == 2 {
                AppendMenuW(menu, MF_SEPARATOR, 0, core::ptr::null());
            }
            let label = wide(label);
            AppendMenuW(menu, MF_STRING, *id, label.as_ptr());
        }
        SetMenuDefaultItem(menu, ID_OPEN as u32, 0);
        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        SetForegroundWindow(hwnd); // so the menu closes when clicking elsewhere
        let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON, pt.x, pt.y, 0, hwnd, core::ptr::null());
        DestroyMenu(menu);
        cmd as usize
    }
}

fn open_log() {
    unsafe {
        let path = wide(&win::data_dir().join("flowe.log").to_string_lossy());
        let open = wide("open");
        ShellExecuteW(core::ptr::null_mut(), open.as_ptr(), path.as_ptr(), core::ptr::null(), core::ptr::null(), SW_SHOWNORMAL);
    }
}

fn run_command(hwnd: HWND, cmd: usize) {
    match cmd {
        ID_OPEN => with_app(|app| app.ui.show()),
        ID_TOGGLE => with_app(|app| app.set_paused(!app.paused)),
        ID_LOG => open_log(),
        ID_QUIT => unsafe {
            DestroyWindow(hwnd);
        },
        _ => {}
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_ACTION => {
            if let Some(a) = Action::from_wparam(wparam) {
                with_app(|app| app.on_action(a));
            }
        }
        WM_AUTOSTOP => {
            if let Ok(mut c) = hotkey::COMBO.lock() {
                c.reset();
            }
            with_app(|app| app.stop());
        }
        WM_ASR => with_app(|app| app.on_asr()),
        WM_TIMER if wparam == TIMER_ANIM => with_app(|app| {
            if !app.overlay.tick(app.recorder.level()) {
                unsafe { KillTimer(hwnd, TIMER_ANIM) };
            }
        }),
        WM_TIMER if wparam == TIMER_FLASH => {
            unsafe { KillTimer(hwnd, TIMER_FLASH) };
            with_app(|app| app.ui.set_notice(None));
        }
        WM_TIMER if wparam == ui::TIMER_UI => with_app(|app| app.ui.tick()),
        WM_TRAY => match (lparam & 0xFFFF) as u32 {
            WM_LBUTTONUP => run_command(hwnd, ID_OPEN),
            WM_RBUTTONUP => {
                let mut paused = false;
                with_app(|app| paused = app.paused);
                let cmd = show_menu(hwnd, paused);
                run_command(hwnd, cmd);
            }
            _ => {}
        },
        WM_PAINT => {
            let mut painted = false;
            with_app(|app| {
                app.ui.paint();
                painted = true;
            });
            if !painted {
                unsafe { ValidateRect(hwnd, core::ptr::null()) }; // never leave the region dirty
            }
        }
        WM_ERASEBKGND => return 1, // the paint covers everything
        WM_SIZE => with_app(|app| app.ui.on_size()),
        WM_MOUSEMOVE | WM_MOUSELEAVE | WM_LBUTTONDOWN | WM_LBUTTONUP | WM_LBUTTONDBLCLK | WM_MOUSEWHEEL => {
            let mut ev = UiEvent::None;
            with_app(|app| ev = app.ui.on_mouse(msg, wparam, lparam));
            match ev {
                UiEvent::TogglePause => with_app(|app| app.set_paused(!app.paused)),
                UiEvent::ClearHistory => with_app(|app| app.clear_history()),
                UiEvent::CopyRow => with_app(|app| app.copy_selected()),
                UiEvent::SetStyle(style) => with_app(|app| app.set_style(style)),
                UiEvent::ToggleSounds => with_app(|app| app.toggle_sounds()),
                UiEvent::None => {}
            }
        }
        WM_SETCURSOR if (lparam & 0xFFFF) as u32 == HTCLIENT => {
            let mut hand = false;
            with_app(|app| hand = app.ui.hand_cursor());
            if !hand {
                return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
            }
            unsafe { SetCursor(LoadCursorW(core::ptr::null_mut(), IDC_HAND)) };
            return 1;
        }
        WM_KEYDOWN if wparam as u16 == VK_ESCAPE => unsafe {
            ShowWindow(hwnd, SW_HIDE);
        },
        WM_GETMINMAXINFO => {
            let mut size = (0, 0);
            with_app(|app| size = app.ui.min_size());
            if size.0 > 0 {
                // SAFETY: WM_GETMINMAXINFO's lparam points to a MINMAXINFO.
                let mmi = unsafe { &mut *(lparam as *mut MINMAXINFO) };
                mmi.ptMinTrackSize = POINT { x: size.0, y: size.1 };
            }
        }
        // Taskbar moved/resized or monitors changed: keep the dash just above the taskbar.
        WM_DISPLAYCHANGE | WM_SETTINGCHANGE => with_app(|app| app.overlay.place()),
        WM_CLOSE => unsafe {
            ShowWindow(hwnd, SW_HIDE); // keep running in the tray
        },
        WM_DESTROY => {
            APP.with(|a| a.borrow_mut().take()); // drops tray icon, overlay, mic
            unsafe { PostQuitMessage(0) };
        }
        _ => {
            let mut handled = false;
            with_app(|app| {
                if msg == app.taskbar_created {
                    app.tray.add();
                    handled = true;
                } else if msg == app.show_me {
                    app.ui.show();
                    handled = true;
                }
            });
            if !handled {
                return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
            }
        }
    }
    0
}

fn asr_thread(hwnd: usize, jobs: mpsc::Receiver<(Vec<f32>, u32)>, results: mpsc::Sender<AsrMsg>) {
    let post = |msg: AsrMsg| {
        let _ = results.send(msg);
        unsafe { PostMessageW(hwnd as HWND, WM_ASR, 0, 0) };
    };
    let dir = win::data_dir().join("model");
    let missing: Vec<_> = asr::MODEL_FILES.iter().filter(|f| !dir.join(f).exists()).collect();
    if !missing.is_empty() {
        win::log(&format!("model files missing from {}: {missing:?} (run download-model.ps1)", dir.display()));
        post(AsrMsg::LoadFailed);
        return;
    }
    let t = Instant::now();
    let mut model = match asr::Parakeet::load(&dir) {
        Ok(m) => m,
        Err(e) => {
            win::log(&format!("model load failed: {e}"));
            post(AsrMsg::LoadFailed);
            return;
        }
    };
    let _ = model.transcribe(&vec![0.0; 16000]); // warm-up
    win::log(&format!("model ready in {:.1}s", t.elapsed().as_secs_f32()));
    post(AsrMsg::Ready);

    for (audio, rate) in jobs {
        let t = Instant::now();
        let audio = audio::resample(&audio, rate, 16000);
        let audio_ms = (audio.len() as u64 * 1000 / 16000) as u32;
        match model.transcribe(&audio) {
            Ok(text) if !text.is_empty() => {
                let latency_ms = t.elapsed().as_millis() as u32;
                // The log only gets numbers; the text itself goes to history.tsv.
                win::log(&format!("{:.1}s audio -> {latency_ms}ms, {} chars", audio_ms as f32 / 1000.0, text.len()));
                post(AsrMsg::Transcribed(Entry::new(text.clone(), audio_ms, latency_ms)));
                // With a fake mic (dev testing) only the clipboard is set: never type into whatever is focused.
                let fake_mic = std::env::var_os("FLOWE_FAKE_MIC").is_some();
                if win::set_clipboard(hwnd as HWND, &(text + " ")) && !fake_mic {
                    win::wait_modifiers_released();
                    win::send_paste();
                }
            }
            Ok(_) => {
                win::log(&format!("{:.1}s audio -> empty", audio_ms as f32 / 1000.0));
                post(AsrMsg::Empty);
            }
            Err(e) => {
                win::log(&format!("transcription failed: {e}"));
                post(AsrMsg::Failed);
            }
        }
    }
}

fn main() {
    let _ = std::fs::create_dir_all(win::data_dir());
    let show_me = unsafe { RegisterWindowMessageW(wide(SHOW_MSG_NAME).as_ptr()) };
    if !win::single_instance() {
        // Already running: just bring up its dashboard (e.g. launched again from Start).
        unsafe {
            let other = FindWindowW(wide("flowe-main").as_ptr(), core::ptr::null());
            if !other.is_null() {
                AllowSetForegroundWindow(ASFW_ANY);
                PostMessageW(other, show_me, 0, 0);
            }
        }
        return;
    }
    let start_hidden = std::env::args().any(|a| a == "--tray");
    win::log("starting");
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_SYSTEM_AWARE);
        let Some(mut ui) = Ui::new(Some(wndproc)) else {
            win::log("failed to create window");
            return;
        };
        let hwnd = ui.hwnd;
        let settings = Settings::load();
        sound::set_enabled(settings.sounds);
        let Some(overlay) = Overlay::new(settings.style) else {
            win::log("failed to create overlay");
            return;
        };
        let taskbar_created = RegisterWindowMessageW(wide("TaskbarCreated").as_ptr());
        let (jobs_tx, jobs_rx) = mpsc::channel();
        let (res_tx, res_rx) = mpsc::channel();
        let h = hwnd as usize;
        if std::thread::Builder::new().name("asr".into()).spawn(move || asr_thread(h, jobs_rx, res_tx)).is_err() {
            win::log("failed to start asr thread");
            return;
        }
        let history = History::load();
        ui.rebuild(&history);
        ui.set_prefs(settings.style, settings.sounds);
        APP.with(|a| {
            *a.borrow_mut() = Some(App {
                hwnd,
                recorder: audio::Recorder::new(),
                overlay,
                tray: Tray::new(hwnd, WM_TRAY),
                ui,
                history,
                jobs: jobs_tx,
                results: res_rx,
                model: Model::Loading,
                recording: false,
                paused: false,
                started: Instant::now(),
                taskbar_created,
                show_me,
                settings,
            })
        });
        with_app(|app| {
            app.sync();
            if !start_hidden {
                app.ui.show();
            }
        });
        hotkey::install(hwnd, WM_ACTION);

        let mut m: MSG = core::mem::zeroed();
        while GetMessageW(&mut m, core::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&m);
            DispatchMessageW(&m);
        }
    }
    win::log("exiting");
}
