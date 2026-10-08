//! Recording visualizations for the overlay pill. Each style turns mic loudness into motion;
//! all of them run only while the pill is animating (recording or previewing).

use tiny_skia::{Color, GradientStop, LinearGradient, Mask, Paint, Path, PathBuilder, Pixmap, Point, PremultipliedColorU8, SpreadMode, Stroke, Transform};

use crate::draw::{capsule, circle, fill_masked};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Style {
    /// Scrolling level bars (the original look).
    Waves,
    /// 1D water simulation: springs between columns, splashes, droplets.
    Liquid,
    /// Flowing color field that speeds up and gains contrast while you talk.
    Plasma,
    /// Glowing metaballs that merge, grow and brighten with your voice.
    Lava,
}

impl Style {
    pub const ALL: [Style; 4] = [Style::Waves, Style::Liquid, Style::Plasma, Style::Lava];

    pub fn label(self) -> &'static str {
        match self {
            Style::Waves => "Waves",
            Style::Liquid => "Liquid",
            Style::Plasma => "Plasma",
            Style::Lava => "Lava lamp",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Style::Waves => "waves",
            Style::Liquid => "liquid",
            Style::Plasma => "plasma",
            Style::Lava => "lava",
        }
    }

    pub fn from_key(k: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.key() == k)
    }
}

/// The inset rectangle (pixmap coordinates) the visualization draws into.
#[derive(Clone, Copy)]
pub struct Area {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

struct Droplet {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
}

pub struct Viz {
    pub style: Style,
    /// Loudness 0..1: raw this frame, and smoothed (fast attack, slow release).
    target: f32,
    pub energy: f32,
    /// How sharply loudness just jumped (a syllable onset).
    onset: f32,
    /// Wall clock, and a pattern clock that runs faster while speaking.
    t: f32,
    phase: f32,
    frame: u32,
    bars: Vec<f32>,
    height: Vec<f32>,
    vel: Vec<f32>,
    fill: f32,
    drops: Vec<Droplet>,
    rng: u32,
    /// Background-noise level the gate subtracts (tracks the quietest recent input).
    floor: f32,
    /// Recent loudest voice level, for auto-scaling (so normal speech doesn't pin everything at max).
    peak: f32,
    /// Plasma colours for this recording: a cyclic 4-colour ramp (5th stop repeats the 1st), and
    /// the two random hues it was built from.
    palette: [[f32; 3]; 5],
    hues: (f32, f32),
}

/// Random plasma palette from two random hues: deep shade of the first, the first at full colour,
/// the second at full colour, then a pale tint between them. Saturation is random too.
fn random_palette(r: &mut impl FnMut() -> f32) -> ([[f32; 3]; 5], (f32, f32)) {
    let h0 = r();
    let h1 = (h0 + 0.18 + 0.5 * r()).rem_euclid(1.0); // far enough apart to read as two colours
    let sat = 0.65 + 0.3 * r();
    let mid = h0 + 0.5 * ((h1 - h0 + 0.5).rem_euclid(1.0) - 0.5); // halfway the short way round
    let dark = hsv(h0, sat, 0.16);
    (
        [dark, hsv(h0, sat, 0.85), hsv(h1, sat, 0.95), hsv(mid.rem_euclid(1.0), sat * 0.45, 1.0), dark],
        (h0, h1),
    )
}

fn hsv(h: f32, s: f32, v: f32) -> [f32; 3] {
    let f = |n: f32| {
        let k = (n + h * 6.0).rem_euclid(6.0);
        v - v * s * k.min(4.0 - k).clamp(0.0, 1.0)
    };
    [f(5.0), f(3.0), f(1.0)]
}

fn clock_seed() -> u32 {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos() ^ d.as_secs() as u32).unwrap_or(1);
    nanos | 1 // xorshift must not start at 0
}

impl Viz {
    pub fn new(style: Style) -> Self {
        Self {
            style,
            target: 0.0,
            energy: 0.0,
            onset: 0.0,
            t: 0.0,
            phase: 0.0,
            frame: 0,
            bars: Vec::new(),
            height: Vec::new(),
            vel: Vec::new(),
            fill: 0.3,
            drops: Vec::new(),
            rng: clock_seed(),
            floor: 0.04,
            peak: 0.03,
            palette: [[0.0; 3]; 5],
            hues: (0.0, 0.0),
        }
        .with_new_palette()
    }

