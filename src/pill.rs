//! What the overlay draws, independent of any window: the idle dash, the recording pill (state
//! indicator on the left, visualization on the right) and the transcribing loader (loader.rs),
//! plus the morph between them: the pill balloons out of the dash on a spring, squeezes down to
//! the loader, and zooms back into the dash when it's done. The overlay presents these frames on
//! screen; `cargo run --bin demo` renders them to the README's GIFs.

use tiny_skia::{FilterQuality, Mask, Pixmap, PixmapPaint, Transform};

use crate::draw::{capsule, circle, fill, rect};
use crate::loader::Loader;
use crate::viz::{Area, Style, Viz, smoothstep};

/// Animation rate (see overlay::TIMER).
pub const FPS: f32 = 60.0;
/// Pill size in logical pixels (multiplied by the DPI scale).
pub const SIZE: (f32, f32) = (168.0, 34.0);
/// Extra canvas around the pill (each side, top) for the morph's overshoot.
const MARGIN: (f32, f32) = (12.0, 6.0);
const DASH: (f32, f32) = (36.0, 5.0);

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

impl View {
    /// Views drawn as a full pill (the others are just the dash, or nothing).
    fn has_content(self) -> bool {
        matches!(self, View::Recording | View::Locked | View::Transcribing)
    }
}

const BG: [u8; 4] = [20, 20, 23, 245];
const WELL: [u8; 4] = [10, 10, 12, 255];
const RED: [u8; 4] = [239, 68, 68, 255];
const AMBER: [u8; 4] = [245, 158, 11, 255];

/// The pill's outline and colour: a capsule standing on the bottom edge of the pill box.
#[derive(Clone, Copy, Debug)]
struct Shell {
    cx: f32,
    bottom: f32,
    w: f32,
    h: f32,
    color: [f32; 4],
}

impl Shell {
    /// `e` moves the shape (may overshoot past 0..1), `c` the colour.
    fn lerp(self, to: Shell, e: f32, c: f32) -> Shell {
        let l = |a: f32, b: f32, t: f32| a + (b - a) * t;
        Shell {
            cx: l(self.cx, to.cx, e),
            bottom: l(self.bottom, to.bottom, e),
            w: l(self.w, to.w, e).max(1.0),
            h: l(self.h, to.h, e).max(1.0),
            color: [0, 1, 2, 3].map(|i| l(self.color[i], to.color[i], c.clamp(0.0, 1.0))),
        }
    }

