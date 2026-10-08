//! Start/stop "whoosh" sounds: band-passed noise with a swept centre frequency, synthesized
//! once and played from memory (no asset files, no extra threads).

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use windows_sys::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};

const RATE: u32 = 44_100;
const VOLUME: f32 = 0.30;
/// The recorder drops this much mic audio after the start sound so it isn't transcribed.
const START_GUARD_MS: u32 = 150;
/// (length ms, start Hz, end Hz, where the swell peaks 0..1)
const START: (u32, f32, f32, f32) = (170, 350.0, 2600.0, 0.55);
const STOP: (u32, f32, f32, f32) = (200, 2400.0, 320.0, 0.35);

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
    play(WAV.get_or_init(|| wav(&whoosh(START))));
}

pub fn stop() {
    static WAV: OnceLock<Vec<u8>> = OnceLock::new();
    play(WAV.get_or_init(|| wav(&whoosh(STOP))));
}

fn play(wav: &'static [u8]) {
    if enabled() {
        // SAFETY: SND_MEMORY reads the wav from this 'static buffer; SND_ASYNC returns immediately.
        unsafe { PlaySoundW(wav.as_ptr().cast(), core::ptr::null_mut(), SND_MEMORY | SND_ASYNC | SND_NODEFAULT) };
    }
}

/// White noise through a state-variable band-pass filter whose centre glides exponentially from
/// `f0` to `f1`, shaped by a smooth swell (sin^2 up to `peak`, cos^2 down), normalized to VOLUME.
fn whoosh((ms, f0, f1, peak): (u32, f32, f32, f32)) -> Vec<f32> {
    let n = (RATE * ms / 1000) as usize;
    let mut rng = 0x1234_5678u32;
    let (mut low, mut band) = (0.0f32, 0.0f32);
    let damping = 0.9;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let x = i as f32 / n as f32;
        let fc = f0 * (f1 / f0).powf(x);
        let f = 2.0 * (std::f32::consts::PI * fc / RATE as f32).sin();
        rng ^= rng << 13;
        rng ^= rng >> 17;
        rng ^= rng << 5;
        let noise = (rng >> 8) as f32 / (1u32 << 23) as f32 - 1.0;
        let high = noise - low - damping * band;
        band += f * high;
        low += f * band;
        let env = if x < peak {
            (std::f32::consts::FRAC_PI_2 * x / peak).sin().powi(2)
        } else {
            (std::f32::consts::FRAC_PI_2 * (x - peak) / (1.0 - peak)).cos().powi(2)
        };
        out.push((band * 0.8 + low * 0.2) * env);
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

    #[test]
    fn whooshes_are_valid_quiet_and_click_free() {
        for spec in [START, STOP] {
            let s = whoosh(spec);
            let peak = s.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!((peak - VOLUME).abs() < 1e-3, "peak {peak}");
            // Fades in and out: no click at either end.
            assert!(s[0].abs() < 0.01 && s[s.len() - 1].abs() < 0.01);
            let w = wav(&s);
            assert_eq!(&w[0..4], b"RIFF");
            assert_eq!(w.len(), 44 + u32::from_le_bytes(w[40..44].try_into().unwrap()) as usize);
        }
    }

    #[test]
    fn start_whoosh_energy_is_mostly_inside_the_mic_guard() {
        let s = whoosh(START);
        let guard = (RATE * START_GUARD_MS / 1000) as usize;
        let energy = |v: &[f32]| v.iter().map(|x| x * x).sum::<f32>();
        let inside = energy(&s[..guard.min(s.len())]) / energy(&s);
        assert!(inside > 0.9, "only {:.0}% of the start whoosh is inside the guard", inside * 100.0);
    }

    #[test]
    fn sweeps_go_the_right_way() {
        // Rough spectral centroid via zero crossings: start rises, stop falls.
        let zc = |v: &[f32]| v.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count();
        for (spec, rising) in [(START, true), (STOP, false)] {
            let s = whoosh(spec);
            let (a, b) = s.split_at(s.len() / 2);
            assert_eq!(zc(b) > zc(a), rising, "{spec:?}");
        }
    }
}
