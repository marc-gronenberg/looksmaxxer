//! Gradient: ein Verlaufsfilter wie vor dem Objektiv. Am Startpunkt färbt er voll, zum Endpunkt hin
//! läuft er aus; dahinter tut er nichts. Gerechnet wird im linearen Licht als Filter (Multiplikation),
//! die Farbe verschiebt die Kanäle, ohne die Helligkeit im Mittel zu ändern.
//!
//! Auf dem Bild zeigt die Oberfläche Start- und Endpunkt als Griffe (`overlay: "points"`).

use super::{Frame, Param, Step, Swatch, ToolDef};
use crate::{hue_colour, linear_to_srgb, srgb_to_linear, Gpu, PositionOp};

pub const TOOL: ToolDef = ToolDef {
    id: "gradient",
    name: "Gradient",
    description: "A graduated colour filter, full at the start point and fading out towards the end point. Drag the points on the image.",
    params: &[
        Param { key: "x1", label: "Start X", min: -100.0, max: 100.0, step: 0.1, digits: 1, signed: true, ..Param::PERCENT },
        Param { key: "y1", label: "Start Y", min: -100.0, max: 100.0, step: 0.1, default: -50.0, digits: 1, signed: true, ..Param::PERCENT },
        Param { key: "x2", label: "End X", min: -100.0, max: 100.0, step: 0.1, digits: 1, signed: true, ..Param::PERCENT },
        Param { key: "y2", label: "End Y", min: -100.0, max: 100.0, step: 0.1, digits: 1, signed: true, ..Param::PERCENT },
        Param { key: "falloff", label: "Falloff", max: 1.0, step: 0.01, default: 0.5, neutral: 0.5, unit: "", digits: 2, ..Param::PERCENT },
        Param { key: "amount", label: "Amount", default: 100.0, ..Param::PERCENT },
        Param { key: "exposure", label: "Exposure Comp.", min: -4.0, max: 4.0, step: 0.01, digits: 2, unit: " EV", signed: true, ..Param::PERCENT },
        Param { key: "hue", label: "Hue", group: "Color", default: 35.0, ..Param::HUE },
        Param { key: "saturation", label: "Saturation", default: 60.0, ..Param::PERCENT },
    ],
    swatch: Swatch::Flat,
    overlay: "points",
    build,
};

fn build(v: &[f32], frame: &Frame) -> Option<Step> {
    let amount = (v[5] / 100.0).clamp(0.0, 1.0);
    let gain = 2.0f32.powf(v[6].clamp(-4.0, 4.0));
    let filter = hue_colour(v[7], v[8]).map(|c| 2.0 * c * gain);
    filter_step(v, frame, amount, filter, false)
}

/// Ein Verlaufsfilter aus `v` = Start X/Y, End X/Y (% ab der Mitte) und Falloff, auch für
/// `grad_exposure.rs`. `filter` ist der Faktor pro Kanal, wo er voll wirkt; mit `stops` wird er
/// zum Ende hin in Blendenstufen kleiner (`filter^w`), sonst wie Deckkraft (`1 + (filter − 1)·w`).
pub(super) fn filter_step(v: &[f32], frame: &Frame, amount: f32, filter: [f32; 3], stops: bool) -> Option<Step> {
    if amount == 0.0 || filter == [1.0; 3] || !filter.iter().all(|c| c.is_finite()) {
        return None;
    }
    let [cx, cy] = frame.centre();
    let at = |x: f32, y: f32| [cx + x.clamp(-100.0, 100.0) / 100.0 * frame.width, cy + y.clamp(-100.0, 100.0) / 100.0 * frame.height];
    let (start, end) = (at(v[0], v[1]), at(v[2], v[3]));
    let d = [end[0] - start[0], end[1] - start[1]];
    let len2 = d[0] * d[0] + d[1] * d[1];
    Some(Step::Position(Box::new(Filter {
        start,
        // Fallen beide Punkte zusammen, färbt der Filter überall voll.
        dir: if len2 > 1e-6 { [d[0] / len2, d[1] / len2] } else { [0.0, 0.0] },
        // Falloff 0.5 = linear, darunter läuft er langsamer aus, darüber schneller.
        power: 2.0f32.powf((v[4].clamp(0.0, 1.0) - 0.5) * 4.0),
        amount,
        filter,
        stops,
    })))
}

