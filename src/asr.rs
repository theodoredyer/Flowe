//! Parakeet TDT (NeMo) inference on ONNX Runtime.
//!
//! Pipeline (same as onnx-asr): waveform -> log-mel (nemo128.onnx) -> conformer encoder
//! (GPU via DirectML) -> greedy TDT decoding with the decoder/joint network (CPU, tiny).

use std::path::Path;
use std::sync::Mutex;
use std::time::Instant;

use ort::ep::DirectML;
use ort::session::Session;
use ort::session::builder::SessionBuilder;
use ort::value::Tensor;

// ort errors hold non-Send builder state; errors never leave the ASR thread anyway.
pub type Error = Box<dyn std::error::Error>;

/// Mel-spectrogram preprocessor from onnx-asr (MIT), matches NeMo's 128-bin features.
const PREPROCESSOR: &[u8] = include_bytes!("../assets/nemo128.onnx");
const MAX_TOKENS_PER_STEP: usize = 10;
const ENC_DIM: usize = 1024;
const STATE_SIZE: usize = 2 * 640;

pub const MODEL_FILES: [&str; 4] =
    ["encoder-model.onnx", "encoder-model.onnx.data", "decoder_joint-model.onnx", "vocab.txt"];

pub struct Parakeet {
    pre: Session,
    enc: Session,
    dec: Session,
    vocab: Vec<String>,
    blank: usize,
}

/// How long the two halves of a transcription took, in seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Timing {
    pub encode: f32,
    pub decode: f32,
}

fn cpu_builder() -> Result<SessionBuilder, Error> {
    // One thread, no pool: nothing left spinning or idling in the background.
    Ok(Session::builder()?.with_intra_threads(1)?.with_inter_threads(1)?)
}

impl Parakeet {
    pub fn load(dir: &Path) -> Result<Self, Error> {
        let vocab_txt = std::fs::read_to_string(dir.join("vocab.txt"))?;
        let mut vocab = Vec::new();
        for line in vocab_txt.lines().filter(|l| !l.is_empty()) {
            let (tok, id) = line.rsplit_once(' ').ok_or("bad vocab line")?;
            let id: usize = id.parse()?;
            if vocab.len() <= id {
                vocab.resize(id + 1, String::new());
            }
            vocab[id] = tok.replace('\u{2581}', " ");
        }
        let blank = vocab.iter().position(|t| t == "<blk>").ok_or("no <blk> in vocab")?;

        let pre = cpu_builder()?.commit_from_memory(PREPROCESSOR)?;
        let enc = cpu_builder()?
            .with_execution_providers([DirectML::default().build()])?
            .with_memory_pattern(false)?
            .commit_from_file(dir.join("encoder-model.onnx"))?;
        let dec = cpu_builder()?.commit_from_file(dir.join("decoder_joint-model.onnx"))?;
        Ok(Self { pre, enc, dec, vocab, blank })
    }

    /// `audio`: mono 16 kHz samples in [-1, 1]. `progress`, if given, is kept up to date while
    /// this runs (paced by `expect`, see `Pace`) and cleared when it's done.
    pub fn transcribe(&mut self, audio: &[f32], progress: Option<(&SharedProgress, Timing)>) -> Result<(String, Timing), Error> {
        let report = |f: &dyn Fn(&mut Option<Progress>)| {
            if let Some(Ok(mut p)) = progress.map(|(p, _)| p.lock()) {
                f(&mut p);
            }
        };
        let started = Instant::now();
        if let Some((_, expect)) = progress {
            let (enc, dec) = (expect.encode.max(0.01), expect.decode.max(0.01));
            report(&|p| *p = Some(Progress { start: started, encode_s: enc, encode_share: enc / (enc + dec), decoded: None }));
        }
        let result = self.run(audio, &report);
        report(&|p| *p = None);
        let (text, decode_start) = result?;
        let timing = Timing { encode: (decode_start - started).as_secs_f32(), decode: decode_start.elapsed().as_secs_f32() };
        Ok((text, timing))
    }

