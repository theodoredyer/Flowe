//! Start/stop chimes: synthesized once, played from memory (no asset files, no extra threads).

use std::sync::OnceLock;

use windows_sys::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};

const RATE: u32 = 44_100;
const VOLUME: f32 = 0.28;
/// The recorder drops this much mic audio after the start chime so the chime is not transcribed.
const START_GUARD_MS: u32 = 150;

/// Dev/test runs (fake mic, fake level, or FLOWE_NO_SOUND) stay silent.
fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| ["FLOWE_FAKE_MIC", "FLOWE_FAKE_LEVEL", "FLOWE_NO_SOUND"].iter().all(|v| std::env::var_os(v).is_none()))
}

/// How much of the start of a recording to discard (0 when the chime is silent).
pub fn start_guard_ms() -> u32 {
    if enabled() { START_GUARD_MS } else { 0 }
}

pub fn start() {
    static WAV: OnceLock<Vec<u8>> = OnceLock::new();
    play(WAV.get_or_init(|| synth(&[(659.25, 60), (987.77, 70)]))); // E5 -> B5, rising
}

pub fn stop() {
    static WAV: OnceLock<Vec<u8>> = OnceLock::new();
    play(WAV.get_or_init(|| synth(&[(987.77, 55), (659.25, 85)]))); // B5 -> E5, falling
}

fn play(wav: &'static [u8]) {
    if enabled() {
        // SAFETY: SND_MEMORY reads the wav from this 'static buffer; SND_ASYNC returns immediately.
        unsafe { PlaySoundW(wav.as_ptr().cast(), core::ptr::null_mut(), SND_MEMORY | SND_ASYNC | SND_NODEFAULT) };
    }
}

/// Soft bell-like notes (sine + quiet octave) with a fast attack and exponential decay, as a 16-bit mono wav.
fn synth(notes: &[(f32, u32)]) -> Vec<u8> {
    let mut pcm: Vec<i16> = Vec::new();
    for &(freq, ms) in notes {
        let n = (RATE * ms / 1000) as usize;
        for i in 0..n {
            let t = i as f32 / RATE as f32;
            let attack = (t / 0.004).min(1.0);
            let release = ((n - i) as f32 / (RATE as f32 * 0.012)).min(1.0); // avoid a click at the note end
            let env = attack * release * (-t * 14.0).exp();
            let tone = (std::f32::consts::TAU * freq * t).sin() + 0.25 * (std::f32::consts::TAU * freq * 2.0 * t).sin();
            pcm.push((tone / 1.25 * env * VOLUME * i16::MAX as f32) as i16);
        }
    }
    pcm.extend(std::iter::repeat_n(0, (RATE / 100) as usize)); // 10 ms of silence so the device doesn't cut the tail
    let data_len = (pcm.len() * 2) as u32;
    let mut wav = Vec::with_capacity(44 + data_len as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&RATE.to_le_bytes());
    wav.extend_from_slice(&(RATE * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    for s in pcm {
        wav.extend_from_slice(&s.to_le_bytes());
    }
    wav
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chimes_are_valid_quiet_and_shorter_than_the_guard() {
        for notes in [&[(659.25, 60), (987.77, 70)][..], &[(987.77, 55), (659.25, 85)][..]] {
            let wav = synth(notes);
            assert_eq!(&wav[0..4], b"RIFF");
            let data = u32::from_le_bytes(wav[40..44].try_into().unwrap()) as usize;
            assert_eq!(wav.len(), 44 + data);
            let samples: Vec<i16> = wav[44..].chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
            let peak = samples.iter().map(|s| s.unsigned_abs()).max().unwrap();
            assert!(peak > 3000 && peak < (0.4 * i16::MAX as f32) as u16, "peak {peak}");
            assert_eq!(samples.last(), Some(&0));
        }
        let start_ms = (synth(&[(659.25, 60), (987.77, 70)]).len() as u32 - 44) / 2 * 1000 / RATE;
        assert!(start_ms <= START_GUARD_MS, "start chime {start_ms} ms outlasts the guard");
    }
}
