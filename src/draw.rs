//! Shape helpers shared by the overlay, tray and app icons.

use tiny_skia::{FillRule, Paint, Path, PathBuilder, Pixmap, Transform};

pub fn fill(px: &mut Pixmap, path: Path, c: [u8; 4]) {
    let mut paint = Paint::default();
    paint.set_color_rgba8(c[0], c[1], c[2], c[3]);
    paint.anti_alias = true;
    px.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
}

pub fn circle(x: f32, y: f32, r: f32) -> Path {
    PathBuilder::from_circle(x, y, r).unwrap_or_else(|| rect(x, y, 1.0, 1.0))
}

pub fn rect(x: f32, y: f32, w: f32, h: f32) -> Path {
    PathBuilder::from_rect(tiny_skia::Rect::from_xywh(x, y, w.max(0.1), h.max(0.1)).unwrap())
}

/// Rectangle with fully rounded ends.
pub fn capsule(x: f32, y: f32, w: f32, h: f32) -> Path {
    rounded_rect(x, y, w, h, w.min(h) / 2.0)
}

pub fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Path {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    let k = 0.5523 * r; // cubic approximation of a quarter circle
    let (x1, y1) = (x + w, y + h);
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x1 - r, y);
    pb.cubic_to(x1 - r + k, y, x1, y + r - k, x1, y + r);
    pb.line_to(x1, y1 - r);
    pb.cubic_to(x1, y1 - r + k, x1 - r + k, y1, x1 - r, y1);
    pb.line_to(x + r, y1);
    pb.cubic_to(x + r - k, y1, x, y1 - r + k, x, y1 - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish().unwrap_or_else(|| rect(x, y, w, h))
}