    /// The pipeline; returns the text and when decoding started.
    fn run(&mut self, audio: &[f32], report: &dyn Fn(&dyn Fn(&mut Option<Progress>))) -> Result<(String, Instant), Error> {
        // 1. features [1, 128, T]
        let out = self.pre.run(ort::inputs![
            "waveforms" => Tensor::from_array(([1usize, audio.len()], audio.to_vec()))?,
            "waveforms_lens" => Tensor::from_array(([1usize], vec![audio.len() as i64]))?,
        ])?;
        let (fshape, feats) = out["features"].try_extract_tensor::<f32>()?;
        let fshape: Vec<usize> = fshape.iter().map(|&d| d as usize).collect();
        let (_, flen) = out["features_lens"].try_extract_tensor::<i64>()?;
        let (feats, flen) = (feats.to_vec(), flen[0]);
        drop(out);

        // 2. encoder -> [1, 1024, T']
        let out = self.enc.run(ort::inputs![
            "audio_signal" => Tensor::from_array((fshape, feats))?,
            "length" => Tensor::from_array(([1usize], vec![flen]))?,
        ])?;
        let (eshape, enc) = out["outputs"].try_extract_tensor::<f32>()?;
        let frames = eshape[2] as usize;
        let (_, elen) = out["encoded_lengths"].try_extract_tensor::<i64>()?;
        let len = (elen[0].max(0) as usize).min(frames);
        // Transpose to one contiguous 1024-vector per frame.
        let mut by_frame = vec![0f32; frames * ENC_DIM];
        for d in 0..ENC_DIM {
            for t in 0..frames {
                by_frame[t * ENC_DIM + d] = enc[d * frames + t];
            }
        }
        drop(out);

        // 3. greedy TDT decoding
        let decode_start = Instant::now();
        report(&|p| p.iter_mut().for_each(|p| p.decoded = Some(0.0)));
        let mut reported = 0usize;
        let vocab_size = self.vocab.len();
        let mut s1 = vec![0f32; STATE_SIZE];
        let mut s2 = vec![0f32; STATE_SIZE];
        let mut tokens: Vec<usize> = Vec::new();
        let (mut t, mut emitted) = (0usize, 0usize);
        while t < len {
            let prev = tokens.last().copied().unwrap_or(self.blank) as i32;
            let out = self.dec.run(ort::inputs![
                "encoder_outputs" => Tensor::from_array(([1usize, ENC_DIM, 1], by_frame[t * ENC_DIM..(t + 1) * ENC_DIM].to_vec()))?,
                "targets" => Tensor::from_array(([1usize, 1], vec![prev]))?,
                "target_length" => Tensor::from_array(([1usize], vec![1i32]))?,
                "input_states_1" => Tensor::from_array(([2usize, 1, 640], s1.clone()))?,
                "input_states_2" => Tensor::from_array(([2usize, 1, 640], s2.clone()))?,
            ])?;
            let (_, logits) = out["outputs"].try_extract_tensor::<f32>()?;
            let token = argmax(&logits[..vocab_size]);
            let step = argmax(&logits[vocab_size..]); // duration index == frames to skip

            if token != self.blank {
                s1.copy_from_slice(out["output_states_1"].try_extract_tensor::<f32>()?.1);
                s2.copy_from_slice(out["output_states_2"].try_extract_tensor::<f32>()?.1);
                tokens.push(token);
                emitted += 1;
            }
            if step > 0 {
                t += step;
                emitted = 0;
            } else if token == self.blank || emitted == MAX_TOKENS_PER_STEP {
                t += 1;
                emitted = 0;
            }
            if (t - reported) * 50 >= len {
                // every 2%
                reported = t;
                let f = t.min(len) as f32 / len as f32;
                report(&|p| p.iter_mut().for_each(|p| p.decoded = Some(f)));
            }
        }

        let joined: String = tokens.iter().map(|&i| self.vocab[i].as_str()).collect();
        Ok((fix_spaces(&joined), decode_start))
    }
}

/// How far along a transcription is, for the overlay's progress bar. The encoder is one opaque
/// GPU call, so its share is paced by how long it took last time; decoding reports real progress.
pub struct Progress {
    start: Instant,
    /// Expected encoder time, and its share of the whole.
    encode_s: f32,
    encode_share: f32,
    /// Fraction of frames decoded, once decoding has started.
    decoded: Option<f32>,
}

