//! What the overlay draws, independent of any window: the idle dash, the recording pill (state
//! indicator on the left, visualization on the right) and the transcribing dots. The overlay
//! presents these frames on screen; `cargo run --bin demo` renders them to the README's GIFs.

use tiny_skia::{Mask, Pixmap, Transform};

use crate::draw::{capsule, circle, fill, rect};
use crate::viz::{Area, Style, Viz};

/// Animation rate (see TIMER_ANIM in main.rs).
pub const FPS: f32 = 60.0;
/// Pill size in logical pixels (multiplied by the DPI scale).
pub const SIZE: (f32, f32) = (168.0, 34.0);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Idle {
    Loading,
    Ready,
    Error,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    /// Paused: nothing on screen.
    Hidden,
    /// The small dash.
    Idle(Idle),
    Recording,
    Locked,
    Transcribing,
    /// Nothing was heard: the dash flashes amber, then returns to idle.
    Flash,
}

const BG: [u8; 4] = [20, 20, 23, 245];
const WELL: [u8; 4] = [10, 10, 12, 255];
const FG: [u8; 4] = [250, 250, 250, 255];
const DIM: [u8; 4] = [113, 113, 122, 255];
const RED: [u8; 4] = [239, 68, 68, 255];
const AMBER: [u8; 4] = [245, 158, 11, 255];

pub struct Pill {
    pub w: i32,
    pub h: i32,
    s: f32,
    pub pixmap: Pixmap,
    /// The inset the visualization draws into, and its rounded clip.
    area: Area,
    mask: Mask,
    pub viz: Viz,
}

impl Pill {
    /// `s` is the DPI scale (1.0 at 96 dpi).
    pub fn new(s: f32, style: Style) -> Option<Self> {
        let (w, h) = ((SIZE.0 * s) as i32, (SIZE.1 * s) as i32);
        // Left: a square slot for the state indicator. Right: the visualization inset.
        let (wf, hf) = (w as f32, h as f32);
        let pad = 3.0 * s; // thin bezel around the visualization
        let ax = hf * 0.80;
        let area = Area { x: ax, y: pad, w: wf - ax - pad, h: hf - 2.0 * pad };
        let mut mask = Mask::new(w as u32, h as u32)?;
        mask.fill_path(&capsule(area.x, area.y, area.w, area.h), tiny_skia::FillRule::Winding, true, Transform::identity());
        Some(Self { w, h, s, pixmap: Pixmap::new(w as u32, h as u32)?, area, mask, viz: Viz::new(style) })
    }

    /// Draws one frame of `view` into `pixmap`. `level` is mic loudness (advances the
    /// visualization by one 1/FPS step); `tick` counts frames since the view was entered.
    pub fn paint(&mut self, view: View, level: f32, tick: u32) {
        let (w, h, s) = (self.w as f32, self.h as f32, self.s);
        self.pixmap.fill(tiny_skia::Color::TRANSPARENT);
        let px = &mut self.pixmap;

        match view {
            View::Hidden => {}
            View::Idle(_) | View::Flash => {
                let color = match view {
                    View::Flash => AMBER,
                    View::Idle(Idle::Ready) => [250, 250, 250, 150],
                    View::Idle(Idle::Loading) => [113, 113, 122, 150],
                    _ => [245, 158, 11, 200], // Idle(Error)
                };
                fill(px, capsule(w / 2.0 - 18.0 * s, h - 5.0 * s, 36.0 * s, 5.0 * s), color);
            }
            View::Recording | View::Locked => {
                self.viz.update(level, 1.0 / FPS);
                fill(px, capsule(0.0, 0.0, w, h), BG);
                let a = self.area;
                fill(px, capsule(a.x, a.y, a.w, a.h), WELL);
                self.viz.render(px, &self.mask, a, s);
                // State indicator, centred in the left slot.
                let (cx, cy) = (a.x / 2.0 + 0.5 * s, h / 2.0);
                if view == View::Recording {
                    let halo = 3.5 * s + 3.5 * s * self.viz.energy;
                    fill(px, circle(cx, cy, halo), [239, 68, 68, (40.0 + 60.0 * self.viz.energy) as u8]);
                    fill(px, circle(cx, cy, 3.5 * s), RED);
                } else {
                    let (hole, k) = ([20, 20, 23, 255], 0.75 * s);
                    fill(px, capsule(cx - 3.6 * k, cy - 8.0 * k, 7.2 * k, 10.0 * k), AMBER); // shackle
                    fill(px, capsule(cx - 2.0 * k, cy - 6.4 * k, 4.0 * k, 8.0 * k), hole);
                    fill(px, rect(cx - 5.0 * k, cy - 2.5 * k, 10.0 * k, 8.0 * k), AMBER); // body
                }
            }
            View::Transcribing => {
                let (pw, ph) = (96.0 * s, h);
                let (x0, y0) = ((w - pw) / 2.0, (h - ph) / 2.0);
                fill(px, capsule(x0, y0, pw, ph), BG);
                for i in 0..3 {
                    let on = ((tick / 12) as i32 - i).rem_euclid(3) == 0;
                    fill(px, circle(w / 2.0 + (i - 1) as f32 * 11.0 * s, h / 2.0, 3.0 * s), if on { FG } else { DIM });
                }
            }
        }
    }
}

/// Speech-like loudness for previews: bursts of syllables with pauses between phrases.
pub fn demo_level(t: f32) -> f32 {
    let syllables = (t * 9.0).sin().abs();
    let phrase = (0.5 + 0.5 * (t * 1.7).sin()).powi(2);
    0.005 + 0.09 * syllables * phrase
}