struct Filter {
    start: [f32; 2],
    /// Richtung Start → Ende, geteilt durch die Länge zum Quadrat: `t` ist dann 0…1 auf der Strecke.
    dir: [f32; 2],
    power: f32,
    amount: f32,
    /// Faktor pro Kanal im linearen Licht, wo der Filter voll wirkt.
    filter: [f32; 3],
    stops: bool,
}

impl Filter {
    /// 1 am Start … 0 ab dem Ende, mal `amount`.
    fn weight(&self, x: f32, y: f32) -> f32 {
        let t = ((x - self.start[0]) * self.dir[0] + (y - self.start[1]) * self.dir[1]).clamp(0.0, 1.0);
        (1.0 - t).powf(self.power) * self.amount
    }
}

impl PositionOp for Filter {
    fn pixel(&self, c: [f32; 3], x: f32, y: f32) -> [f32; 3] {
        let w = self.weight(x, y);
        if w == 0.0 {
            return c;
        }
        let mut out = c;
        for (o, f) in out.iter_mut().zip(self.filter) {
            let k = if self.stops { f.max(0.0).powf(w) } else { 1.0 + (f - 1.0) * w };
            *o = linear_to_srgb(srgb_to_linear(*o) * k);
        }
        out
    }

    fn gpu(&self) -> Option<Gpu> {
        let (s, d, f) = (self.start, self.dir, self.filter);
        Some(Gpu {
            code: "float t = clamp(dot(p - vec2(u[0], u[1]), vec2(u[2], u[3])), 0.0, 1.0);
  float w = pow(1.0 - t, u[4]) * u[5];
  if (w == 0.0) return c;
  vec3 f = vec3(u[6], u[7], u[8]);
  return srgb(lin(c) * (u[9] > 0.5 ? pow(max(f, 0.0), vec3(w)) : 1.0 + (f - 1.0) * w));",
            lib: "",
            values: [s[0], s[1], d[0], d[1], self.power, self.amount, f[0], f[1], f[2], if self.stops { 1.0 } else { 0.0 }, 0.0, 0.0],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter(v: &[f32]) -> Box<dyn PositionOp> {
        match build(v, &Frame::of(100.0, 50.0)) {
            Some(Step::Position(op)) => op,
            _ => panic!("kein Filter"),
        }
    }

    #[test]
    fn full_at_the_start_and_gone_past_the_end() {
        // Start oben in der Mitte, Ende in der Mitte; Farbe rot.
        let op = filter(&[0.0, -50.0, 0.0, 0.0, 0.5, 100.0, 0.0, 0.0, 100.0]);
        let grey = [0.5; 3];
        let top = op.pixel(grey, 50.0, 0.0);
        assert!(top[0] > 0.6 && top[1] < 0.4, "{top:?}");
        let half = op.pixel(grey, 50.0, 12.5);
        assert!(half[0] > 0.5 && half[0] < top[0], "{half:?}");
        assert_eq!(op.pixel(grey, 50.0, 25.0), grey);
        assert_eq!(op.pixel(grey, 10.0, 40.0), grey);
    }

    #[test]
    fn nothing_to_do_gives_no_filter() {
        assert!(build(&[0.0, -50.0, 0.0, 0.0, 0.5, 0.0, 0.0, 35.0, 60.0], &Frame::of(10.0, 10.0)).is_none());
        assert!(build(&[0.0, -50.0, 0.0, 0.0, 0.5, 100.0, 0.0, 35.0, 0.0], &Frame::of(10.0, 10.0)).is_none());
    }
}