pub type SharedProgress = Mutex<Option<Progress>>;

impl Progress {
    /// 0..1. While encoding it follows the clock but eases off near the encoder's share rather
    /// than stalling or overrunning if this run is slower than expected.
    pub fn fraction(&self) -> f32 {
        if let Some(d) = self.decoded {
            return self.encode_share + (1.0 - self.encode_share) * d;
        }
        let x = self.start.elapsed().as_secs_f32() / self.encode_s.max(0.01);
        let x = if x < 0.8 { x } else { 0.8 + 0.2 * (1.0 - (-(x - 0.8) / 0.2).exp()) };
        self.encode_share * x
    }
}

/// Predicts how long each stage will take for a clip, for the progress bar. Each stage is fitted
/// as `time = overhead + rate * seconds of audio` to this machine's recent transcriptions (newest
/// weighted most), starting from typical numbers. Kept in pace.txt so it survives the model
/// being unloaded and the app restarting.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pace {
    /// (seconds of audio, timing), oldest first.
    runs: Vec<(f32, Timing)>,
}

const PACE_RUNS: usize = 40;
/// Each older run counts this much less than the one after it.
const PACE_DECAY: f32 = 0.85;
/// A run this many times longer or shorter than the clip being predicted counts 40% less.
const PACE_SPREAD: f32 = 3.0;
/// A typical machine per stage, (overhead s, s per s of audio), measured on a mid-range GPU; its
/// shape is assumed (scaled to this machine) until there are runs near a length, worth this many.
const PRIOR_ENCODE: (f32, f32) = (0.2, 0.0055);
const PRIOR_DECODE: (f32, f32) = (0.005, 0.0067);
const PRIOR_WEIGHT: f32 = 1.0;

impl Pace {
    fn path() -> std::path::PathBuf {
        crate::win::data_dir().join("pace.txt")
    }

    pub fn load() -> Self {
        Self::parse(&std::fs::read_to_string(Self::path()).unwrap_or_default())
    }

    pub fn save(&self) {
        let _ = std::fs::write(Self::path(), self.to_text());
    }

    fn parse(text: &str) -> Self {
        let runs = text
            .lines()
            .filter_map(|l| {
                let v: Vec<f32> = l.split_whitespace().filter_map(|x| x.parse().ok()).collect();
                let run = (v.len() == 3).then(|| (v[0], Timing { encode: v[1], decode: v[2] }))?;
                plausible(run.0, run.1).then_some(run)
            })
            .collect::<Vec<_>>();
        let skip = runs.len().saturating_sub(PACE_RUNS);
        Self { runs: runs[skip..].to_vec() }
    }

    fn to_text(&self) -> String {
        self.runs.iter().map(|(s, t)| format!("{s:.2} {:.4} {:.4}\n", t.encode, t.decode)).collect()
    }

    pub fn record(&mut self, secs: f32, t: Timing) {
        if !plausible(secs, t) {
            return;
        }
        self.runs.push((secs, t));
        if self.runs.len() > PACE_RUNS {
            self.runs.remove(0);
        }
    }

    pub fn predict(&self, secs: f32) -> Timing {
        let n = self.runs.len();
        let fit = |stage: fn(&Timing) -> f32, prior: (f32, f32)| {
            let recency = |i: usize| PACE_DECAY.powi((n - 1 - i) as i32);
            // The typical curve, scaled by how much faster or slower this machine has been lately.
            let scale = {
                let (mut got, mut want) = (0.0, 0.0);
                for (i, (x, t)) in self.runs.iter().enumerate() {
                    got += recency(i) * stage(t);
                    want += recency(i) * (prior.0 + prior.1 * x);
                }
                if want > 0.0 { (got / want).clamp(0.1, 10.0) } else { 1.0 }
            };
            let typical = |x: f32| scale * (prior.0 + prior.1 * x);
            // Then a line fitted near this length (runs much shorter or longer count less, since
            // the encoder isn't quite linear), on relative error so short clips matter as much as
            // long ones. The typical curve enters as one run either side, so a couple of odd runs
            // can't swing it wildly.
            let near = |x: f32| (-(x / secs).ln().powi(2) / (2.0 * PACE_SPREAD.ln().powi(2))).exp();
            let mut pts: Vec<(f32, f32, f32)> = [secs / 2.0, secs * 2.0].iter().map(|&x| (x, typical(x), PRIOR_WEIGHT / 2.0)).collect();
            pts.extend(self.runs.iter().enumerate().map(|(i, (x, t))| (*x, stage(t), recency(i) * near(*x))));
            pts.iter_mut().for_each(|p| p.2 /= typical(p.0).max(1e-3).powi(2));
            let (a, b) = weighted_line(&pts);
            (a + b * secs).max(0.0)
        };
        Timing { encode: fit(|t| t.encode, PRIOR_ENCODE), decode: fit(|t| t.decode, PRIOR_DECODE) }
    }
}

