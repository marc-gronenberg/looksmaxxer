//! Exposure: heller oder dunkler in Blendenstufen, im linearen Licht.

use super::{Frame, Param, Step, Swatch, ToolDef};
use crate::{ColourOp, Gain};

pub const TOOL: ToolDef = ToolDef {
    id: "exposure",
    name: "Exposure",
    description: "Brighter or darker, in stops, in linear light.",
    params: &[Param { key: "stops", label: "Exposure", min: -4.0, max: 4.0, step: 0.05, default: 0.5, unit: " EV", digits: 2, signed: true, ..Param::PERCENT }],
    swatch: Swatch::Ramp,
    overlay: "",
    build,
};

fn build(v: &[f32], _: &Frame) -> Option<Step> {
    Some(Step::Colour(op(v[0])))
}

/// Belichtung in Blenden (−4…4).
pub fn op(stops: f32) -> Box<dyn ColourOp> {
    Box::new(Gain([2.0f32.powf(stops.clamp(-4.0, 4.0)); 3]))
}
