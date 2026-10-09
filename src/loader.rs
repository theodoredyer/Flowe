//! What the pill shows while the speech model works ("Loading" in the dashboard). The coloured
//! ones borrow this recording's plasma palette, so the loader carries on from the visualization.

use tiny_skia::{Mask, Pixmap};

use crate::draw::{capsule, circle, fill, fill_masked};
use crate::viz::{Area, Viz, shade, smoothstep};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Loader {
    /// Three dots riding a soft wave.
    Dots,
    /// A glowing comet circling the inside of the pill.
    Comet,
    /// Slowly flowing colour with a bright sheen sweeping across.
    Shimmer,
    /// A bar that fills with the transcription's real progress.
    Progress,
}

const WELL: [u8; 4] = [10, 10, 12, 255];
const FG: [f32; 3] = [250.0, 250.0, 250.0];
const DIM: [f32; 3] = [113.0, 113.0, 122.0];

impl Loader {
    pub const ALL: [Loader; 4] = [Loader::Dots, Loader::Comet, Loader::Shimmer, Loader::Progress];

    pub fn label(self) -> &'static str {
        match self {
            Loader::Dots => "Dots",
            Loader::Comet => "Comet",
            Loader::Shimmer => "Shimmer",
            Loader::Progress => "Progress",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Loader::Dots => "dots",
            Loader::Comet => "comet",
            Loader::Shimmer => "shimmer",
            Loader::Progress => "progress",
        }
    }

    pub fn from_key(k: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|l| l.key() == k)
    }

    /// Width of the transcribing pill in logical pixels (its height is the recording pill's).
    pub fn width(self) -> f32 {
        match self {
            Loader::Dots | Loader::Comet => 96.0,
            Loader::Shimmer => 112.0,
            Loader::Progress => 128.0,
        }
    }

    /// Draws into `well` (clipped by `mask`); the pill behind it is already painted.
    /// `t` is seconds since transcribing started, `progress` 0..1.
    pub fn draw(self, px: &mut Pixmap, mask: &Mask, a: Area, s: f32, t: f32, progress: f32, viz: &Viz) {
        let (cx, cy) = (a.x + a.w / 2.0, a.y + a.h / 2.0);
        match self {
            Loader::Dots => {
                for i in 0..3 {
                    let wave = 0.5 + 0.5 * (t * 7.5 - i as f32 * 0.9).sin();
                    let b = wave * wave * (3.0 - 2.0 * wave); // linger at the top and bottom
                    let c = [0, 1, 2].map(|j| DIM[j] + (FG[j] - DIM[j]) * b);
                    let x = cx + (i - 1) as f32 * 11.0 * s;
                    fill(px, circle(x, cy + 1.5 * s - 3.0 * s * b, 2.5 * s + 0.8 * s * b), [c[0] as u8, c[1] as u8, c[2] as u8, 255]);
                }
            }
            Loader::Comet => {
                fill(px, capsule(a.x, a.y, a.w, a.h), WELL);
                // Head runs round a stadium track inside the well; the tail trails behind it.
                let r = a.h / 2.0 - 3.5 * s;
                let straight = a.w - a.h;
                let perimeter = 2.0 * straight + 2.0 * std::f32::consts::PI * r;
                let head = (t * 0.85 + 0.04 * (t * 3.0).sin()).rem_euclid(1.0) * perimeter;
                let point = |d: f32| -> (f32, f32) {
                    let d = d.rem_euclid(perimeter);
                    let (l, arc) = (straight, std::f32::consts::PI * r);
                    let (x0, x1) = (cx - straight / 2.0, cx + straight / 2.0);
                    if d < l {
                        (x0 + d, cy - r)
                    } else if d < l + arc {
                        let th = (d - l) / r;
                        (x1 + r * th.sin(), cy - r * th.cos())
                    } else if d < 2.0 * l + arc {
                        (x1 - (d - l - arc), cy + r)
                    } else {
                        let th = (d - 2.0 * l - arc) / r;
                        (x0 - r * th.sin(), cy + r * th.cos())
                    }
                };
                const TAIL: usize = 70;
                for i in (0..TAIL).rev() {
                    let k = i as f32 / TAIL as f32;
                    let (x, y) = point(head - k * perimeter * 0.42);
                    let c = viz.tint(t * 0.15 + k * 0.5);
                    let alpha = (1.0 - k).powf(1.6);
                    fill_masked(px, circle(x, y, (2.6 - 1.6 * k) * s), rgba(c, alpha), Some(mask));
                }
                let (x, y) = point(head);
                fill_masked(px, circle(x, y, 5.5 * s), rgba(viz.tint(t * 0.15), 0.22), Some(mask));
                fill_masked(px, circle(x, y, 2.2 * s), [255, 255, 255, 230], Some(mask));
            }
            Loader::Shimmer => {
                let sweep = (t / 1.3).fract();
                let band_x = a.x - 0.35 * a.w + sweep * 1.7 * a.w;
                shade(px, mask, a, |x, y| {
                    let u = (x - a.x) / a.w;
                    let base = viz.tint(u * 0.6 - t * 0.12 + 0.08 * ((y - a.y) / a.h));
                    let d = (x - band_x - (y - cy) * 0.8) / (9.0 * s);
                    let band = (-d * d).exp() * 0.8;
                    let breathe = 0.55 + 0.1 * (t * 2.4).sin();
                    (base.map(|c| c * breathe + (1.0 - c * breathe) * band), 1.0)
                });
            }
            Loader::Progress => {
                fill(px, capsule(a.x, a.y, a.w, a.h), WELL);
                fill_masked(px, capsule(a.x, a.y, a.w, a.h), [255, 255, 255, 14], Some(mask));
                // Never quite empty, so there's always a rounded cap of colour to grow from.
                let edge = a.x + a.h * 0.6 + progress.clamp(0.0, 1.0) * (a.w - a.h * 0.6);
                let glint_x = a.x + ((t / 1.1).fract() * 1.4 - 0.2) * a.w;
                shade(px, mask, a, |x, _| {
                    let alpha = smoothstep(edge + 0.8 * s, edge - 0.8 * s, x);
                    if alpha <= 0.0 {
                        return ([0.0; 3], 0.0);
                    }
                    let c = viz.tint((x - a.x) / a.w * 0.7 - t * 0.35);
                    let head = (-((edge - x) / (5.0 * s)).powi(2)).exp() * 0.55;
                    let glint = (-((x - glint_x) / (6.0 * s)).powi(2)).exp() * 0.35;
                    let lift = (head + glint).min(1.0);
                    (c.map(|v| 0.15 + v * 0.85 + (1.0 - v) * lift), alpha)
                });
            }
        }
    }
}

fn rgba(c: [f32; 3], alpha: f32) -> [u8; 4] {
    [(c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8, (alpha.clamp(0.0, 1.0) * 255.0) as u8]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_roundtrip() {
        for l in Loader::ALL {
            assert_eq!(Loader::from_key(l.key()), Some(l));
        }
        assert_eq!(Loader::from_key("nope"), None);
    }
}