/// Whether a run says something about this machine's speed. Too-short clips are all overhead,
/// and a run slower than realtime by half a minute was interrupted (the PC slept, say); one of
/// those would leave the bar crawling for weeks.
fn plausible(secs: f32, t: Timing) -> bool {
    let ok = |v: f32| v.is_finite() && v >= 0.0;
    ok(secs) && ok(t.encode) && ok(t.decode) && secs >= 0.5 && t.encode + t.decode < secs + 30.0
}

/// Least-squares `y = a + b x` over (x, y, weight), with neither term allowed below zero.
fn weighted_line(pts: &[(f32, f32, f32)]) -> (f32, f32) {
    let sw: f32 = pts.iter().map(|p| p.2).sum();
    let mx = pts.iter().map(|p| p.2 * p.0).sum::<f32>() / sw;
    let my = pts.iter().map(|p| p.2 * p.1).sum::<f32>() / sw;
    let sxx: f32 = pts.iter().map(|p| p.2 * (p.0 - mx).powi(2)).sum();
    let sxy: f32 = pts.iter().map(|p| p.2 * (p.0 - mx) * (p.1 - my)).sum();
    let b = if sxx > 1e-6 { sxy / sxx } else { 0.0 };
    if b < 0.0 {
        return (my.max(0.0), 0.0);
    }
    let a = my - b * mx;
    if a < 0.0 {
        // Through the origin instead.
        let b = pts.iter().map(|p| p.2 * p.0 * p.1).sum::<f32>() / pts.iter().map(|p| p.2 * p.0 * p.0).sum::<f32>().max(1e-6);
        return (0.0, b.max(0.0));
    }
    (a, b)
}

fn argmax(v: &[f32]) -> usize {
    v.iter().enumerate().fold((0, f32::NEG_INFINITY), |b, (i, &x)| if x > b.1 { (i, x) } else { b }).0
}

