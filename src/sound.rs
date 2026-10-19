//! Start / stop / lock sounds: soft keyboard clicks, picked in the dashboard from a few packs of
//! real recordings (all CC0, see assets/sounds/CREDITS.md). Shaped once and played from memory.
//!
//! Each clip is trimmed to its onset, high-passed (no thump) and gently low-passed (no tick),
//! faded out, and normalized, so e    out.iter_mut().for_each(|v| *v *= gain(&out));ery pack sits at the same level and fits in the mic guards.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use windows_sys::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};

const RATE: u32 = 44_100;
/// Every clip is scaled to this RMS over CLIP_MS (so short and long clicks sound about as loud),
/// as long as its peak stays under MAX_PEAK.
const LOUDNESS: f32 = 0.035;
const MAX_PEAK: f32 = 0.7;
/// Each keystroke is cut to this long, the last FADE_MS fading out.
const CLIP_MS: u32 = 80;
const FADE_MS: u32 = 30;
const HIGH_PASS_HZ: f32 = 150.0;
const LOW_PASS_HZ: f32 = 5000.0;
/// Lock is two keystrokes this far apart.
const LOCK_GAP_MS: u32 = 70;
/// The dashboard preview plays start, then stop this much later.
const PREVIEW_GAP_MS: u32 = 320;
/// The recorder drops this much mic audio while our own sound plays so it isn't transcribed.
const START_GUARD_MS: u32 = 90;
const LOCK_GUARD_MS: u32 = 160;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pack {
    /// A lubed tactile switch: low and rounded.
    Tactile,
    /// A typical mechanical keyboard.
    Mechanical,
    /// Short, tight gaming-keyboard clicks.
    Crisp,
    /// A clean little UI click.
    Click,
}

#[derive(Clone, Copy)]
enum Role {
    Start,
    Stop,
    Lock,
}

/// The raw cuts, [start, stop, lock] per pack (16-bit mono 44.1 kHz).
const CLIPS: [[&[u8]; 3]; 4] = [
    [
        include_bytes!("../assets/sounds/tactile-start.wav"),
        include_bytes!("../assets/sounds/tactile-stop.wav"),
        include_bytes!("../assets/sounds/tactile-lock.wav"),
    ],
    [
        include_bytes!("../assets/sounds/mechanical-start.wav"),
        include_bytes!("../assets/sounds/mechanical-stop.wav"),
        include_bytes!("../assets/sounds/mechanical-lock.wav"),
    ],
    [
        include_bytes!("../assets/sounds/crisp-start.wav"),
        include_bytes!("../assets/sounds/crisp-stop.wav"),
        include_bytes!("../assets/sounds/crisp-lock.wav"),
    ],
    [
        include_bytes!("../assets/sounds/click-start.wav"),
        include_bytes!("../assets/sounds/click-stop.wav"),
        include_bytes!("../assets/sounds/click-start.wav"), // a double tap of the start click
    ],
];

impl Pack {
    pub const ALL: [Pack; 4] = [Pack::Tactile, Pack::Mechanical, Pack::Crisp, Pack::Click];

    pub fn label(self) -> &'static str {
        match self {
            Pack::Tactile => "Tactile",
            Pack::Mechanical => "Mechanical",
            Pack::Crisp => "Crisp",
            Pack::Click => "Click",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Pack::Tactile => "tactile",
            Pack::Mechanical => "mechanical",
            Pack::Crisp => "crisp",
            Pack::Click => "click",
        }
    }

    pub fn from_key(k: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.key() == k)
    }

    fn index(self) -> usize {
        self as usize
    }

    fn clip(self, role: Role) -> Vec<f32> {
        shape(&decode(CLIPS[self.index()][role as usize]))
    }

    fn render(self, role: Role) -> Vec<f32> {
        match role {
            Role::Start | Role::Stop => self.clip(role),
            Role::Lock => mix(&[(self.clip(Role::Lock), 0), (self.clip(Role::Lock), LOCK_GAP_MS)]),
        }
    }

    fn preview(self) -> Vec<f32> {
        mix(&[(self.clip(Role::Start), 0), (self.clip(Role::Stop), PREVIEW_GAP_MS)])
    }
}

static USER_ON: AtomicBool = AtomicBool::new(true);
static PACK: AtomicU8 = AtomicU8::new(0);

/// The dashboard switch.
pub fn set_enabled(on: bool) {
    USER_ON.store(on, Ordering::Relaxed);
}

pub fn set_pack(p: Pack) {
    PACK.store(p.index() as u8, Ordering::Relaxed);
}

fn pack() -> Pack {
    Pack::ALL[PACK.load(Ordering::Relaxed) as usize % Pack::ALL.len()]
}

