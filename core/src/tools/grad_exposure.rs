//! Grad Exposure: ein Verlaufsfilter in Blendenstufen, etwa um einen hellen Himmel abzudunkeln.
//! Am Startpunkt wirken die vollen Stufen, zum Endpunkt hin weniger, dahinter nichts. Color Balance
//! tönt dabei, ohne die Helligkeit im Mittel zu ändern. Geometrie wie `gradient.rs`.

use super::{gradient, Frame, Param, Step, Swatch, ToolDef};
use crate::hue_colour;

pub const TOOL: ToolDef = ToolDef {
    id: "gradExposure",
    name: "Grad Exposure",
    description: "A graduated exposure filter in stops, full at the start point and fading out towards the end point. Drag the points on the image.",
    params: &[
        Param { key: "stops", label: "Stops", min: -4.0, max: 4.0, step: 0.01, default: -1.0, unit: " EV", digits: 2, signed: true, ..Param::PERCENT },
        Param { key: "x1", label: "Start X", min: -100.0, max: 100.0, step: 0.1, digits: 1, signed: true, ..Param::PERCENT },
        Param { key: "y1", label: "Start Y", min: -100.0, max: 100.0, step: 0.1, default: -50.0, digits: 1, signed: true, ..Param::PERCENT },
        Param { key: "x2", label: "End X", min: -100.0, max: 100.0, step: 0.1, digits: 1, signed: true, ..Param::PERCENT },
        Param { key: "y2", label: "End Y", min: -100.0, max: 100.0, step: 0.1, digits: 1, signed: true, ..Param::PERCENT },
        Param { key: "falloff", label: "Falloff", max: 1.0, step: 0.01, default: 0.5, neutral: 0.5, unit: "", digits: 2, ..Param::PERCENT },
        Param { key: "hue", label: "Hue", group: "Color Balance", default: 35.0, ..Param::HUE },
        Param { key: "saturation", label: "Saturation", ..Param::PERCENT },
    ],
    swatch: Swatch::Flat,
    overlay: "points",
    build,
};

fn build(v: &[f32], frame: &Frame) -> Option<Step> {
    let gain = 2.0f32.powf(v[0].clamp(-4.0, 4.0));
    let filter = hue_colour(v[6], v[7]).map(|c| 2.0 * c * gain);
    gradient::filter_step(&v[1..6], frame, 1.0, filter, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stops_fade_from_start_to_end() {
        let Some(Step::Position(op)) = build(&[-1.0, 0.0, -50.0, 0.0, 0.0, 0.5, 0.0, 0.0], &Frame::of(100.0, 50.0)) else { panic!("kein Filter") };
        let lin = |c: f32| crate::srgb_to_linear(c);
        let grey = [0.5; 3];
        // Oben eine Blende dunkler, auf halber Strecke eine halbe, ab dem Ende nichts.
        assert!((lin(op.pixel(grey, 50.0, 0.0)[0]) / lin(0.5) - 0.5).abs() < 1e-4);
        assert!((lin(op.pixel(grey, 50.0, 12.5)[0]) / lin(0.5) - 0.5f32.sqrt()).abs() < 1e-4);
        assert_eq!(op.pixel(grey, 50.0, 30.0), grey);
        assert!(build(&[0.0, 0.0, -50.0, 0.0, 0.0, 0.5, 35.0, 0.0], &Frame::of(10.0, 10.0)).is_none());
    }
}
