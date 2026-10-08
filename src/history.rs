//! Recording history (%LOCALAPPDATA%\Flowe\history.tsv) and the usage stats derived from it.
//! Plain tab-separated text so it stays readable and greppable.

use std::io::Write;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// Local time, "YYYY-MM-DD HH:MM:SS".
    pub when: String,
    pub audio_ms: u32,
    pub latency_ms: u32,
    pub words: u32,
    pub text: String,
}

impl Entry {
    pub fn new(text: String, audio_ms: u32, latency_ms: u32) -> Self {
        let words = text.split_whitespace().count() as u32;
        Self { when: crate::win::local_time(), audio_ms, latency_ms, words, text }
    }

    fn line(&self) -> String {
        let text = self.text.replace(['\t', '\n', '\r'], " ");
        format!("{}\t{}\t{}\t{}\t{}", self.when, self.audio_ms, self.latency_ms, self.words, text)
    }

    fn parse(line: &str) -> Option<Self> {
        let mut it = line.splitn(5, '\t');
        Some(Self {
            when: it.next()?.to_string(),
            audio_ms: it.next()?.parse().ok()?,
            latency_ms: it.next()?.parse().ok()?,
            words: it.next()?.parse().ok()?,
            text: it.next()?.to_string(),
        })
    }
}

pub struct History {
    path: PathBuf,
    pub entries: Vec<Entry>,
}

impl History {
    pub fn load() -> Self {
        let path = crate::win::data_dir().join("history.tsv");
        let entries = std::fs::read_to_string(&path).unwrap_or_default().lines().filter_map(Entry::parse).collect();
        Self { path, entries }
    }

    pub fn push(&mut self, e: Entry) {
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&self.path) {
            let _ = writeln!(f, "{}", e.line());
        }
        self.entries.push(e);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        let _ = std::fs::remove_file(&self.path);
    }

    pub fn stats(&self) -> Stats {
        let now = crate::win::local_time();
        let today = &now[..10];
        let mut st = Stats::default();
        for e in &self.entries {
            st.recordings += 1;
            st.words += e.words as u64;
            st.audio_ms += e.audio_ms as u64;
            st.latency_ms += e.latency_ms as u64;
            if e.when.starts_with(today) {
                st.today_recordings += 1;
                st.today_words += e.words as u64;
            }
        }
        st
    }
}

#[derive(Default, Debug, PartialEq, Eq)]
pub struct Stats {
    pub recordings: u64,
    pub words: u64,
    pub audio_ms: u64,
    pub latency_ms: u64,
    pub today_recordings: u64,
    pub today_words: u64,
}

impl Stats {
    /// Dictation pace: words per minute of speech.
    pub fn wpm(&self) -> f64 {
        if self.audio_ms > 0 { self.words as f64 / (self.audio_ms as f64 / 60_000.0) } else { 0.0 }
    }

    pub fn avg_latency_s(&self) -> f64 {
        if self.recordings > 0 { self.latency_ms as f64 / self.recordings as f64 / 1000.0 } else { 0.0 }
    }

    /// Seconds of speech transcribed per second of compute.
    pub fn speed(&self) -> f64 {
        if self.latency_ms > 0 { self.audio_ms as f64 / self.latency_ms as f64 } else { 0.0 }
    }
}

pub fn fmt_duration(ms: u64) -> String {
    let s = ms.div_ceil(1000);
    match (s / 3600, s / 60 % 60, s % 60) {
        (0, 0, s) => format!("{s}s"),
        (0, m, s) => format!("{m}m {s:02}s"),
        (h, m, _) => format!("{h}h {m:02}m"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_sanitize() {
        let mut e = Entry { when: "2026-10-07 15:40:12".into(), audio_ms: 7500, latency_ms: 390, words: 3, text: "a\tb\nc".into() };
        let back = Entry::parse(&e.line()).unwrap();
        e.text = "a b c".into();
        assert_eq!(back, e);
        assert_eq!(Entry::parse("garbage"), None);
    }

    #[test]
    fn durations() {
        assert_eq!(fmt_duration(900), "1s");
        assert_eq!(fmt_duration(65_000), "1m 05s");
        assert_eq!(fmt_duration(3_725_000), "1h 02m");
    }
}