    fn with_new_palette(mut self) -> Self {
        let (p, h) = random_palette(&mut || self.rand());
        self.palette = p;
        self.hues = h;
        self
    }

    /// Calm state for a fresh recording, with freshly picked random plasma colours.
    pub fn reset(&mut self) {
        let mut fresh = Self::new(self.style);
        fresh.rng = self.rng;
        fresh.floor = self.floor; // the room's noise level and your voice level carry over
        fresh.peak = self.peak;
        *self = fresh.with_new_palette();
    }

    pub fn update(&mut self, level: f32, dt: f32) {
        // Noise gate: follow the quietest input (drops instantly, creeps up slowly) and only
        // count what rises above it, so the visuals go still the moment you stop talking.
        if level < self.floor {
            self.floor = level;
        } else {
            self.floor = (self.floor + (level - self.floor) * 0.002).min(0.04);
        }
        let voice = (level - self.floor * 1.3 - 0.002).max(0.0);
        // Auto-scale to your voice: your recent loudest syllables reach ~90%, quieter ones stay
        // lower, so the visuals breathe with speech instead of sitting at max the whole time.
        // The peak decays over a few seconds and never drops below a floor (so silence isn't amplified).
        self.peak = (self.peak * 0.996).max(voice).max(0.03);
        self.target = (voice / self.peak * 0.9).min(1.0);
        let prev = self.energy;
        let k = if self.target > self.energy { 0.45 } else { 0.2 };
        self.energy += (self.target - self.energy) * k;
        self.onset = (self.target - prev).max(0.0);
        self.t += dt;
        // Patterns barely drift in silence and speed up a lot while you talk.
        self.phase += dt * (0.08 + 3.4 * self.energy);
        self.frame = self.frame.wrapping_add(1);
    }