/// Dev/test runs (fake mic, fake level, or FLOWE_NO_SOUND) are always silent.
fn enabled() -> bool {
    static DEV: OnceLock<bool> = OnceLock::new();
    let dev = *DEV.get_or_init(|| ["FLOWE_FAKE_MIC", "FLOWE_FAKE_LEVEL", "FLOWE_NO_SOUND"].iter().any(|v| std::env::var_os(v).is_some()));
    !dev && USER_ON.load(Ordering::Relaxed)
}

/// How much of the start of a recording to discard (0 when sounds are off).
pub fn start_guard_ms() -> u32 {
    if enabled() { START_GUARD_MS } else { 0 }
}

/// Rendered wavs, made on first use: [start, stop, lock, preview] per pack.
static WAVS: [[OnceLock<Vec<u8>>; 4]; 4] = [const { [const { OnceLock::new() }; 4] }; 4];

fn cached(p: Pack, slot: usize, make: impl FnOnce() -> Vec<f32>) -> &'static [u8] {
    WAVS[p.index()][slot].get_or_init(|| wav(&make()))
}

pub fn start() {
    let p = pack();
    play("start", cached(p, 0, || p.render(Role::Start)));
}

pub fn stop() {
    let p = pack();
    play("stop", cached(p, 1, || p.render(Role::Stop)));
}

/// Plays the lock sound; returns how much mic input to discard while it plays (0 when silent).
pub fn lock() -> u32 {
    let p = pack();
    play("lock", cached(p, 2, || p.render(Role::Lock)));
    if enabled() { LOCK_GUARD_MS } else { 0 }
}

/// Start then stop, so a pack can be heard when it's picked.
pub fn preview() {
    let p = pack();
    play("preview", cached(p, 3, || p.preview()));
}

fn play(name: &str, wav: &'static [u8]) {
    if !enabled() {
        return;
    }
    // SAFETY: SND_MEMORY reads the wav from this 'static buffer; SND_ASYNC returns immediately.
    let ok = unsafe { PlaySoundW(wav.as_ptr().cast(), core::ptr::null_mut(), SND_MEMORY | SND_ASYNC | SND_NODEFAULT) };
    if ok == 0 {
        crate::win::log(&format!("sound '{name}' failed to play"));
    }
}

/// Samples of one of our own embedded wavs (16-bit mono PCM).
fn decode(wav: &[u8]) -> Vec<f32> {
    let data = wav.windows(4).position(|w| w == b"data").map_or(wav.len(), |i| i + 8);
    wav[data.min(wav.len())..].chunks_exact(2).map(|s| i16::from_le_bytes([s[0], s[1]]) as f32 / 32768.0).collect()
}

/// Trims to the onset, filters, cuts to CLIP_MS with a fade, and normalizes to LOUDNESS.
fn shape(raw: &[f32]) -> Vec<f32> {
    let peak = raw.iter().fold(0.0f32, |m, v| m.max(v.abs())).max(1e-6);
    let onset = raw.iter().position(|v| v.abs() > 0.1 * peak).unwrap_or(0).saturating_sub((RATE / 1000) as usize);
    let len = ((RATE * CLIP_MS / 1000) as usize).min(raw.len() - onset);
    let hp = 1.0 - (-std::f32::consts::TAU * HIGH_PASS_HZ / RATE as f32).exp(); // one-pole coefficients
    let lp = 1.0 - (-std::f32::consts::TAU * LOW_PASS_HZ / RATE as f32).exp();
    let (mut low, mut y) = (0.0f32, 0.0f32);
    let mut out: Vec<f32> = raw[onset..onset + len]
        .iter()
        .map(|&x| {
            low += hp * (x - low);
            y += lp * ((x - low) - y);
            y
        })
        .collect();
    let fade_in = (RATE / 1000) as usize;
    let fade_out = ((RATE * FADE_MS / 1000) as usize).min(len);
    for (i, v) in out.iter_mut().enumerate() {
        let tail = (len - i) as f32 / fade_out as f32;
        let head = i as f32 / fade_in as f32;
        // Raised-cosine out, so the end lands on silence without a click.
        let out_gain = if tail < 1.0 { 0.5 - 0.5 * (std::f32::consts::PI * tail).cos() } else { 1.0 };
        *v *= head.min(1.0) * out_gain;
    }
    let window = (RATE * CLIP_MS / 1000) as f32;
    let rms = (out.iter().map(|v| v * v).sum::<f32>() / window).sqrt().max(1e-6);
    let peak = out.iter().fold(0.0f32, |m, v| m.max(v.abs())).max(1e-6);
    let gain = (LOUDNESS / rms).min(MAX_PEAK / peak);
    out.iter_mut().for_each(|v| *v *= gain);
    out
}

/// Lays clips over each other at the given offsets (ms).
fn mix(parts: &[(Vec<f32>, u32)]) -> Vec<f32> {
    let at = |ms: u32| (RATE * ms / 1000) as usize;
    let len = parts.iter().map(|(c, ms)| at(*ms) + c.len()).max().unwrap_or(0);
    let mut out = vec![0.0f32; len];
    for (c, ms) in parts {
        for (o, v) in out[at(*ms)..].iter_mut().zip(c) {
            *o += v;
        }
    }
    out
}

