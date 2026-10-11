//! Die Tools der frei sortierten Kette ([`crate::Chain`]), eines pro Datei.
//!
//! Jede Datei hat ein `pub const TOOL: ToolDef`: Name, Beschreibung, Regler und eine Funktion, die
//! aus den Reglerwerten das fertige Tool baut. Plug-in-Manifest, Parameter-Leser und Oberfläche
//! von Looksmaxxer holen sich alles aus [`ALL`]; sonst muss nirgends etwas angepasst werden.
//!
//! - **Tool hinzufügen:** eine bestehende Datei als Vorlage kopieren (`exposure.rs` ist die
//!   kleinste), `id`, Regler und Rechnung anpassen, unten `pub mod …;` eintragen und `&….::TOOL`
//!   in [`ALL`] aufnehmen. Die Reihenfolge in [`ALL`] ist die im Werkzeugkasten.
//! - **Tool entfernen:** aus [`ALL`] streichen (und die Datei samt `mod`-Zeile löschen, wenn sie
//!   nichts anderes mehr braucht). Gespeicherte Looks, die es benutzen, lassen es einfach weg.
//! - **Tool weiterentwickeln:** nur seine Datei ändern. Wer die Bedeutung eines Reglers ändert,
//!   ändert auch, was gespeicherte Looks damit ergeben; neue Regler gehören ans Ende der Liste.
//!
//! Die Werte kommen in der Reihenfolge von [`ToolDef::params`] an. Ein Tool darf höchstens
//! [`MAX_PARAMS`] Regler haben.

use crate::{ColourOp, PositionOp};

pub mod black_white;
pub mod contrast;
pub mod exposure;
pub mod grad_exposure;
pub mod gradient;
pub mod grain;
pub mod lift_gamma_gain;
pub mod shoulder;
pub mod teal_orange;
pub mod vignette;
pub mod warm_cool;

/// Alle Tools, in der Reihenfolge des Werkzeugkastens.
pub const ALL: &[&ToolDef] = &[&warm_cool::TOOL, &exposure::TOOL, &contrast::TOOL, &shoulder::TOOL, &teal_orange::TOOL, &lift_gamma_gain::TOOL, &black_white::TOOL, &gradient::TOOL, &grad_exposure::TOOL, &vignette::TOOL, &grain::TOOL];

/// So viele Regler darf ein Tool haben (das PhotoCraft-Plug-in packt je Platz drei Zahlen zu je drei Werten).
pub const MAX_PARAMS: usize = 9;

/// Das Tool mit dieser `id`.
pub fn find(id: &str) -> Option<&'static ToolDef> {
    ALL.iter().copied().find(|t| t.id == id)
}

/// Ein Tool: was die Oberfläche zeigt, und wie es gebaut wird.
pub struct ToolDef {
    /// Kennung in gespeicherten Looks und im Plug-in, nur Buchstaben und Ziffern. Nie ändern.
    pub id: &'static str,
    pub name: &'static str,
    /// Ein Satz für die Oberfläche.
    pub description: &'static str,
    pub params: &'static [Param],
    /// Worauf die Kachel das Tool live zeigt.
    pub swatch: Swatch,
    /// Ein Griff auf dem Bild, den die Oberfläche kennt, oder leer. `"ellipse"` braucht die Regler
    /// `size`, `softness`, `centerX`, `centerY`, `aspect` und `anamorphic` (siehe `vignette.rs`),
    /// `"points"` die Regler `x1`, `y1`, `x2`, `y2` in % von Breite und Höhe ab der Mitte (siehe
    /// `gradient.rs`).
    pub overlay: &'static str,
    /// Baut das Tool aus seinen Werten (eine Zahl je Regler). `None`, wenn es nichts tut.
    pub build: fn(&[f32], &Frame) -> Option<Step>,
}

impl ToolDef {
    /// Die Vorgaben aller Regler.
    pub fn defaults(&self) -> Vec<f32> {
        self.params.iter().map(|p| p.default).collect()
    }
}

/// Das Bild, auf dem die Kachel eines Tools seine Wirkung zeigt.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Swatch {
    /// Oben ein Graukeil, unten ein Farbkeil: für Farb-Tools.
    Ramp,
    /// Eine graue Fläche: für Tools, die von der Lage abhängen (Vignette).
    Flat,
    /// Ein Verlauf von Weiß (oben links) nach Schwarz (unten rechts): für Tonwert-Tools (Contrast).
    Gray,
}

