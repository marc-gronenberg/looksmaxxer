//! Warm/Cool: Farbtemperatur und Tönung, im linearen Licht, bei gleicher Helligkeit.

use super::{Frame, Param, Step, Swatch, ToolDef};
use crate::{luma, ColourOp, Gain, WarmCool};

pub const TOOL: ToolDef = ToolDef {
    id: "warmCool",
    name: "Warm/Cool",
    description: "Colour temperature and tint, in linear light. Brightness stays.",
    params: &[
        Param { key: "temperature", label: "Temperature", min: -100.0, default: 25.0, unit: "", signed: true, ..Param::PERCENT },
        Param { key: "tint", label: "Tint", min: -100.0, unit: "", signed: true, ..Param::PERCENT },
    ],
    swatch: Swatch::Ramp,
    overlay: "",
    build,
};

fn build(v: &[f32], _: &Frame) -> Option<Step> {
    Some(Step::Colour(op(WarmCool { temperature: v[0], tint: v[1] })))
}

pub fn op(wc: WarmCool) -> Box<dyn ColourOp> {
    let t = (wc.temperature / 100.0).clamp(-1.0, 1.0);
    let m = (wc.tint / 100.0).clamp(-1.0, 1.0);
    // Wärmer: Rot hoch, Blau runter. Tönung: + = Magenta, − = Grün.
    let mut g = [(0.45 * t + 0.15 * m).exp(), (-0.30 * m).exp(), (-0.45 * t + 0.15 * m).exp()];
    // Helligkeit halten.
    let y = luma(g);
    for v in &mut g {
        *v /= y;
    }
    Box::new(Gain(g))
}
