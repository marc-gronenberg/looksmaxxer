//! Vignette: dunkelt zum Rand hin ab (oder hellt auf), als Ellipse, die sich verschieben, in der
//! Größe und im Seitenverhältnis ändern lässt. Helle Stellen können geschützt werden.
//!
//! Auf dem Bild zeigt die Oberfläche die Ellipse als Griff (`overlay: "ellipse"`): gestrichelt in
//! der Mitte des Übergangs, also bei `(inner + 1) / 2` der Radien. Die Oberfläche rechnet die Form
//! genauso wie [`Shape::new`].

use super::{Frame, Param, Step, Swatch, ToolDef};
use crate::{luma, smoothstep, Gpu, PositionOp};

pub const TOOL: ToolDef = ToolDef {
    id: "vignette",
    name: "Vignette",
    description: "Darkens (or lightens) towards the edges. Drag the ring on the image to move and shape it.",
    params: &[
        // Der Schlüssel bleibt `amount`, damit gespeicherte Looks passen.
        Param { key: "amount", label: "Strength", min: -100.0, default: 50.0, signed: true, ..Param::PERCENT },
        Param { key: "size", label: "Size", min: 10.0, max: 200.0, default: 65.0, neutral: 10.0, ..Param::PERCENT },
        Param { key: "softness", label: "Softness", default: 50.0, ..Param::PERCENT },
        Param { key: "centerX", label: "Center X", min: -50.0, max: 50.0, step: 0.1, digits: 1, signed: true, ..Param::PERCENT },
        Param { key: "centerY", label: "Center Y", min: -50.0, max: 50.0, step: 0.1, digits: 1, signed: true, ..Param::PERCENT },
        Param { key: "highlights", label: "Highlights", ..Param::PERCENT },
        Param { key: "aspect", label: "Aspect", min: 0.25, max: 4.0, step: 0.01, default: 1.0, neutral: 1.0, unit: "", digits: 2, ..Param::PERCENT },
        Param { key: "anamorphic", label: "Anamorphic", ..Param::TOGGLE },
    ],
    swatch: Swatch::Flat,
    overlay: "ellipse",
    build,
};

fn build(v: &[f32], frame: &Frame) -> Option<Step> {
    let strength = (v[0] / 100.0).clamp(-1.0, 1.0);
    if strength == 0.0 || !strength.is_finite() {
        return None;
    }
    let shape = Shape::new(frame, v[1], v[2], v[3], v[4], v[6], v[7] > 0.5);
    Some(Step::Position(Box::new(Lens { shape, strength, highlights: (v[5] / 100.0).clamp(0.0, 1.0) })))
}

/// Die Ellipse in Pixeln.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shape {
    pub cx: f32,
    pub cy: f32,
    /// Radien, bei denen die volle Stärke erreicht ist.
    pub rx: f32,
    pub ry: f32,
    /// Ab diesem Anteil der Radien beginnt der Übergang (0…1).
    pub inner: f32,
}

impl Shape {
    /// Größe 10…200 % der halben Diagonale, Weichheit 0…100, Mitte als Verschiebung in % von
    /// Breite und Höhe, Seitenverhältnis 0.25…4; `anamorphic` = die Ellipse folgt dem Bildformat.
    pub fn new(frame: &Frame, size: f32, softness: f32, center_x: f32, center_y: f32, aspect: f32, anamorphic: bool) -> Shape {
        let [cx, cy] = frame.centre();
        let r = frame.half_diagonal() * (size / 100.0).clamp(0.1, 2.0);
        let format = if anamorphic && frame.height > 0.0 { (frame.width / frame.height).sqrt() } else { 1.0 };
        let a = aspect.clamp(0.25, 4.0).sqrt() * format;
        Shape {
            cx: cx + center_x.clamp(-50.0, 50.0) / 100.0 * frame.width,
            cy: cy + center_y.clamp(-50.0, 50.0) / 100.0 * frame.height,
            rx: (r * a).max(1e-3),
            ry: (r / a).max(1e-3),
            inner: 1.0 - (softness / 100.0).clamp(0.0, 1.0),
        }
    }

    /// 0 innen … 1 ab dem Rand der Ellipse.
    fn falloff(&self, x: f32, y: f32) -> f32 {
        let (dx, dy) = ((x - self.cx) / self.rx, (y - self.cy) / self.ry);
        let d = (dx * dx + dy * dy).sqrt();
        if d <= self.inner { 0.0 } else { smoothstep(self.inner, 1.0, d) }
    }
}