/// Same as onnx-asr's `\A\s|\s\B|(\s)\b` rule: keep a space only if a word character follows it.
fn fix_spaces(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    for (i, &c) in chars.iter().enumerate() {
        if c.is_whitespace() {
            let next_is_word = chars.get(i + 1).is_some_and(|n| n.is_alphanumeric() || *n == '_');
            if i == 0 || !next_is_word {
                continue;
            }
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spaces() {
        assert_eq!(fix_spaces(" Hello , world . It's"), "Hello, world. It's");
    }

    fn total(t: Timing) -> f32 {
        t.encode + t.decode
    }

    #[test]
    fn pace_learns_a_slow_machine() {
        // Three times slower than the prior, plus a fixed 0.4 s of encoder overhead.
        let machine = |x: f32| Timing { encode: 0.4 + 0.015 * x, decode: 0.02 * x };
        let mut pace = Pace::default();
        let before = total(pace.predict(30.0)) / total(machine(30.0));
        for x in [3.0, 8.0, 20.0, 5.0, 45.0, 12.0, 90.0, 2.0] {
            pace.record(x, machine(x));
        }
        for x in [1.5, 4.0, 15.0, 60.0, 150.0] {
            let ratio = total(pace.predict(x)) / total(machine(x));
            assert!((ratio - 1.0).abs() < 0.15, "{x}s: predicted {ratio:.2}x the real time (untrained: {before:.2}x)");
        }
    }

    #[test]
    fn pace_follows_a_change_and_ignores_junk() {
        let mut pace = Pace::default();
        for _ in 0..20 {
            pace.record(10.0, Timing { encode: 1.0, decode: 1.0 });
        }
        // The machine got faster (say the GPU was busy before): recent runs win.
        for _ in 0..8 {
            pace.record(10.0, Timing { encode: 0.2, decode: 0.1 });
        }
        assert!(total(pace.predict(10.0)) < 1.0, "{:?}", pace.predict(10.0));
        pace.record(0.1, Timing { encode: 50.0, decode: 50.0 }); // too short to learn from
        pace.record(10.0, Timing { encode: f32::NAN, decode: 0.1 });
        pace.record(10.0, Timing { encode: 3600.0, decode: 0.1 }); // the PC slept mid-transcription
        assert!(total(pace.predict(10.0)) < 1.0);
    }

    #[test]
    fn pace_roundtrips_and_caps() {
        let mut pace = Pace::default();
        for i in 0..(PACE_RUNS + 5) {
            pace.record(1.0 + i as f32, Timing { encode: 0.25, decode: 0.0125 });
        }
        assert_eq!(pace.runs.len(), PACE_RUNS);
        assert_eq!(Pace::parse(&pace.to_text()), pace);
        assert_eq!(Pace::parse("garbage\n1 2\n-1 0.2 0.1\n10 4000 1\n"), Pace::default());
        let p = Pace::default().predict(0.0);
        assert!(p.encode > 0.0 && p.decode >= 0.0);
    }
}

/// End-to-end check against a real model + 16-bit PCM wav:
/// `FLOWE_WAV=clip.wav cargo test --release -- --ignored --nocapture`
#[cfg(test)]
mod e2e {
    #[test]
    #[ignore]
    fn transcribe_wav() {
        let path = std::env::var("FLOWE_WAV").expect("set FLOWE_WAV");
        let (pcm, rate) = crate::audio::read_wav_pcm16(std::path::Path::new(&path)).unwrap();
        let audio = crate::audio::resample(&pcm, rate, 16000);
        let mut m = super::Parakeet::load(&crate::win::data_dir().join("model")).unwrap();
        m.transcribe(&audio[..16000], None).unwrap(); // warm-up
        let t = std::time::Instant::now();
        let (text, _) = m.transcribe(&audio, None).unwrap();
        println!("{rate} Hz, {:.1}s audio -> {:.3}s: {text:?}", pcm.len() as f32 / rate as f32, t.elapsed().as_secs_f32());
        assert!(text.to_lowercase().contains("local dictation"));
    }

    /// How long each stage takes for clips of different lengths, and how well `Pace` predicts it
    /// after learning from the earlier clips: `FLOWE_WAV=long.wav cargo test --release pace_bench -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn pace_bench() {
        let path = std::env::var("FLOWE_WAV").expect("set FLOWE_WAV");
        let (pcm, rate) = crate::audio::read_wav_pcm16(std::path::Path::new(&path)).unwrap();
        let audio = crate::audio::resample(&pcm, rate, 16000);
        let mut m = super::Parakeet::load(&crate::win::data_dir().join("model")).unwrap();
        m.transcribe(&audio[..16000], None).unwrap(); // warm-up
        let mut pace = super::Pace::default();
        for secs in [2, 4, 7, 12, 20, 35, 60, 90, 150, 240, 400, 3, 10, 30, 120] {
            let n = (secs * 16000).min(audio.len());
            let guess = pace.predict(n as f32 / 16000.0);
            let (_, t) = m.transcribe(&audio[..n], None).unwrap();
            println!(
                "{:>5.1}s audio: encode {:>5.0}ms decode {:>5.0}ms | predicted {:>5.0} {:>5.0} ({:+.0}% total)",
                n as f32 / 16000.0,
                t.encode * 1e3,
                t.decode * 1e3,
                guess.encode * 1e3,
                guess.decode * 1e3,
                ((guess.encode + guess.decode) / (t.encode + t.decode) - 1.0) * 100.0
            );
            pace.record(n as f32 / 16000.0, t);
        }
    }
}
