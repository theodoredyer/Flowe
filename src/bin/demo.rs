//! Renders the README's GIFs with the real overlay drawing code and synthetic speech:
//! `cargo run --release --features demo --bin demo` (writes docs/*.gif).
#![allow(dead_code)] // the shared modules have helpers only the main app uses

#[path = "../draw.rs"]
mod draw;
#[path = "../loader.rs"]
mod loader;
#[path = "../pill.rs"]
mod pill;
#[path = "../viz.rs"]
mod viz;

use std::fs::File;

use gif::{DisposalMethod, Encoder, Frame, Repeat};
use loader::Loader;
use pill::{FPS, Idle, Pill, View, demo_progress};
use tiny_skia::{Color, GradientStop, LinearGradient, Pixmap, PixmapPaint, Point, Rect, SpreadMode, Transform};
use viz::Style;

/// Render scale: 2x so the GIFs stay crisp on high-DPI screens.
const S: f32 = 2.0;
/// GIF frame delays in centiseconds, cycled: averages 30 fps (two 60 Hz sim steps per frame).
const DELAYS: [u16; 3] = [3, 3, 4];

/// A slice of desktop: dark wallpaper, optionally a taskbar edge, the pill centred above it.
struct Scene {
    bg: Pixmap,
    /// Where the pill sits on the canvas.
    at: (i32, i32),
    pill: Pill,
}

impl Scene {
    fn new(style: Style, loader: Loader, (lw, lh): (f32, f32), taskbar: bool) -> Self {
        let pill = Pill::new(S, style, loader).expect("pill");
        let (w, h) = ((lw * S) as u32, (lh * S) as u32);
        let mut bg = Pixmap::new(w, h).expect("canvas");
        let paint = tiny_skia::Paint {
            shader: LinearGradient::new(
                Point::from_xy(0.0, 0.0),
                Point::from_xy(w as f32, h as f32),
                vec![GradientStop::new(0.0, Color::from_rgba8(24, 22, 38, 255)), GradientStop::new(1.0, Color::from_rgba8(10, 12, 20, 255))],
                SpreadMode::Pad,
                Transform::identity(),
            )
            .expect("gradient"),
            ..Default::default()
        };
        bg.fill_rect(Rect::from_xywh(0.0, 0.0, w as f32, h as f32).unwrap(), &paint, Transform::identity(), None);
        let mut floor = h as i32;
        if taskbar {
            let tb = (14.0 * S) as i32;
            floor -= tb;
            let r = Rect::from_xywh(0.0, floor as f32, w as f32, tb as f32).unwrap();
            let solid =
                |c: [u8; 4]| tiny_skia::Paint { shader: tiny_skia::Shader::SolidColor(Color::from_rgba8(c[0], c[1], c[2], c[3])), ..Default::default() };
            bg.fill_rect(r, &solid([28, 28, 34, 255]), Transform::identity(), None);
            bg.fill_rect(Rect::from_xywh(0.0, floor as f32, w as f32, S).unwrap(), &solid([48, 48, 56, 255]), Transform::identity(), None);
            floor -= (6.0 * S) as i32; // the overlay's gap above the taskbar
        } else {
            floor -= (h as i32 - pill.h) / 2;
        }
        let at = ((w as i32 - pill.w) / 2, floor - pill.h);
        Self { bg, at, pill }
    }

    /// The next frame of the scene, cropped to the pill's rectangle (the rest never changes).
    fn frame(&mut self, level: f32) -> Vec<u8> {
        self.pill.paint(level);
        let mut canvas = self.bg.clone();
        canvas.draw_pixmap(self.at.0, self.at.1, self.pill.pixmap.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
        let (cw, (x0, y0)) = (canvas.width() as usize, self.at);
        let mut out = Vec::with_capacity((self.pill.w * self.pill.h * 4) as usize);
        for y in y0..y0 + self.pill.h {
            let row = (y as usize * cw + x0 as usize) * 4;
            out.extend_from_slice(&canvas.data()[row..row + self.pill.w as usize * 4]); // opaque, so premultiplied == straight
        }
        out
    }
}

struct Gif {
    enc: Encoder<File>,
    n: usize,
}

impl Gif {
    fn create(path: &str, scene: &Scene) -> Self {
        let (w, h) = (scene.bg.width() as u16, scene.bg.height() as u16);
        let mut enc = Encoder::new(File::create(path).expect(path), w, h, &[]).expect("gif");
        enc.set_repeat(Repeat::Infinite).expect("repeat");
        let mut bg = scene.bg.data().to_vec();
        let mut first = Frame::from_rgba_speed(w, h, &mut bg, 10);
        first.delay = 0;
        enc.write_frame(&first).expect("frame");
        Self { enc, n: 0 }
    }