    fn rand(&mut self) -> f32 {
        // xorshift32: plenty for visual jitter
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng >> 8) as f32 / (1u32 << 24) as f32
    }

    pub fn render(&mut self, px: &mut Pixmap, mask: &Mask, a: Area, s: f32) {
        match self.style {
            Style::Waves => self.waves(px, a, s),
            Style::Liquid => self.liquid(px, mask, a, s),
            Style::Plasma => self.plasma(px, mask, a, s),
            Style::Lava => self.lava(px, mask, a, s),
        }
    }

    fn waves(&mut self, px: &mut Pixmap, a: Area, s: f32) {
        let (bw, gap) = (4.0 * s, 3.0 * s);
        let n = (((a.w + gap) / (bw + gap)).floor() as usize).max(1);
        if self.bars.len() != n {
            self.bars = vec![0.0; n];
        }
        if self.frame % 2 == 0 {
            // ~30 steps/s, the same scroll speed as the original 30 fps meter
            self.bars.rotate_left(1);
            self.bars[n - 1] = self.target;
        }
        let total = n as f32 * bw + (n - 1) as f32 * gap;
        let mut x = a.x + (a.w - total) / 2.0;
        let cy = a.y + a.h / 2.0;
        for &lv in &self.bars {
            let bh = (lv * a.h).max(4.0 * s);
            fill_masked(px, capsule(x, cy - bh / 2.0, bw, bh), [250, 250, 250, 255], None);
            x += bw + gap;
        }
    }

    fn liquid(&mut self, px: &mut Pixmap, mask: &Mask, a: Area, s: f32) {
        let n = ((a.w / (3.0 * s)) as usize).max(8);
        if self.height.len() != n {
            self.height = vec![0.0; n];
            self.vel = vec![0.0; n];
            self.drops.clear();
        }
        let bottom = a.y + a.h;
        let dx = a.w / (n - 1) as f32;
        self.fill += (0.22 + 0.5 * self.energy - self.fill) * 0.16;
        let rest = bottom - self.fill * a.h;

        // Excite: a constant shimmer that grows with loudness, plus a splash on each syllable onset.
        let pushes = 1 + (self.energy * 3.0) as usize;
        for _ in 0..pushes {
            let i = (self.rand() * n as f32) as usize % n;
            let r = self.rand() - 0.5;
            self.vel[i] += r * (0.6 + 4.5 * self.energy) * s;
        }
        if self.onset > 0.05 {
            let i = (self.rand() * n as f32) as usize % n;
            self.vel[i] += (7.0 + 32.0 * self.onset) * s;
            let count = ((self.onset * 14.0) as usize).clamp(1, 4);
            for _ in 0..count {
                if self.drops.len() >= 24 {
                    break;
                }
                let (rx, ry) = (self.rand(), self.rand());
                self.drops.push(Droplet {
                    x: a.x + i as f32 * dx,
                    y: rest - self.height[i],
                    vx: (rx - 0.5) * 1.6 * s,
                    vy: -(1.2 + ry * 2.2) * s * (0.6 + 2.5 * self.onset),
                });
            }
        }

        // Springs: each column is pulled back to rest and damped, then neighbours share height
        // a few times per frame so disturbances travel along the surface as ripples.
        for i in 0..n {
            self.vel[i] += -0.02 * self.height[i] - 0.04 * self.vel[i];
            self.height[i] += self.vel[i];
        }
        let mut left = vec![0.0f32; n];
        let mut right = vec![0.0f32; n];
        for _ in 0..4 {
            for i in 0..n {
                if i > 0 {
                    left[i] = 0.2 * (self.height[i] - self.height[i - 1]);
                    self.vel[i - 1] += left[i];
                }
                if i + 1 < n {
                    right[i] = 0.2 * (self.height[i] - self.height[i + 1]);
                    self.vel[i + 1] += right[i];
                }
            }
            for i in 0..n {
                if i > 0 {
                    self.height[i - 1] += left[i];
                }
                if i + 1 < n {
                    self.height[i + 1] += right[i];
                }
            }
        }
        let cap = a.h * 0.45;
        for h in &mut self.height {
            *h = h.clamp(-cap, cap);
        }

        // Droplets fly, fall back, and ripple the surface where they land.
        let gravity = 0.22 * s;
        let mut i = 0;
        while i < self.drops.len() {
            let d = &mut self.drops[i];
            d.vy += gravity;
            d.x += d.vx;
            d.y += d.vy;
            let col = (((d.x - a.x) / dx).round().max(0.0) as usize).min(n - 1);
            if d.vy > 0.0 && d.y >= rest - self.height[col] {
                self.vel[col] -= 1.2 * s;
                self.drops.swap_remove(i);
            } else {
                i += 1;
            }
        }

        // Draw: a darker mirrored back layer for depth, the lit front body, a glossy rim, droplets.
        let front: Vec<(f32, f32)> = (0..n).map(|i| (a.x + i as f32 * dx, rest - self.height[i])).collect();
        let back: Vec<(f32, f32)> = (0..n).map(|i| (a.x + i as f32 * dx, rest - 3.0 * s - 0.6 * self.height[n - 1 - i])).collect();
        fill_masked(px, smooth_body(&back, bottom + 1.0), [37, 72, 190, 170], Some(mask));
        let top = front.iter().map(|p| p.1).fold(f32::MAX, f32::min);
        let glow = (self.energy * 0.45).min(0.45);
        let mix = |c: [f32; 3]| -> Color {
            let c = c.map(|v| v + (255.0 - v) * glow);
            Color::from_rgba8(c[0] as u8, c[1] as u8, c[2] as u8, 235)
        };
        let shader = LinearGradient::new(
            Point::from_xy(0.0, top),
            Point::from_xy(0.0, bottom),
            vec![
                GradientStop::new(0.0, mix([125.0, 211.0, 252.0])),
                GradientStop::new(0.5, mix([56.0, 132.0, 246.0])),
                GradientStop::new(1.0, mix([67.0, 56.0, 202.0])),
            ],
            SpreadMode::Pad,
            Transform::identity(),
        );
        if let Some(shader) = shader {
            let paint = Paint { shader, anti_alias: true, ..Paint::default() };
            px.fill_path(&smooth_body(&front, bottom + 1.0), &paint, tiny_skia::FillRule::Winding, Transform::identity(), Some(mask));
        }
        if let Some(rim) = smooth_line(&front) {
            let mut paint = Paint::default();
            paint.set_color_rgba8(224, 242, 254, 150);
            paint.anti_alias = true;
            px.stroke_path(&rim, &paint, &Stroke { width: 1.2 * s, ..Stroke::default() }, Transform::identity(), Some(mask));
        }
        for d in &self.drops {
            fill_masked(px, circle(d.x, d.y, 1.5 * s), [186, 230, 253, 235], Some(mask));
        }
    }

    fn plasma(&mut self, px: &mut Pixmap, mask: &Mask, a: Area, s: f32) {
        let e = self.energy;
        let (t, drift) = (self.phase, self.t * 0.04);
        let sat = 0.35 + 0.65 * e;
        let bright = 0.5 + 0.5 * e;
        let contrast = 0.75 + 0.85 * e;
        let (cw, ch) = (a.w / s, a.h / s);
        let cx = cw * (0.5 + 0.35 * (t * 0.6).sin());
        let cy = ch * (0.5 + 0.4 * (t * 0.8).cos());
        let palette = &self.palette;
        shade(px, mask, a, |x, y| {
            let (u, v) = ((x - a.x) / s, (y - a.y) / s);
            let mut f = (u * 0.06 + t * 1.3).sin() + (v * 0.11 - t * 0.9).sin() + ((u + v) * 0.045 + t * 0.7).sin();
            f += (((u - cx).powi(2) + (v - cy).powi(2)).sqrt() * 0.13 - t * 1.6).sin();
            // A curated cyclic ramp rather than a full rainbow.
            let mut c = ramp(palette, f * 0.125 + 0.5 + drift);
            let l = (c[0] + c[1] + c[2]) / 3.0;
            for v in &mut c {
                *v = ((l + (*v - l) * sat - 0.5) * contrast + 0.5) * bright;
            }
            (c, 1.0)
        });
    }

    fn lava(&mut self, px: &mut Pixmap, mask: &Mask, a: Area, s: f32) {
        const BLOBS: [([f32; 3], f32, f32, f32, f32); 5] = [
            ([236.0, 72.0, 153.0], 0.70, 1.10, 0.0, 1.0),
            ([249.0, 115.0, 22.0], 0.90, 0.80, 1.7, 2.1),
            ([139.0, 92.0, 246.0], 0.55, 1.30, 3.3, 0.4),
            ([244.0, 63.0, 94.0], 1.15, 0.65, 4.6, 2.9),
            ([250.0, 204.0, 21.0], 0.80, 1.45, 5.9, 3.7),
        ];
        let e = self.energy;
        let (t, wall) = (self.phase, self.t);
        let blobs: Vec<(f32, f32, f32, [f32; 3])> = BLOBS
            .iter()
            .enumerate()
            .map(|(k, &(color, fx, fy, px0, py0))| {
                let x = a.x + a.w * (0.5 + 0.42 * (t * fx * 0.6 + px0).sin());
                let y = a.y + a.h * (0.5 + 0.36 * (t * fy * 0.6 + py0).sin());
                let r = a.h * (0.34 + 0.06 * (k % 3) as f32) * (1.0 + 0.4 * e + 0.08 * (wall * 3.0 + k as f32).sin());
                (x, y, r * r, color)
            })
            .collect();
        let bright = 0.55 + 0.45 * e;
        let _ = s;
        shade(px, mask, a, |x, y| {
            let (mut f, mut c) = (0.0f32, [0.0f32; 3]);
            for &(bx, by, r2, col) in &blobs {
                let w = r2 / ((x - bx).powi(2) + (y - by).powi(2)).max(1.0);
                f += w;
                for i in 0..3 {
                    c[i] += col[i] * w;
                }
            }
            let c = c.map(|v| v / f / 255.0);
            let edge = smoothstep(0.55, 1.05, f); // soft glow into the dark around each blob
            let core = smoothstep(1.6, 3.2, f) * (0.25 + 0.5 * e); // hot white centres while speaking
            let shade = bright * (0.75 + 0.25 * (f / 2.0).min(1.0));
            (c.map(|v| (v * shade) + (1.0 - v * shade) * core), edge)
        });
    }
}