    fn same_shape(&self, o: &Shell) -> bool {
        (self.w - o.w).abs() < 0.5 && (self.h - o.h).abs() < 0.5
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Ease {
    /// Grow or resize: a damped spring that overshoots a little, like a balloon.
    Spring,
    /// Back into the dash: a small anticipation puff, then an accelerating zoom down.
    Zoom,
    /// Same shape, new colour.
    Fade,
}

impl Ease {
    fn duration(self) -> f32 {
        match self {
            Ease::Spring => 0.55,
            Ease::Zoom => 0.36,
            Ease::Fade => 0.25,
        }
    }
}

/// Damped spring step response at `t` seconds (~9% overshoot, settled by ~0.5 s).
fn spring(t: f32) -> f32 {
    let (zeta, omega) = (0.6f32, 18.0f32);
    let wd = omega * (1.0 - zeta * zeta).sqrt();
    1.0 - (-zeta * omega * t).exp() * ((wd * t).cos() + zeta * omega / wd * (wd * t).sin())
}

/// Ease-in with a slight backwards wind-up first.
fn back_in(p: f32) -> f32 {
    let c = 1.5;
    p * p * ((c + 1.0) * p - c)
}

pub struct Pill {
    /// Canvas size: the pill box plus the overshoot margin. This is what goes on screen.
    pub w: i32,
    pub h: i32,
    s: f32,
    pub pixmap: Pixmap,
    /// Top-left of the pill box on the canvas.
    origin: (f32, f32),
    /// The pill box (SIZE): its width and height in pixels.
    bw: f32,
    bh: f32,
    /// The current view and the one it's morphing away from, each drawn at rest in the pill box.
    layers: [Pixmap; 2],
    clip: Mask,
    /// The inset the visualization draws into, and its rounded clip.
    area: Area,
    mask: Mask,
    /// The same for the loader, which is narrower.
    well: Area,
    well_mask: Mask,
    pub viz: Viz,
    loader: Loader,
    /// Transcription progress 0..1 as reported by the owner, and what the bar shows (eased).
    pub progress: f32,
    shown: f32,
    view: View,
    prev: View,
    from: Shell,
    ease: Ease,
    /// Seconds since the view changed, and since transcribing started.
    age: f32,
    clock: f32,
}

impl Pill {
    /// `s` is the DPI scale (1.0 at 96 dpi).
    pub fn new(s: f32, style: Style, loader: Loader) -> Option<Self> {
        let (bw, bh) = ((SIZE.0 * s).round(), (SIZE.1 * s).round());
        let origin = ((MARGIN.0 * s).round(), (MARGIN.1 * s).round());
        let (w, h) = ((bw + 2.0 * origin.0) as i32, (bh + origin.1) as i32);
        // Left: a square slot for the state indicator. Right: the visualization inset.
        let pad = 3.0 * s; // thin bezel around the visualization
        let ax = bh * 0.80;
        let area = Area { x: ax, y: pad, w: bw - ax - pad, h: bh - 2.0 * pad };
        let mut pill = Self {
            w,
            h,
            s,
            pixmap: Pixmap::new(w as u32, h as u32)?,
            origin,
            bw,
            bh,
            layers: [Pixmap::new(bw as u32, bh as u32)?, Pixmap::new(bw as u32, bh as u32)?],
            clip: Mask::new(w as u32, h as u32)?,
            area,
            mask: well_mask(bw, bh, area)?,
            well: area,
            well_mask: Mask::new(1, 1)?,
            viz: Viz::new(style),
            loader,
            progress: 0.0,
            shown: 0.0,
            view: View::Hidden,
            prev: View::Hidden,
            from: Shell { cx: 0.0, bottom: 0.0, w: 1.0, h: 1.0, color: [0.0; 4] },
            ease: Ease::Fade,
            age: f32::MAX,
            clock: 0.0,
        };
        pill.set_loader(loader);
        Some(pill)
    }

    pub fn set_loader(&mut self, loader: Loader) {
        self.loader = loader;
        let pw = (loader.width() * self.s).round();
        let pad = 3.0 * self.s;
        self.well = Area { x: (self.bw - pw) / 2.0 + pad, y: pad, w: pw - 2.0 * pad, h: self.bh - 2.0 * pad };
        if let Some(m) = well_mask(self.bw, self.bh, self.well) {
            self.well_mask = m;
        }
    }

    pub fn view(&self) -> View {
        self.view
    }

    /// Switches to `view`, morphing from wherever the pill is right now (even mid-morph).
    pub fn show(&mut self, view: View) {
        if view == self.view {
            return;
        }
        let from = self.shell();
        let to = self.rest(view);
        self.ease = if !view.has_content() && from.w > to.w + 0.5 {
            Ease::Zoom
        } else if from.same_shape(&to) {
            Ease::Fade
        } else {
            Ease::Spring
        };
        self.from = from;
        self.prev = self.view;
        self.view = view;
        self.age = 0.0;
        if matches!((self.prev, view), (View::Recording, View::Locked) | (View::Locked, View::Recording)) {
            self.age = f32::MAX; // same pill, only the indicator changes
        }
        if view == View::Transcribing {
            (self.progress, self.shown, self.clock) = (0.0, 0.0, 0.0);
        } else if self.prev == View::Transcribing {
            self.progress = 1.0; // let the bar finish while the pill zooms away
        }
    }

    fn morphing(&self) -> bool {
        self.age < self.ease.duration()
    }

    /// Whether frames still change (the timer must keep running).
    pub fn animating(&self) -> bool {
        self.morphing() || self.view.has_content()
    }

    /// Where each view's pill rests, in pill-box pixels.
    fn rest(&self, view: View) -> Shell {
        let s = self.s;
        let base = Shell { cx: self.bw / 2.0, bottom: self.bh, w: DASH.0 * s, h: DASH.1 * s, color: [0.0; 4] };
        let rgba = |c: [u8; 4]| c.map(|v| v as f32 / 255.0);
        match view {
            View::Hidden => Shell { color: rgba([250, 250, 250, 0]), ..base },
            View::Idle(Idle::Ready) => Shell { color: rgba([250, 250, 250, 150]), ..base },
            View::Idle(Idle::Loading) => Shell { color: rgba([113, 113, 122, 150]), ..base },
            View::Idle(Idle::Error) => Shell { color: rgba([245, 158, 11, 200]), ..base },
            View::Flash => Shell { color: rgba(AMBER), ..base },
            View::Recording | View::Locked => Shell { w: self.bw, h: self.bh, color: rgba(BG), ..base },
            View::Transcribing => Shell { w: (self.loader.width() * s).round(), h: self.bh, color: rgba(BG), ..base },
        }
    }

    /// The pill's outline right now.
    fn shell(&self) -> Shell {
        let to = self.rest(self.view);
        if !self.morphing() {
            return to;
        }
        let (e, c) = self.curve();
        self.from.lerp(to, e, c)
    }

    /// How far the morph has got: (shape, may overshoot; colour 0..1).
    fn curve(&self) -> (f32, f32) {
        let p = (self.age / self.ease.duration()).clamp(0.0, 1.0);
        match self.ease {
            Ease::Spring => (spring(self.age), smoothstep(0.0, 0.5, p)),
            // The colour follows the shape, so the pill only turns into the dash as it gets small.
            Ease::Zoom => (back_in(p), smoothstep(0.2, 1.0, back_in(p))),
            Ease::Fade => (smoothstep(0.0, 1.0, p), smoothstep(0.0, 1.0, p)),
        }
    }

    /// Draws the next frame (1/FPS later) into `pixmap`. `level` is mic loudness.
    pub fn paint(&mut self, level: f32) {
        let dt = 1.0 / FPS;
        self.age = (self.age + dt).min(f32::MAX);
        self.clock += dt;
        if self.view == View::Transcribing || self.prev == View::Transcribing {
            self.shown += ((self.progress - self.shown) * 0.12).max(0.0); // eased, never backwards
        }
        // New content fades in over the first 0.2 s of a morph while the old fades out; zooming
        // into the dash, the old content stays until the pill is actually shrinking.
        let fade = match (self.morphing(), self.ease) {
            (false, _) => 1.0,
            (true, Ease::Zoom) => smoothstep(0.05, 0.6, self.curve().0),
            (true, _) => smoothstep(0.0, 0.2, self.age),
        };
        let (view, prev) = (self.view, self.prev);
        if view.has_content() {
            self.draw_view(0, view, Some(level));
        }
        let old = self.morphing() && prev.has_content() && fade < 1.0;
        if old {
            self.draw_view(1, prev, None);
        }

        self.pixmap.fill(tiny_skia::Color::TRANSPARENT);
        let (ox, oy) = self.origin;
        if !self.morphing() {
            if view.has_content() {
                self.pixmap.draw_pixmap(ox as i32, oy as i32, self.layers[0].as_ref(), &PixmapPaint::default(), Transform::identity(), None);
            } else {
                let sh = self.shell();
                let outline = self.outline(sh);
                fill(&mut self.pixmap, outline, sh.color.map(|v| (v * 255.0) as u8));
            }
            return;
        }
        let sh = self.shell();
        let outline = self.outline(sh);
        fill(&mut self.pixmap, outline.clone(), sh.color.map(|v| (v * 255.0) as u8));
        self.clip.clear();
        self.clip.fill_path(&outline, tiny_skia::FillRule::Winding, true, Transform::identity());
        // Each layer zooms with the pill: scaled by the width ratio about the centres, clipped to the outline.
        for (i, v, alpha) in [(1, prev, if old { 1.0 - fade } else { 0.0 }), (0, view, fade)] {
            if !v.has_content() || alpha <= 0.0 {
                continue;
            }
            let r = self.rest(v);
            let k = sh.w / r.w;
            let t = Transform::from_translate(-r.cx, -(r.bottom - r.h / 2.0)).post_scale(k, k).post_translate(ox + sh.cx, oy + sh.bottom - sh.h / 2.0);
            let paint = PixmapPaint { opacity: alpha, quality: FilterQuality::Bilinear, ..PixmapPaint::default() };
            self.pixmap.draw_pixmap(0, 0, self.layers[i].as_ref(), &paint, t, Some(&self.clip));
        }
    }

    fn outline(&self, sh: Shell) -> tiny_skia::Path {
        let (ox, oy) = self.origin;
        capsule(ox + sh.cx - sh.w / 2.0, oy + sh.bottom - sh.h, sh.w, sh.h)
    }

    /// Draws `view` at rest into layer `i`. `level` advances the visualization; `None` redraws
    /// it as it is (the outgoing view during a morph).
    fn draw_view(&mut self, i: usize, view: View, level: Option<f32>) {
        let (w, h, s) = (self.bw, self.bh, self.s);
        let px = &mut self.layers[i];
        px.fill(tiny_skia::Color::TRANSPARENT);
        match view {
            View::Recording | View::Locked => {
                if let Some(level) = level {
                    self.viz.update(level, 1.0 / FPS);
                }
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
                let pw = (self.loader.width() * s).round();
                fill(px, capsule((w - pw) / 2.0, 0.0, pw, h), BG);
                self.loader.draw(px, &self.well_mask, self.well, s, self.clock, self.shown, &self.viz);
            }
            View::Hidden | View::Idle(_) | View::Flash => {}
        }
    }
}

fn well_mask(w: f32, h: f32, a: Area) -> Option<Mask> {
    let mut mask = Mask::new(w as u32, h as u32)?;
    mask.fill_path(&capsule(a.x, a.y, a.w, a.h), tiny_skia::FillRule::Winding, true, Transform::identity());
    Some(mask)
}

/// Speech-like loudness for previews: bursts of syllables with pauses between phrases.
pub fn demo_level(t: f32) -> f32 {
    let syllables = (t * 9.0).sin().abs();
    let phrase = (0.5 + 0.5 * (t * 1.7).sin()).powi(2);
    0.005 + 0.09 * syllables * phrase
}

/// Made-up transcription progress for previews: quick start, a pause, then a run to the end.
pub fn demo_progress(t: f32, secs: f32) -> f32 {
    let p = (t / secs).clamp(0.0, 1.0);
    (p + 0.08 * (p * std::f32::consts::TAU).sin()).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames(pill: &mut Pill, n: usize) {
        for _ in 0..n {
            pill.paint(0.02);
        }
    }

    #[test]
    fn spring_and_zoom_end_where_they_should() {
        assert!((spring(Ease::Spring.duration()) - 1.0).abs() < 0.01);
        assert!((0.0..=0.12).contains(&(spring(0.18) - 1.0)), "should overshoot a little: {}", spring(0.18));
        assert_eq!(back_in(1.0), 1.0);
        assert!(back_in(0.2) < 0.0, "zoom winds up first");
    }

    #[test]
    fn morphs_stay_on_the_canvas_and_settle() {
        for loader in Loader::ALL {
            let mut pill = Pill::new(1.5, Style::Plasma, loader).unwrap();
            let path = [View::Idle(Idle::Ready), View::Recording, View::Locked, View::Transcribing, View::Idle(Idle::Ready), View::Hidden];
            for v in path {
                pill.show(v);
                for _ in 0..60 {
                    pill.paint(0.05);
                    let sh = pill.shell();
                    let (ox, oy) = pill.origin;
                    assert!(ox + sh.cx - sh.w / 2.0 >= -0.5 && ox + sh.cx + sh.w / 2.0 <= pill.w as f32 + 0.5, "{v:?} {sh:?}");
                    assert!(oy + sh.bottom - sh.h >= -0.5, "{v:?} {sh:?}");
                }
                assert!(!pill.morphing(), "{v:?} still morphing after 1 s");
            }
            assert_eq!(pill.pixmap.pixels().iter().filter(|p| p.alpha() > 0).count(), 0, "hidden draws nothing");
        }
    }

    #[test]
    fn dash_only_animates_while_morphing() {
        let mut pill = Pill::new(1.0, Style::Waves, Loader::Dots).unwrap();
        pill.show(View::Idle(Idle::Loading));
        frames(&mut pill, 30);
        assert!(!pill.animating());
        pill.show(View::Idle(Idle::Ready)); // colour fade only
        assert!(pill.animating() && pill.ease == Ease::Fade);
        frames(&mut pill, 30);
        assert!(!pill.animating());
        pill.show(View::Recording);
        assert_eq!(pill.ease, Ease::Spring);
        pill.show(View::Idle(Idle::Ready)); // interrupted: zooms back from wherever it got to
        assert!(pill.animating());
    }

    #[test]
    fn progress_bar_finishes_while_zooming_away() {
        let mut pill = Pill::new(1.0, Style::Plasma, Loader::Progress).unwrap();
        pill.show(View::Transcribing);
        pill.progress = 0.4;
        frames(&mut pill, 60);
        assert!((pill.shown - 0.4).abs() < 0.01);
        pill.progress = 0.2; // never runs backwards
        frames(&mut pill, 10);
        assert!(pill.shown >= 0.39);
        pill.show(View::Idle(Idle::Ready));
        assert_eq!(pill.ease, Ease::Zoom);
        frames(&mut pill, 10);
        assert!(pill.shown > 0.75);
    }
}