    fn push(&mut self, scene: &Scene, mut rgba: Vec<u8>) {
        let mut f = Frame::from_rgba_speed(scene.pill.w as u16, scene.pill.h as u16, &mut rgba, 10);
        (f.left, f.top) = (scene.at.0 as u16, scene.at.1 as u16);
        f.delay = DELAYS[self.n % DELAYS.len()];
        f.dispose = DisposalMethod::Keep;
        self.enc.write_frame(&f).expect("frame");
        self.n += 1;
    }
}

/// Talking between `from` and `to` seconds (syllables in phrases), silence around it.
fn speech(t: f32, from: f32, to: f32) -> f32 {
    if t < from || t > to {
        return 0.003;
    }
    let edge = ((t - from).min(to - t) / 0.15).min(1.0); // no hard start/stop
    let t = t - from;
    let syllables = (t * 9.0).sin().abs();
    let stress = 0.4 + 0.6 * (0.5 + 0.5 * (t * 2.9 + 1.0).sin()); // louder and quieter words
    let breath = ((t % 1.5 - 1.25) / 0.06).clamp(0.0, 1.0); // a short pause every 1.5 s
    let breath = 1.0 - breath * (1.0 - ((t % 1.5 - 1.43) / 0.06).clamp(0.0, 1.0));
    0.003 + 0.085 * syllables * stress * breath * edge
}

/// Plays a timeline of (view, seconds) segments into a GIF. The visualization starts calm at
/// the first Recording and keeps going through Locked, and the pill morphs between views,
/// exactly like the app. Transcribing gets made-up progress that finishes on time.
fn render(path: &str, mut scene: Scene, timeline: &[(View, f32)], talk: (f32, f32)) {
    let mut gif = Gif::create(path, &scene);
    let (mut t, mut step) = (0.0f32, 0u32);
    for &(view, secs) in timeline {
        if view == View::Recording {
            scene.pill.viz.reset();
        }
        scene.pill.show(view);
        let steps = (secs * FPS) as u32;
        for tick in 0..steps {
            if view == View::Transcribing {
                scene.pill.progress = demo_progress(tick as f32 / FPS, secs * 0.85);
            }
            // Advance the simulation every 60 Hz step; keep every second one as a frame.
            let rgba = scene.frame(speech(t, talk.0, talk.1));
            if step % 2 == 0 {
                gif.push(&scene, rgba);
            }
            t += 1.0 / FPS;
            step += 1;
        }
    }
    println!("{path}: {} frames, {} KB", gif.n, std::fs::metadata(path).map(|m| m.len() / 1024).unwrap_or(0));
}

fn main() {
    std::fs::create_dir_all("docs").expect("docs/");
    // The whole flow: idle dash, hold Ctrl+Win, tap Space to lock, stop, transcribe, back to idle.
    render(
        "docs/flow.gif",
        Scene::new(Style::Plasma, Loader::Progress, (260.0, 76.0), true),
        &[(View::Idle(Idle::Ready), 0.8), (View::Recording, 2.2), (View::Locked, 2.6), (View::Transcribing, 1.6), (View::Idle(Idle::Ready), 0.9)],
        (1.05, 5.3),
    );
    // Each visualization: a few seconds of speech, then it settles.
    for style in [Style::Waves, Style::Liquid, Style::Lava] {
        render(&format!("docs/{}.gif", style.key()), Scene::new(style, Loader::Progress, (196.0, 54.0), false), &[(View::Recording, 4.5)], (0.3, 3.4));
    }
    // Plasma picks new colours every recording, so show three.
    let rec = (View::Recording, 1.9);
    render("docs/plasma.gif", Scene::new(Style::Plasma, Loader::Progress, (196.0, 54.0), false), &[rec, rec, rec], (0.2, 5.5));
    // Each loader: the end of a recording, transcribing, and the zoom back into the dash.
    for loader in Loader::ALL {
        render(
            &format!("docs/loader-{}.gif", loader.key()),
            Scene::new(Style::Plasma, loader, (196.0, 54.0), false),
            &[(View::Recording, 0.9), (View::Transcribing, 2.0), (View::Idle(Idle::Ready), 0.8)],
            (0.0, 0.9),
        );
    }
}
