//! Teal-Orange: Schatten Richtung Blaugrün, Lichter Richtung Orange; Hauttöne werden vom Blaugrün
//! ausgenommen.

use super::{Frame, Param, Step, Swatch, ToolDef};
use crate::{luma, smoothstep, ColourOp, TealOrange};

pub const TOOL: ToolDef = ToolDef {
    id: "tealOrange",
    name: "Teal-Orange",
    description: "Shadows towards teal, highlights towards orange. Skin protection keeps skin tones out of the teal.",
    params: &[
        Param { key: "amount", label: "Amount", default: 55.0, ..Param::PERCENT },
        Param { key: "skin", label: "Skin protection", default: 60.0, ..Param::PERCENT },
    ],
    swatch: Swatch::Ramp,
    overlay: "",
    build,
};

fn build(v: &[f32], _: &Frame) -> Option<Step> {
    op(TealOrange { amount: v[0], skin: v[1] }).map(Step::Colour)
}

/// `None`, wenn Teal-Orange nichts tut.
pub fn op(t: TealOrange) -> Option<Box<dyn ColourOp>> {
    let (amount, skin) = ((t.amount / 100.0).clamp(0.0, 1.0), (t.skin / 100.0).clamp(0.0, 1.0));
    (amount > 1e-4).then(|| Box::new(Split { amount, skin }) as Box<dyn ColourOp>)
}

/// Stärke und Hautschutz, je 0…1.
struct Split {
    amount: f32,
    skin: f32,
}

impl ColourOp for Split {
    fn apply(&self, mut c: [f32; 3]) -> [f32; 3] {
        const TEAL: [f32; 3] = [-1.0, 0.237, 0.6];
        const ORANGE: [f32; 3] = [0.6, -0.078, -1.0];
        let y = luma(c).clamp(0.0, 1.0);
        // Reines Schwarz bleibt schwarz: der Schatten-Anteil setzt erst knapp darüber ein.
        let shadows = (1.0 - y) * (1.0 - y) * smoothstep(0.0, 0.15, y);
        let lights = y * y;
        let protect = 1.0 - self.skin * skin_mask(c);
        let a = self.amount * 0.16;
        for i in 0..3 {
            c[i] += a * (shadows * protect * TEAL[i] + lights * ORANGE[i] * 0.6);
        }
        c
    }
}

/// 0–1: wie sehr eine Farbe nach Haut aussieht (Farbton um 25°, mittlere Sättigung, nicht zu
/// dunkel).
fn skin_mask(c: [f32; 3]) -> f32 {
    let max = c[0].max(c[1]).max(c[2]);
    let min = c[0].min(c[1]).min(c[2]);
    let d = max - min;
    // Haut liegt zwischen Rot und Gelb: Rot ist der größte Kanal.
    if d < 1e-4 || c[0] < max {
        return 0.0;
    }
    // Farbton im roten Sektor, −60° (Magenta) … 0° (Rot) … 60° (Gelb). Das Fenster läuft bei
    // −5° und 55° stetig auf null aus, es gibt also nirgends eine Kante.
    let hue = 60.0 * (c[1] - c[2]) / d;
    let hue_w = 1.0 - smoothstep(12.0, 30.0, (hue - 25.0).abs());
    let sat = d / max;
    // Untere Grenze über den Farbabstand (nahe an Grau ist es keine Haut), obere über die
    // Sättigung (knallige Farben auch nicht).
    let sat_w = smoothstep(0.03, 0.15, d) * (1.0 - smoothstep(0.75, 0.95, sat));
    // Sehr dunkle Farben zählen nicht als Haut: Dort schwankt die Sättigung von Pixel zu Pixel
    // stark, der Schutz würde in den Schatten rauschen.
    let value_w = smoothstep(0.12, 0.35, max);
    hue_w * sat_w * value_w
}
