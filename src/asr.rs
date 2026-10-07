//! Parakeet TDT (NeMo) inference on ONNX Runtime.
//!
//! Pipeline (same as onnx-asr): waveform -> log-mel (nemo128.onnx) -> conformer encoder
//! (GPU via DirectML) -> greedy TDT decoding with the decoder/joint network (CPU, tiny).

use std::path::Path;

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

    /// `audio`: mono 16 kHz samples in [-1, 1].
    pub fn transcribe(&mut self, audio: &[f32]) -> Result<String, Error> {
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
        }

        let joined: String = tokens.iter().map(|&i| self.vocab[i].as_str()).collect();
        Ok(fix_spaces(&joined))
    }
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
}

/// End-to-end check against a real model + 16-bit PCM wav:
/// `PARAKEY_WAV=clip.wav cargo test --release -- --ignored --nocapture`
#[cfg(test)]
mod e2e {
    #[test]
    #[ignore]
    fn transcribe_wav() {
        let path = std::env::var("PARAKEY_WAV").expect("set PARAKEY_WAV");
        let (pcm, rate) = crate::audio::read_wav_pcm16(std::path::Path::new(&path)).unwrap();
        let audio = crate::audio::resample(&pcm, rate, 16000);
        let mut m = super::Parakeet::load(&crate::win::data_dir().join("model")).unwrap();
        m.transcribe(&audio[..16000]).unwrap(); // warm-up
        let t = std::time::Instant::now();
        let text = m.transcribe(&audio).unwrap();
        println!("{rate} Hz, {:.1}s audio -> {:.3}s: {text:?}", pcm.len() as f32 / rate as f32, t.elapsed().as_secs_f32());
        assert!(text.to_lowercase().contains("local dictation"));
    }
}
