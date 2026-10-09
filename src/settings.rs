//! User preferences in %LOCALAPPDATA%\Flowe\settings.txt (plain `key=value` lines).

use crate::loader::Loader;
use crate::viz::Style;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Settings {
    pub style: Style,
    pub sounds: bool,
    /// What the pill shows while transcribing.
    pub loader: Loader,
}

impl Default for Settings {
    fn default() -> Self {
        Self { style: Style::Waves, sounds: true, loader: Loader::Progress }
    }
}

impl Settings {
    pub fn load() -> Self {
        let text = std::fs::read_to_string(crate::win::data_dir().join("settings.txt")).unwrap_or_default();
        Self::parse(&text)
    }

    pub fn save(&self) {
        let _ = std::fs::write(crate::win::data_dir().join("settings.txt"), self.to_text());
    }

    fn parse(text: &str) -> Self {
        let mut s = Self::default();
        for line in text.lines() {
            match line.split_once('=').map(|(k, v)| (k.trim(), v.trim())) {
                Some(("style", v)) => s.style = Style::from_key(v).unwrap_or(s.style),
                Some(("sounds", v)) => s.sounds = v != "off",
                Some(("loader", v)) => s.loader = Loader::from_key(v).unwrap_or(s.loader),
                _ => {}
            }
        }
        s
    }

    fn to_text(self) -> String {
        format!("style={}\nsounds={}\nloader={}\n", self.style.key(), if self.sounds { "on" } else { "off" }, self.loader.key())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_defaults() {
        let s = Settings { style: Style::Lava, sounds: false, loader: Loader::Comet };
        assert_eq!(Settings::parse(&s.to_text()), s);
        assert_eq!(Settings::parse(""), Settings::default());
        assert_eq!(Settings::parse("style=bogus\ngarbage\n"), Settings::default());
    }
}
