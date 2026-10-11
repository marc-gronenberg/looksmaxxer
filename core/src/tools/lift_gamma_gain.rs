//! Lift/Gamma/Gain: Schatten, Mitten und Lichter einfärben, je mit Farbton und Betrag.

use super::{Frame, Param, Step, Swatch, ToolDef};
use crate::{ColourOp, LiftGammaGain};

pub const TOOL: ToolDef = ToolDef {
    id: "liftGammaGain",
    name: "Lift/Gamma/Gain",
    description: "Tint shadows, midtones and highlights. Hue picks the colour, amount how much.",
    params: &[
        Param { key: "liftHue", label: "Hue", group: "Lift", default: 200.0, ..Param::HUE },
        Param { key: "liftAmount", label: "Amount", default: 20.0, ..Param::PERCENT },
        Param { key: "gammaHue", label: "Hue", group: "Gamma", default: 30.0, ..Param::HUE },
        Param { key: "gammaAmount", label: "Amount", ..Param::PERCENT },
        Param { key: "gainHue", label: "Hue", group: "Gain", default: 45.0, ..Param::HUE },
        Param { key: "gainAmount", label: "Amount", default: 20.0, ..Param::PERCENT },
    ],
    swatch: Swatch::Ramp,
    overlay: "",
    build,
};

fn build(v: &[f32], _: &Frame) -> Option<Step> {
    Some(Step::Colour(op(LiftGammaGain::from_hues((v[0], v[1]), (v[2], v[3]), (v[4], v[5])))))
}

pub fn op(l: LiftGammaGain) -> Box<dyn ColourOp> {
    Box::new(Lgg {
        lift: l.lift.map(|v| (v - 0.5) * 0.5),
        gain: l.gain.map(|v| 2.0f32.powf((v - 0.5) * 2.0)),
        inv_gamma: l.gamma.map(|v| 2.0f32.powf((0.5 - v) * 2.0)),
    })
}

struct Lgg {
    lift: [f32; 3],
    gain: [f32; 3],
    inv_gamma: [f32; 3],
}

impl ColourOp for Lgg {
    fn per_channel(&self) -> bool {
        true
    }

    fn channel(&self, i: usize, v: f32) -> f32 {
        let v = self.gain[i] * (v + self.lift[i] * (1.0 - v));
        if v > 0.0 { v.powf(self.inv_gamma[i]) } else { v }
    }
}
