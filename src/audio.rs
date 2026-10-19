//! Microphone capture. The device is only opened while recording, so idle costs nothing.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// Hard cap so a forgotten hands-free recording can't grow without bound.
pub const MAX_SECONDS: usize = 5 * 60;

#[derive(Default)]
struct Shared {
    buf: Mutex<Vec<f32>>,
    level: AtomicU32, // f32 bits, RMS of the latest callback
    full: AtomicBool,
    /// Samples still to discard (while one of our own sounds is playing).
    skip: AtomicUsize,
}

pub struct Recorder {
    stream: Option<cpal::Stream>,
    shared: Arc<Shared>,
    rate: u32,
    /// Dev hook (FLOWE_FAKE_LEVEL): synthetic loudness so the overlay animation can be checked without speaking.
    fake_level: Option<std::time::Instant>,
}

impl Recorder {
    pub fn new() -> Self {
        Self {
            stream: None,
            shared: Arc::default(),
            rate: 16000,
            fake_level: std::env::var_os("FLOWE_FAKE_LEVEL").map(|_| std::time::Instant::now()),
        }
    }

    pub fn level(&self) -> f32 {
        if let Some(t0) = self.fake_level {
            let t = t0.elapsed().as_secs_f32();
            return 0.01 + 0.08 * (2.2 * t).sin().abs() * (0.6 + 0.4 * (0.9 * t).sin());
        }
        f32::from_bits(self.shared.level.load(Ordering::Relaxed))
    }

    /// `on_full` fires (once, from the audio thread) when MAX_SECONDS is reached.
    pub fn start(&mut self, guard_ms: u32, on_full: impl Fn() + Send + 'static) -> Result<(), String> {
        self.stream = None;
        let device = cpal::default_host().default_input_device().ok_or("no microphone found")?;
        let config = device.default_input_config().map_err(|e| e.to_string())?;
        self.rate = config.sample_rate();
        let channels = config.channels() as usize;
        let cap = MAX_SECONDS * self.rate as usize;

        if let Ok(mut b) = self.shared.buf.lock() {
            b.clear();
            b.shrink_to(self.rate as usize * 30);
        }
        self.shared.level.store(0, Ordering::Relaxed);
        self.shared.full.store(false, Ordering::Relaxed);

        let shared = self.shared.clone();
        // Drop the first `guard_ms` of input: that is when the start sound is playing.
        self.mute(guard_ms);
        let push = move |mono: &mut dyn Iterator<Item = f32>| {
            let mut skip = shared.skip.load(Ordering::Relaxed);
            if skip > 0 {
                while skip > 0 && mono.next().is_some() {
                    skip -= 1;
                }
                shared.skip.store(skip, Ordering::Relaxed);
                if skip > 0 {
                    return;
                }
            }
            let Ok(mut buf) = shared.buf.lock() else { return };
            let start = buf.len();
            buf.extend(mono.take(cap.saturating_sub(start)));
            let new = &buf[start..];
            if !new.is_empty() {
                let rms = (new.iter().map(|x| x * x).sum::<f32>() / new.len() as f32).sqrt();
                shared.level.store(rms.to_bits(), Ordering::Relaxed);
            }
            if buf.len() >= cap && !shared.full.swap(true, Ordering::Relaxed) {
                on_full();
            }
        };
        let err = |e: cpal::Error| {
            // WASAPI flags the first packet of a fresh stream as a discontinuity; harmless.
            if !e.to_string().contains("underrun") {
                crate::win::log(&format!("audio stream error: {e}"));
            }
        };
        let cfg: cpal::StreamConfig = config.clone().into();
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device.build_input_stream(
                cfg.clone(),
                move |d: &[f32], _: &_| push(&mut d.chunks(channels).map(|f| f.iter().sum::<f32>() / channels as f32)),
                err,
                None,
            ),
            cpal::SampleFormat::I16 => device.build_input_stream(
                cfg.clone(),
                move |d: &[i16], _: &_| {
                    push(&mut d.chunks(channels).map(|f| f.iter().map(|&s| s as f32 / 32768.0).sum::<f32>() / channels as f32))
                },
                err,
                None,
            ),
            other => return Err(format!("unsupported sample format {other:?}")),
        }
        .map_err(|e| e.to_string())?;
        stream.play().map_err(|e| e.to_string())?;
        self.stream = Some(stream);
        Ok(())
    }

    /// Discards the next `ms` of mic input (so our own sounds are not transcribed).
    pub fn mute(&self, ms: u32) {
        self.shared.skip.store((self.rate as u64 * ms as u64 / 1000) as usize, Ordering::Relaxed);
    }

    /// Closes the mic and hands back (samples, sample_rate).
    pub fn stop(&mut self) -> (Vec<f32>, u32) {
        self.stream = None; // dropping closes the device
        let samples = self.shared.buf.lock().map(|mut b| std::mem::take(&mut *b)).unwrap_or_default();
        if let Some(path) = std::env::var_os("FLOWE_FAKE_MIC") {
            // Dev aid: pretend the mic heard this wav, so the whole pipeline can be tested silently.
            if let Some(wav) = read_wav_pcm16(std::path::Path::new(&path)) {
                return wav;
            }
        }
        (samples, self.rate)
    }
}

pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|x| x * x).sum::<f32>() / samples.len() as f32).sqrt()
}

/// Minimal reader for 16-bit PCM wav files: (mono samples, sample rate).
pub fn read_wav_pcm16(path: &std::path::Path) -> Option<(Vec<f32>, u32)> {
    let b = std::fs::read(path).ok()?;
    if b.len() < 44 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return None;
    }
    let channels = u16::from_le_bytes([b[22], b[23]]).max(1) as usize;
    let rate = u32::from_le_bytes([b[24], b[25], b[26], b[27]]);
    let data = b.windows(4).position(|w| w == b"data")? + 8;
    let samples = b[data..]
        .chunks_exact(2 * channels)
        .map(|f| f.chunks_exact(2).map(|s| i16::from_le_bytes([s[0], s[1]]) as f32 / 32768.0).sum::<f32>() / channels as f32)
        .collect();
    Some((samples, rate))
}

/// Windowed-sinc resampler (band-limited, so 48k -> 16k doesn't alias).
pub fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || input.is_empty() || from == 0 || to == 0 {
        return input.to_vec(); // 0 Hz only comes from a broken wav; don't size a buffer from it
    }
    let ratio = to as f64 / from as f64;
    let cutoff = ratio.min(1.0) * 0.92; // fraction of the input Nyquist we keep
    let half = (16.0 / cutoff).ceil() as usize;
    let taps = 2 * half + 1;
    // Output i sits at input position i * step / phases. The fractional part only takes `phases`
    // values, so each one's filter is computed once rather than per sample (sin/cos are the cost).
    let g = gcd(from, to);
    let (step, phases) = ((from / g) as usize, (to / g) as usize);
    let kernel = |frac: f64| -> Vec<f32> {
        (0..taps)
            .map(|j| {
                let d = frac + half as f64 - j as f64;
                let t = cutoff * d;
                let sinc = if t.abs() < 1e-9 { 1.0 } else { (std::f64::consts::PI * t).sin() / (std::f64::consts::PI * t) };
                let w = 0.5 + 0.5 * (std::f64::consts::PI * d / (half as f64 + 1.0)).cos(); // Hann
                (sinc * w) as f32
            })
            .collect()
    };
    let table: Vec<Vec<f32>> = if phases <= 4096 { (0..phases).map(|p| kernel(p as f64 / phases as f64)).collect() } else { Vec::new() };
    let out_len = (input.len() as f64 * ratio) as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let pos = i * step;
        let (center, phase) = (pos / phases, pos % phases);
        let odd;
        let k = match table.get(phase) {
            Some(k) => k,
            None => {
                odd = kernel(phase as f64 / phases as f64);
                &odd
            }
        };
        // Taps falling off either end of the input are dropped and the rest renormalized.
        let first = half.saturating_sub(center);
        let last = (input.len() + half - center).min(taps);
        let start = center + first - half;
        let (mut acc, mut norm) = (0.0f32, 0.0f32);
        for (c, x) in k[first..last].iter().zip(&input[start..]) {
            acc += x * c;
            norm += c;
        }
        out.push(if norm.abs() > 1e-9 { acc / norm } else { 0.0 });
    }
    out
}

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 { a } else { gcd(b, a % b) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_keeps_low_tone_and_kills_high_tone() {
        let sr = 48000;
        let tone = |f: f64| (0..sr).map(|i| (2.0 * std::f64::consts::PI * f * i as f64 / sr as f64).sin() as f32).collect::<Vec<_>>();
        let rms = |v: &[f32]| (v[1000..v.len() - 1000].iter().map(|x| x * x).sum::<f32>() / (v.len() - 2000) as f32).sqrt();
        let low = resample(&tone(440.0), 48000, 16000);
        let high = resample(&tone(12000.0), 48000, 16000); // above the 8k Nyquist: must not alias in
        assert_eq!(low.len(), 16000);
        assert!((rms(&low) - 0.707).abs() < 0.02, "{}", rms(&low));
        assert!(rms(&high) < 0.02, "{}", rms(&high));
        // 44.1k has 160 filter phases instead of 1.
        let tone = |f: f64| (0..44100).map(|i| (2.0 * std::f64::consts::PI * f * i as f64 / 44100.0).sin() as f32).collect::<Vec<_>>();
        let low = resample(&tone(440.0), 44100, 16000);
        assert_eq!(low.len(), 16000);
        assert!((rms(&low) - 0.707).abs() < 0.02, "{}", rms(&low));
        assert!(rms(&resample(&tone(12000.0), 44100, 16000)) < 0.02);
    }

    #[test]
    fn resample_edges_stay_level() {
        assert_eq!(resample(&[0.1, 0.2], 0, 16000), vec![0.1, 0.2]); // a broken wav header
        // Taps past either end are dropped and the rest renormalized, so a constant stays constant.
        for from in [48000, 44100, 22050] {
            let out = resample(&vec![0.5; from as usize / 10], from, 16000);
            assert!(out.iter().all(|v| (v - 0.5).abs() < 1e-3), "{from}: {:?} .. {:?}", &out[..3], &out[out.len() - 3..]);
        }
    }
}

