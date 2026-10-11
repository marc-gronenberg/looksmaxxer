//! Shoulder: Lichter laufen oberhalb des Einsatzpunkts weich aus, statt hart zu clippen.

use super::{Frame, Param, Step, Swatch, ToolDef};
use crate::{ColourOp, Shoulder};

pub const TOOL: ToolDef = ToolDef {
    id: "shoulder",
    name: "Shoulder",
    description: "Highlights above the start point roll off softly instead of clipping.",
    params: &[
        Param { key: "start", label: "Start", default: 70.0, ..Param::PERCENT },
        Param { key: "amount", label: "Amount", default: 30.0, ..Param::PERCENT },
    ],
    swatch: Swatch::Ramp,
    overlay: "",
    build,
};

fn build(v: &[f32], _: &Frame) -> Option<Step> {
    op(Shoulder { start: v[0], amount: v[1] }).map(Step::Colour)
}

/// `None`, wenn die Schulter nichts tut.
pub fn op(sh: Shoulder) -> Option<Box<dyn ColourOp>> {
    let (start, k) = ((sh.start / 100.0).clamp(0.0, 0.999), (sh.amount / 100.0).clamp(0.0, 1.0) * 2.0);
    (k > 1e-4).then(|| Box::new(Curve { start, k }) as Box<dyn ColourOp>)
}

struct Curve {
    start: f32,
    /// Krümmung.
    k: f32,
}

impl ColourOp for Curve {
    fn per_channel(&self) -> bool {
        true
    }

    fn channel(&self, _: usize, v: f32) -> f32 {
        let (s, k) = (self.start, self.k);
        if !(v > s) {
            return v;
        }
        let u = (v - s) / (1.0 - s);
        s + (1.0 - s) * (1.0 - (-k * u).exp()) / k
    }
}