/// Per-pixel painter over `a`, clipped by `mask` and composited over what is already there.
/// `f` returns (rgb 0..1, alpha 0..1).
fn shade(px: &mut Pixmap, mask: &Mask, a: Area, mut f: impl FnMut(f32, f32) -> ([f32; 3], f32)) {
    let pw = px.width() as usize;
    let ph = px.height() as usize;
    let cov = mask.data();
    let pixels = px.pixels_mut();
    let (x0, x1) = (a.x.max(0.0) as usize, ((a.x + a.w).ceil() as usize).min(pw));
    let (y0, y1) = (a.y.max(0.0) as usize, ((a.y + a.h).ceil() as usize).min(ph));
    for y in y0..y1 {
        for x in x0..x1 {
            let i = y * pw + x;
            let m = cov[i] as f32 / 255.0;
            if m == 0.0 {
                continue;
            }
            let (rgb, alpha) = f(x as f32 + 0.5, y as f32 + 0.5);
            let k = (alpha * m).clamp(0.0, 1.0);
            let under = pixels[i];
            let mix = |src: f32, dst: u8| (src.clamp(0.0, 1.0) * 255.0 * k + dst as f32 * (1.0 - k)).round() as u8;
            let out_a = mix(1.0, under.alpha());
            let r = mix(rgb[0], under.red()).min(out_a);
            let g = mix(rgb[1], under.green()).min(out_a);
            let b = mix(rgb[2], under.blue()).min(out_a);
            if let Some(p) = PremultipliedColorU8::from_rgba(r, g, b, out_a) {
                pixels[i] = p;
            }
        }
    }
}