impl Swatch {
    pub fn name(self) -> &'static str {
        match self {
            Swatch::Ramp => "ramp",
            Swatch::Flat => "flat",
            Swatch::Gray => "gray",
        }
    }
}

/// Ein Regler (oder ein Schalter, `toggle`).
pub struct Param {
    /// Kennung in gespeicherten Looks. Nie ändern.
    pub key: &'static str,
    pub label: &'static str,
    pub min: f32,
    pub max: f32,
    pub step: f32,
    pub default: f32,
    /// Hinter dem Wert, etwa `%` oder ` EV`.
    pub unit: &'static str,
    /// Nachkommastellen in der Anzeige.
    pub digits: u8,
    /// Positive Werte mit `+` zeigen.
    pub signed: bool,
    /// Ein Farbton (0–360°): Die Oberfläche zeigt einen Farbbalken darunter.
    pub hue: bool,
    /// Überschrift über diesem Regler, etwa „Lift"; leer = keine.
    pub group: &'static str,
    /// Der Wert, der nichts tut: Dorthin setzt ein Doppelklick, und von dort aus füllt sich der Regler.
    pub neutral: f32,
    /// Ein Schalter statt eines Reglers: Werte 0 (aus) und 1 (an).
    pub toggle: bool,
}

impl Param {
    /// Ein Regler 0–100 %, Vorgabe 0. Mit `..Param::PERCENT` nur ändern, was anders ist.
    pub const PERCENT: Param = Param { key: "", label: "", min: 0.0, max: 100.0, step: 1.0, default: 0.0, unit: "%", digits: 0, signed: false, hue: false, group: "", neutral: 0.0, toggle: false };
    /// Ein Schalter, Vorgabe aus.
    pub const TOGGLE: Param = Param { max: 1.0, unit: "", toggle: true, ..Param::PERCENT };
    /// Ein Farbton 0–360°.
    pub const HUE: Param = Param { max: 360.0, unit: "°", hue: true, ..Param::PERCENT };
}

/// Die Arbeitsfläche in Pixeln, für Tools, die von der Lage abhängen. Pixelpositionen zählen ab
/// ihrer linken oberen Ecke. `seed` unterscheidet die Bilder eines Videos (Korn), bei Fotos 0.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub width: f32,
    pub height: f32,
    pub seed: u32,
}

impl Frame {
    pub fn of(width: f32, height: f32) -> Frame {
        Frame { width, height, seed: 0 }
    }

    pub fn centre(&self) -> [f32; 2] {
        [self.width / 2.0, self.height / 2.0]
    }

    pub fn half_diagonal(&self) -> f32 {
        0.5 * (self.width * self.width + self.height * self.height).sqrt()
    }
}

/// Ein fertig gebautes Tool.
pub enum Step {
    Colour(Box<dyn ColourOp>),
    Position(Box<dyn PositionOp>),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_is_well_formed() {
        let frame = Frame::of(64.0, 48.0);
        for (i, tool) in ALL.iter().enumerate() {
            assert!(!tool.id.is_empty() && tool.id.bytes().all(|b| b.is_ascii_alphanumeric()), "id {:?}", tool.id);
            assert!(ALL[..i].iter().all(|t| t.id != tool.id), "{} doppelt", tool.id);
            assert!(!tool.params.is_empty() && tool.params.len() <= MAX_PARAMS, "{}: {} Regler", tool.id, tool.params.len());
            for (k, p) in tool.params.iter().enumerate() {
                assert!(!p.key.is_empty() && tool.params[..k].iter().all(|q| q.key != p.key), "{}.{}", tool.id, p.key);
                assert!(p.min < p.max && (p.min..=p.max).contains(&p.default) && (p.min..=p.max).contains(&p.neutral) && p.step > 0.0, "{}.{}", tool.id, p.key);
            }
            // Mit den Vorgaben und an den Grenzen gebaut, liefert jedes Tool Zahlen.
            for values in [tool.defaults(), tool.params.iter().map(|p| p.min).collect(), tool.params.iter().map(|p| p.max).collect()] {
                let c = match (tool.build)(&values, &frame) {
                    Some(Step::Colour(op)) => op.apply([0.2, 0.5, 0.8]),
                    Some(Step::Position(op)) => op.pixel([0.2, 0.5, 0.8], 1.5, 1.5),
                    None => continue,
                };
                assert!(c.iter().all(|v| v.is_finite()), "{}: {c:?}", tool.id);
            }
            assert_eq!(find(tool.id).map(|t| t.id), Some(tool.id));
        }
    }
}