/// 16-bit mono wav with 10 ms of trailing silence so the device doesn't clip the tail.
fn wav(samples: &[f32]) -> Vec<u8> {
    let pcm: Vec<i16> = samples
        .iter()
        .map(|v| (v.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
        .chain(std::iter::repeat_n(0, (RATE / 100) as usize))
        .collect();
    let data_len = (pcm.len() * 2) as u32;
    let mut w = Vec::with_capacity(44 + data_len as usize);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data_len).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes()); // PCM
    w.extend_from_slice(&1u16.to_le_bytes()); // mono
    w.extend_from_slice(&RATE.to_le_bytes());
    w.extend_from_slice(&(RATE * 2).to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data_len.to_le_bytes());
    for s in pcm {
        w.extend_from_slice(&s.to_le_bytes());
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROLES: [Role; 3] = [Role::Start, Role::Stop, Role::Lock];

    fn energy(v: &[f32]) -> f32 {
        v.iter().map(|x| x * x).sum()
    }

    /// Share of the energy above (or, `high == false`, below) `hz`, split with a one-pole filter.
    fn band_share(s: &[f32], hz: f32, high: bool) -> f32 {
        let a = 1.0 - (-std::f32::consts::TAU * hz / RATE as f32).exp();
        let mut lp = 0.0;
        let band: Vec<f32> = s
            .iter()
            .map(|&x| {
                lp += a * (x - lp);
                if high { x - lp } else { lp }
            })
            .collect();
        energy(&band) / energy(s)
    }

    #[test]
    fn keys_roundtrip() {
        for p in Pack::ALL {
            assert_eq!(Pack::from_key(p.key()), Some(p));
        }
        assert_eq!(Pack::from_key("thock"), None);
    }

    #[test]
    fn sounds_are_valid_and_click_free() {
        for p in Pack::ALL {
            for role in ROLES {
                let s = p.render(role);
                let peak = s.iter().fold(0.0f32, |m, v| m.max(v.abs()));
                assert!(peak > 0.1 && peak <= MAX_PEAK + 1e-4, "{p:?}: peak {peak}");
                assert!(s[0].abs() < 0.02 && s[s.len() - 1].abs() < 1e-4, "{p:?}: click at an edge");
                let w = wav(&s);
                assert_eq!(&w[0..4], b"RIFF");
                assert_eq!(w.len(), 44 + u32::from_le_bytes(w[40..44].try_into().unwrap()) as usize);
            }
        }
    }

    #[test]
    fn sounds_are_soft_not_thumpy_or_sharp() {
        // No boom from the desk or the case, and no hiss or tick on top.
        for p in Pack::ALL {
            for role in ROLES {
                let s = p.render(role);
                let low = band_share(&s, 200.0, false);
                let high = band_share(&s, 6000.0, true);
                assert!(low < 0.06, "{p:?}: {:.1}% of the energy is below ~200 Hz", low * 100.0);
                assert!(high < 0.1, "{p:?}: {:.1}% of the energy is above ~6 kHz", high * 100.0);
            }
        }
    }

    #[test]
    fn packs_are_about_equally_loud() {
        // Energy over a fixed window, start sounds only (lock is two of them).
        let loud = |p: Pack| p.render(Role::Start).iter().map(|v| v * v).sum::<f32>();
        let (min, max) = Pack::ALL.iter().map(|&p| loud(p)).fold((f32::MAX, 0.0f32), |(a, b), e| (a.min(e), b.max(e)));
        assert!(max / min < 1.6, "loudest pack has {:.1}x the energy of the quietest", max / min);
    }

    #[test]
    fn lock_is_a_double_tap() {
        for p in Pack::ALL {
            let s = p.render(Role::Lock);
            let win = |ms: u32| energy(&s[(RATE * ms / 1000) as usize..(RATE * (ms + 10) / 1000) as usize]);
            assert!(win(0) > 3.0 * win(LOCK_GAP_MS - 15) && win(LOCK_GAP_MS) > 3.0 * win(LOCK_GAP_MS - 15), "{p:?}: lock doesn't sound like two taps");
        }
    }

    #[test]
    fn sounds_fit_inside_their_mic_guards() {
        let audible = |s: &[f32]| s.iter().rposition(|v| v.abs() > 0.003).unwrap_or(0) as u32 * 1000 / RATE;
        for p in Pack::ALL {
            assert!(audible(&p.render(Role::Start)) <= START_GUARD_MS, "{p:?}: start audible for {} ms", audible(&p.render(Role::Start)));
            assert!(audible(&p.render(Role::Lock)) <= LOCK_GUARD_MS, "{p:?}: lock audible for {} ms", audible(&p.render(Role::Lock)));
        }
    }
}