/// Samples a cyclic 4-colour ramp (see random_palette) at `k`, with smooth blends between stops.
fn ramp(stops: &[[f32; 3]; 5], k: f32) -> [f32; 3] {
    let k = k.rem_euclid(1.0) * 4.0;
    let i = (k as usize).min(3);
    let t = k - i as f32;
    let t = t * t * (3.0 - 2.0 * t);
    [0, 1, 2].map(|j| stops[i][j] + (stops[i + 1][j] - stops[i][j]) * t)
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Smooth curve through the points (quadratic segments between midpoints).
fn smooth_line(pts: &[(f32, f32)]) -> Option<Path> {
    let mut pb = PathBuilder::new();
    push_smooth(&mut pb, pts);
    pb.finish()
}

/// Region under a smooth curve through the points, down to `bottom`.
fn smooth_body(pts: &[(f32, f32)], bottom: f32) -> Path {
    let mut pb = PathBuilder::new();
    push_smooth(&mut pb, pts);
    if let (Some(first), Some(last)) = (pts.first(), pts.last()) {
        pb.line_to(last.0, bottom);
        pb.line_to(first.0, bottom);
        pb.close();
    }
    pb.finish().unwrap_or_else(|| crate::draw::rect(0.0, 0.0, 1.0, 1.0))
}

fn push_smooth(pb: &mut PathBuilder, pts: &[(f32, f32)]) {
    let Some(&(x0, y0)) = pts.first() else { return };
    pb.move_to(x0, y0);
    for w in pts.windows(2) {
        let (p, q) = (w[0], w[1]);
        pb.quad_to(p.0, p.1, (p.0 + q.0) / 2.0, (p.1 + q.1) / 2.0);
    }
    if let Some(&(xl, yl)) = pts.last() {
        pb.line_to(xl, yl);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area() -> (Pixmap, Mask, Area) {
        let px = Pixmap::new(220, 44).unwrap();
        let mut mask = Mask::new(220, 44).unwrap();
        let a = Area { x: 40.0, y: 7.0, w: 173.0, h: 30.0 };
        mask.fill_path(&capsule(a.x, a.y, a.w, a.h), tiny_skia::FillRule::Winding, true, Transform::identity());
        (px, mask, a)
    }

    #[test]
    fn every_style_renders_and_stays_inside_the_pill() {
        for style in Style::ALL {
            let (mut px, mask, a) = area();
            let mut v = Viz::new(style);
            for i in 0..240 {
                let level = if (i / 20) % 2 == 0 { 0.08 } else { 0.0 }; // talk / pause
                v.update(level, 1.0 / 60.0);
                px.fill(Color::TRANSPARENT);
                v.render(&mut px, &mask, a, 1.0);
            }
            let painted = px.pixels().iter().filter(|p| p.alpha() > 0).count();
            assert!(painted > 200, "{style:?} painted only {painted} pixels");
            // Nothing outside the inset (with 1px slack for anti-aliasing).
            for (i, p) in px.pixels().iter().enumerate() {
                let (x, y) = ((i % 220) as f32, (i / 220) as f32);
                if p.alpha() > 0 {
                    assert!(x >= a.x - 1.0 && x <= a.x + a.w + 1.0 && y >= a.y - 1.0 && y <= a.y + a.h + 1.0, "{style:?} leaked at {x},{y}");
                }
            }
        }
    }

    #[test]
    fn liquid_stays_stable_under_constant_shouting() {
        let (mut px, mask, a) = area();
        let mut v = Viz::new(Style::Liquid);
        for i in 0..3000 {
            v.update(if i % 7 < 4 { 0.2 } else { 0.0 }, 1.0 / 60.0);
            v.render(&mut px, &mask, a, 1.0);
        }
        assert!(v.height.iter().all(|h| h.is_finite() && h.abs() <= a.h * 0.45 + 0.01));
        assert!(v.drops.len() <= 24);
    }

    #[test]
    fn goes_still_when_you_stop_talking() {
        let mut v = Viz::new(Style::Waves);
        let mut run = |v: &mut Viz, level: f32, secs: f32| {
            for _ in 0..(secs * 60.0) as usize {
                v.update(level, 1.0 / 60.0);
            }
        };
        run(&mut v, 0.012, 2.0); // room noise: learned as the floor
        assert_eq!(v.target, 0.0, "background noise must not move the bars");
        run(&mut v, 0.08, 1.0); // talking
        assert!(v.energy > 0.5);
        run(&mut v, 0.012, 0.25); // stop talking
        assert_eq!(v.target, 0.0);
        assert!(v.energy < 0.05, "still moving after 0.25 s of silence: {}", v.energy);
    }

    #[test]
    fn normal_speech_does_not_pin_the_meter_at_max() {
        let mut v = Viz::new(Style::Waves);
        let (mut sum, mut low, n) = (0.0, 0, 600);
        for i in 0..n {
            // syllables at ~5 Hz between 0.02 and 0.12 RMS, like ordinary talking
            let t = i as f32 / 60.0;
            v.update(0.02 + 0.10 * (t * 15.0).sin().abs(), 1.0 / 60.0);
            if i >= 120 {
                sum += v.target;
                low += (v.target < 0.35) as i32;
            }
        }
        let mean = sum / (n - 120) as f32;
        assert!(mean < 0.7, "meter sits near max while talking: mean {mean}");
        assert!(low > 60, "the gaps between syllables never show: {low} low frames");
    }

    #[test]
    fn plasma_colours_are_random_every_recording() {
        let mut v = Viz::new(Style::Plasma);
        let mut hues = Vec::new();
        for _ in 0..40 {
            let before = v.hues.0;
            v.reset();
            assert!((v.hues.0 - before).abs() > 1e-4, "same colours twice in a row");
            hues.push(v.hues.0);
            let gap = (v.hues.1 - v.hues.0).rem_euclid(1.0);
            assert!((0.17..=0.69).contains(&gap), "hues too close to tell apart: {gap}");
        }
        // Spread across the whole colour wheel, not a couple of presets.
        let buckets: std::collections::HashSet<_> = hues.iter().map(|h| (h * 8.0) as u32).collect();
        assert!(buckets.len() >= 6, "only {} of 8 hue sectors used", buckets.len());
        // Two separate launches start with different colours (clock-seeded).
        let a = Viz::new(Style::Plasma).hues.0;
        std::thread::sleep(std::time::Duration::from_millis(3));
        assert_ne!(a, Viz::new(Style::Plasma).hues.0);
    }

    #[test]
    fn hsv_matches_known_colours() {
        let close = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4);
        assert!(close(hsv(0.0, 1.0, 1.0), [1.0, 0.0, 0.0]));
        assert!(close(hsv(1.0 / 3.0, 1.0, 1.0), [0.0, 1.0, 0.0]));
        assert!(close(hsv(2.0 / 3.0, 1.0, 0.5), [0.0, 0.0, 0.5]));
        assert!(close(hsv(0.3, 0.0, 0.7), [0.7, 0.7, 0.7]));
    }

    #[test]
    fn style_keys_roundtrip() {
        for s in Style::ALL {
            assert_eq!(Style::from_key(s.key()), Some(s));
        }
        assert_eq!(Style::from_key("nope"), None);
    }
}
