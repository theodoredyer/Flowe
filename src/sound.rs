//! Start / stop / lock sounds: soft "thocks" like a creamy, lubed mechanical keyboard switch.
//! Synthesized once and played from memory (no asset files, no extra threads).
//!
//! A thock is a short damped low tone (the body of the switch and keycap) plus a brief muffled
//! noise burst (the bottom-out), all through a gentle low-pass so nothing is bright or sharp.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use windows_sys::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};

const RATE: u32 = 44_100;
const VOLUME: f32 = 0.42;

/// One keystroke: (body pitch Hz, body decay s, noise brightness Hz, start offset ms).
type Thock = (f32, f32, f32, u32);
/// Start: a single mid thock. Stop: a deeper, slightly longer one. Lock: a quick double-tap.
const START: &[Thock] = &[(240.0, 0.020, 1300.0, 0)];
const STOP: &[Thock] = &[(165.0, 0.026, 1000.0, 0)];
const LOCK: &[Thock] = &[(285.0, 0.016, 1500.0, 0), (285.0, 0.016, 1500.0, 70)];
/// The recorder drops this much mic audio while our own sound plays so it isn't transcribed.
const START_GUARD_MS: u32 = 90;
const LOCK_GUARD_MS: u32 = 160;

static USER_ON: AtomicBool = AtomicBool::new(true);

/// The dashboard switch.
pub fn set_enabled(on: bool) {
    USER_ON.store(on, Ordering::Relaxed);
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

pub fn start() {
    static WAV: OnceLock<Vec<u8>> = OnceLock::new();
    play("start", WAV.get_or_init(|| wav(&synth(START))));
}

pub fn stop() {
    static WAV: OnceLock<Vec<u8>> = OnceLock::new();
    play("stop", WAV.get_or_init(|| wav(&synth(STOP))));
}

/// Plays the lock sound; returns how much mic input to discard while it plays (0 when silent).
pub fn lock() -> u32 {
    static WAV: OnceLock<Vec<u8>> = OnceLock::new();
    play("lock", WAV.get_or_init(|| wav(&synth(LOCK))));
    if enabled() { LOCK_GUARD_MS } else { 0 }
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

/// Mixes the keystrokes, low-passes the whole thing, and normalizes to VOLUME.
fn synth(strokes: &[Thock]) -> Vec<f32> {
    let len_ms = strokes.iter().map(|s| s.3 + 90).max().unwrap_or(90);
    let mut out = vec![0.0f32; (RATE * len_ms / 1000) as usize];
    let mut rng = 0x2468_ACE1u32;
    for &(pitch, decay, bright, at) in strokes {
        let start = (RATE * at / 1000) as usize;
        let mut noise_lp = 0.0f32;
        let a = 1.0 - (-std::f32::consts::TAU * bright / RATE as f32).exp(); // one-pole low-pass coefficient
        for (i, o) in out[start..].iter_mut().enumerate() {
            let t = i as f32 / RATE as f32;
            let attack = (t / 0.0015).min(1.0); // 1.5 ms ramp: soft, no click
            // Body: fundamental plus a quieter, faster-dying overtone (wood/plastic, not a bell).
            let body = (std::f32::consts::TAU * pitch * t).sin() * (-t / decay).exp()
                + 0.35 * (std::f32::consts::TAU * pitch * 2.03 * t).sin() * (-t / (decay * 0.5)).exp();
            // Bottom-out: a few ms of noise, already darkened by its own low-pass.
            rng ^= rng << 13;
            rng ^= rng >> 17;
            rng ^= rng << 5;
            let white = (rng >> 8) as f32 / (1u32 << 23) as f32 - 1.0;
            noise_lp += a * (white - noise_lp);
            let thud = noise_lp * (-t / 0.006).exp();
            *o += attack * (0.85 * body + 1.6 * thud);
        }
    }
    // Final gentle low-pass at ~2.2 kHz takes off any remaining edge.
    let a = 1.0 - (-std::f32::consts::TAU * 2200.0 / RATE as f32).exp();
    let mut y = 0.0f32;
    for v in out.iter_mut() {
        y += a * (*v - y);
        *v = y;
    }
    // Fade the last 10 ms so the end is silent.
    let fade = (RATE / 100) as usize;
    let n = out.len();
    for (i, v) in out[n - fade..].iter_mut().enumerate() {
        *v *= 1.0 - i as f32 / fade as f32;
    }
    let max = out.iter().fold(0.0f32, |m, v| m.max(v.abs())).max(1e-6);
    out.iter_mut().for_each(|v| *v *= VOLUME / max);
    out
}

/// 16-bit mono wav with 10 ms of trailing silence so the device doesn't clip the tail.
fn wav(samples: &[f32]) -> Vec<u8> {
    let pcm: Vec<i16> = samples
        .iter()
        .map(|v| (v * i16::MAX as f32) as i16)
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

    fn energy(v: &[f32]) -> f32 {
        v.iter().map(|x| x * x).sum()
    }

    #[test]
    fn sounds_are_valid_and_click_free() {
        for spec in [START, STOP, LOCK] {
            let s = synth(spec);
            let peak = s.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!((peak - VOLUME).abs() < 1e-3, "peak {peak}");
            assert!(s[0].abs() < 0.01 && s[s.len() - 1].abs() < 1e-4, "click at an edge");
            let w = wav(&s);
            assert_eq!(&w[0..4], b"RIFF");
            assert_eq!(w.len(), 44 + u32::from_le_bytes(w[40..44].try_into().unwrap()) as usize);
        }
    }

    #[test]
    fn sounds_are_soft_not_sharp() {
        // "Harshness" = energy in the high band. Split the signal with a ~2.5 kHz one-pole low-pass:
        // what the low-pass removes is the high band. A creamy thock keeps it tiny.
        for spec in [START, STOP, LOCK] {
            let s = synth(spec);
            let a = 1.0 - (-std::f32::consts::TAU * 2500.0 / RATE as f32).exp();
            let mut lp = 0.0;
            let high: Vec<f32> = s
                .iter()
                .map(|&x| {
                    lp += a * (x - lp);
                    x - lp
                })
                .collect();
            let ratio = energy(&high) / energy(&s);
            assert!(ratio < 0.03, "{:.1}% of the energy is above ~2.5 kHz", ratio * 100.0);
        }
    }

    #[test]
    fn stop_is_deeper_than_start_and_lock_is_a_double_tap() {
        let zc = |v: &[f32]| v.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count();
        assert!(zc(&synth(STOP)) < zc(&synth(START)));
        // Two distinct hits: energy dips between them and comes back.
        let s = synth(LOCK);
        let win = |ms: u32| energy(&s[(RATE * ms / 1000) as usize..(RATE * (ms + 10) / 1000) as usize]);
        assert!(win(0) > 5.0 * win(55) && win(70) > 5.0 * win(55), "lock doesn't sound like two taps");
    }

    #[test]
    fn sounds_fit_inside_their_mic_guards() {
        let ms = |s: &[f32]| (s.len() as u32 + RATE / 100) * 1000 / RATE; // + wav tail
        let audible = |s: &[f32]| {
            // last sample still above 1% of peak
            let last = s.iter().rposition(|v| v.abs() > 0.01 * VOLUME).unwrap_or(0) as u32;
            last * 1000 / RATE
        };
        assert!(audible(&synth(START)) <= START_GUARD_MS, "start audible for {} ms", audible(&synth(START)));
        assert!(audible(&synth(LOCK)) <= LOCK_GUARD_MS, "lock audible for {} ms", audible(&synth(LOCK)));
        assert!(ms(&synth(START)) < 150);
    }
}