struct Lens {
    shape: Shape,
    /// −1…1: positiv abdunkeln, negativ aufhellen.
    strength: f32,
    /// 0…1: so sehr bleiben helle Stellen verschont.
    highlights: f32,
}

impl PositionOp for Lens {
    fn pixel(&self, c: [f32; 3], x: f32, y: f32) -> [f32; 3] {
        let fall = self.shape.falloff(x, y);
        if fall == 0.0 {
            return c;
        }
        let protect = 1.0 - self.highlights * smoothstep(0.5, 1.0, luma(c).clamp(0.0, 1.0));
        let k = fall * self.strength.abs() * protect;
        if self.strength > 0.0 {
            [c[0] * (1.0 - k), c[1] * (1.0 - k), c[2] * (1.0 - k)]
        } else {
            c.map(|v| v + (1.0 - v).max(0.0) * k)
        }
    }

    fn gpu(&self) -> Option<Gpu> {
        let s = &self.shape;
        Some(Gpu {
            code: "float d = length((p - vec2(u[0], u[1])) / vec2(u[2], u[3]));
  if (d <= u[4]) return c;
  float k = ss(u[4], 1.0, d) * abs(u[5]) * (1.0 - u[6] * ss(0.5, 1.0, clamp(luma(c), 0.0, 1.0)));
  return u[5] > 0.0 ? c * (1.0 - k) : c + max(1.0 - c, 0.0) * k;",
            lib: "",
            values: [s.cx, s.cy, s.rx, s.ry, s.inner, self.strength, self.highlights, 0.0, 0.0, 0.0, 0.0, 0.0],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: Frame = Frame { width: 160.0, height: 90.0, seed: 0 };

    fn lens(v: [f32; 8]) -> Box<dyn PositionOp> {
        match build(&v, &FRAME) {
            Some(Step::Position(op)) => op,
            _ => panic!("keine Vignette"),
        }
    }

    #[test]
    fn darkens_corners_lightens_with_negative_strength_and_leaves_the_centre() {
        let g = [0.5; 3];
        let dark = lens([60.0, 60.0, 50.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        let light = lens([-60.0, 60.0, 50.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        assert_eq!(dark.pixel(g, 80.0, 45.0), g);
        assert!(dark.pixel(g, 1.0, 1.0)[0] < 0.3);
        assert!(light.pixel(g, 1.0, 1.0)[0] > 0.7);
        assert!(build(&[0.0, 60.0, 50.0, 0.0, 0.0, 0.0, 1.0, 0.0], &FRAME).is_none());
    }

    #[test]
    fn the_centre_moves_and_highlights_are_protected() {
        let g = [0.5; 3];
        let moved = lens([80.0, 30.0, 20.0, 40.0, 0.0, 0.0, 1.0, 0.0]);
        // Mitte nach rechts verschoben: rechts unberührt, links dunkel.
        assert_eq!(moved.pixel(g, 144.0, 45.0), g);
        assert!(moved.pixel(g, 16.0, 45.0)[0] < 0.3);
        let open = lens([80.0, 40.0, 30.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        let kept = lens([80.0, 40.0, 30.0, 0.0, 0.0, 100.0, 1.0, 0.0]);
        let white = [1.0; 3];
        assert!(kept.pixel(white, 1.0, 1.0)[0] > open.pixel(white, 1.0, 1.0)[0] + 0.5);
    }

    #[test]
    fn aspect_and_anamorphic_stretch_the_ellipse() {
        let round = Shape::new(&FRAME, 50.0, 50.0, 0.0, 0.0, 1.0, false);
        assert!((round.rx - round.ry).abs() < 1e-3);
        let wide = Shape::new(&FRAME, 50.0, 50.0, 0.0, 0.0, 4.0, false);
        assert!((wide.rx / wide.ry - 4.0).abs() < 1e-3);
        let format = Shape::new(&FRAME, 50.0, 50.0, 0.0, 0.0, 1.0, true);
        assert!((format.rx / format.ry - 160.0 / 90.0).abs() < 1e-3);
        // Fläche bleibt gleich.
        assert!((wide.rx * wide.ry - round.rx * round.ry).abs() < 1e-1);
    }
}
