//! Contrast: Kontrast im linearen Licht um einen Drehpunkt (Pivot), danach eine
//! Belichtungskorrektur, die die Helligkeit wieder ausgleichen kann.

use super::{Frame, Param, Step, Swatch, ToolDef};
use crate::{linear_to_srgb, srgb_to_linear, ColourOp};

pub const TOOL: ToolDef = ToolDef {
    id: "contrast",
    name: "Contrast",
    description: "Contrast in linear light around a pivot. Exposure compensation evens out the brightness afterwards.",
    params: &[
        Param { key: "contrast", label: "Contrast", min: -1.0, max: 1.0, step: 0.001, digits: 3, unit: "", signed: true, ..Param::PERCENT },
        // 0,18 ist das mittlere Grau im linearen Licht.
        Param { key: "pivot", label: "Pivot", min: 0.01, max: 1.0, step: 0.001, default: 0.18, neutral: 0.18, digits: 3, unit: "", ..Param::PERCENT },
        Param { key: "compensation", label: "Exposure Comp.", min: -4.0, max: 4.0, step: 0.01, digits: 2, unit: " EV", signed: true, ..Param::PERCENT },
    ],
    swatch: Swatch::Gray,
    overlay: "",
    build,
};

fn build(v: &[f32], _: &Frame) -> Option<Step> {
    let (contrast, pivot, compensation) = (v[0].clamp(-1.0, 1.0), v[1].clamp(0.01, 1.0), v[2].clamp(-4.0, 4.0));
    if contrast == 0.0 && compensation == 0.0 {
        return None;
    }
    // Kontrast 1 verdoppelt die Steigung um den Pivot, −1 halbiert sie.
    Some(Step::Colour(Box::new(Curve { power: 2.0f32.powf(contrast), pivot, gain: 2.0f32.powf(compensation) })))
}

struct Curve {
    power: f32,
    pivot: f32,
    gain: f32,
}

impl ColourOp for Curve {
    fn per_channel(&self) -> bool {
        true
    }

    fn channel(&self, _: usize, v: f32) -> f32 {
        let l = srgb_to_linear(v.max(0.0));
        linear_to_srgb(self.pivot * (l / self.pivot).powf(self.power) * self.gain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn curve(v: [f32; 3]) -> Box<dyn ColourOp> {
        match build(&v, &Frame::of(1.0, 1.0)) {
            Some(Step::Colour(op)) => op,
            _ => panic!("kein Contrast"),
        }
    }

    #[test]
    fn the_pivot_stays_and_contrast_spreads_around_it() {
        let pivot = linear_to_srgb(0.18);
        let more = curve([0.5, 0.18, 0.0]);
        assert!((more.channel(0, pivot) - pivot).abs() < 1e-4);
        assert!(more.channel(0, 0.2) < 0.2 && more.channel(0, 0.8) > 0.8);
        let less = curve([-0.5, 0.18, 0.0]);
        assert!(less.channel(0, 0.2) > 0.2 && less.channel(0, 0.8) < 0.8);
        // Belichtungskorrektur allein: eine Blende heller.
        let comp = curve([0.0, 0.18, 1.0]);
        assert!((srgb_to_linear(comp.channel(0, 0.5)) - 2.0 * srgb_to_linear(0.5)).abs() < 1e-4);
        assert!(build(&[0.0, 0.5, 0.0], &Frame::of(1.0, 1.0)).is_none());
    }
}
