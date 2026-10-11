//! Black & White: Schwarzweiß mit einstellbarer Gewichtung der Farbkanäle („Component Balance"),
//! wie ein Farbfilter vor einem Schwarzweißfilm. Gemischt wird im linearen Licht; die Gewichte
//! werden auf die Summe 1 gebracht, neutrales Grau bleibt also gleich hell.

use super::{Frame, Param, Step, Swatch, ToolDef};
use crate::{linear_to_srgb, srgb_to_linear, ColourOp};

const WEIGHT: Param = Param { min: 0.0, max: 1.0, step: 0.001, digits: 3, unit: "", ..Param::PERCENT };

pub const TOOL: ToolDef = ToolDef {
    id: "blackWhite",
    name: "Black & White",
    description: "Turns the image black and white. The balance sets how bright each colour becomes, like a colour filter on black-and-white film.",
    params: &[
        // Vorgabe: die Luminanzgewichte von Rec. 709, auf das Raster der Schrittweite gerundet.
        Param { key: "red", label: "Red", group: "Component Balance", default: 0.213, neutral: 0.213, ..WEIGHT },
        Param { key: "green", label: "Green", default: 0.715, neutral: 0.715, ..WEIGHT },
        Param { key: "blue", label: "Blue", default: 0.072, neutral: 0.072, ..WEIGHT },
    ],
    swatch: Swatch::Ramp,
    overlay: "",
    build,
};

fn build(v: &[f32], _: &Frame) -> Option<Step> {
    let w = [v[0].clamp(0.0, 1.0), v[1].clamp(0.0, 1.0), v[2].clamp(0.0, 1.0)];
    let sum = w[0] + w[1] + w[2];
    // Alle Gewichte null: gleich verteilen.
    let w = if sum > 1e-6 { w.map(|x| x / sum) } else { [1.0 / 3.0; 3] };
    Some(Step::Colour(Box::new(Mono(w))))
}

struct Mono([f32; 3]);

impl ColourOp for Mono {
    fn apply(&self, c: [f32; 3]) -> [f32; 3] {
        let l = c.map(|v| srgb_to_linear(v.max(0.0)));
        let y = linear_to_srgb(self.0[0] * l[0] + self.0[1] * l[1] + self.0[2] * l[2]);
        [y; 3]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mono(v: [f32; 3]) -> Box<dyn ColourOp> {
        match build(&v, &Frame::of(1.0, 1.0)) {
            Some(Step::Colour(op)) => op,
            _ => panic!("kein Black & White"),
        }
    }

    #[test]
    fn grey_stays_and_the_balance_decides_brightness() {
        let standard = mono([0.2126, 0.7152, 0.0722]);
        let g = standard.apply([0.4; 3]);
        assert!(g.iter().all(|v| (v - 0.4).abs() < 1e-4), "{g:?}");
        let red = [0.8, 0.1, 0.1];
        let out = standard.apply(red);
        assert!(out[0] == out[1] && out[1] == out[2]);
        // Mehr Rot im Filter: Rotes wird heller.
        let red_filter = mono([1.0, 0.2, 0.0]);
        assert!(red_filter.apply(red)[0] > out[0] + 0.2);
        // Alles null: gleich verteilt, kein Fehler.
        assert!(mono([0.0; 3]).apply(red)[0].is_finite());
    }
}
