//! Look-Kette: der gemeinsame Kern.
//!
//! Sechs Tools in fester Reihenfolge, jedes einzeln abschaltbar:
//! Warm/Cool → Belichtung → Schulter → Teal-Orange → Lift/Gamma/Gain → Vignette,
//! am Ende eine Gesamtstärke.
//!
//! Der Kern kennt keine der beiden Apps. Er rechnet auf sRGB-kodierten Farben **ohne**
//! Alpha-Vormultiplikation; die Adapter (`effectcraft`, `photocraft`) übersetzen Parameter und
//! Pixelformat der jeweiligen Plug-in-Schnittstelle.
//!
//! Alle reinen Farb-Tools lassen sich in eine 3D-LUT ([`Lut`]) vorrechnen und pro Pixel nur
//! nachschlagen; die Vignette hängt von der Position ab und läuft danach.
//!
//! Neben der festen Kette ([`Grade`] und [`Look`]) gibt es eine frei sortierte ([`Chain`]):
//! die Tools aus [`tools`] in beliebiger Reihenfolge, jedes auch mehrfach. Die nutzt Looksmaxxer.
//! Jedes Tool ist dort eine eigene Datei; hinzufügen und entfernen siehe [`tools`].

#![forbid(unsafe_code)]

pub mod json;
pub mod tools;

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

pub(crate) fn luma(c: [f32; 3]) -> f32 {
    c[0] * LUMA[0] + c[1] * LUMA[1] + c[2] * LUMA[2]
}

pub(crate) fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

pub(crate) fn linear_to_srgb(v: f32) -> f32 {
    if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

pub(crate) fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    if b <= a {
        return if x < a { 0.0 } else { 1.0 };
    }
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Warm/Cool: Temperatur und Tönung, je −100…100.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WarmCool {
    pub temperature: f32,
    pub tint: f32,
}

/// Schulter: Einsatzpunkt und Stärke, je 0…100.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shoulder {
    pub start: f32,
    pub amount: f32,
}

/// Teal-Orange: Stärke und Hautschutz, je 0…100.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TealOrange {
    pub amount: f32,
    pub skin: f32,
}

/// Lift/Gamma/Gain als drei Farben (RGB 0…1); mittleres Grau (0.5) ist neutral.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LiftGammaGain {
    pub lift: [f32; 3],
    pub gamma: [f32; 3],
    pub gain: [f32; 3],
}

impl LiftGammaGain {
    /// Aus Farbton (Grad, 0 = Rot) und Stärke (0…100) je Bereich, für Hosts ohne Farbfeld.
    pub fn from_hues(lift: (f32, f32), gamma: (f32, f32), gain: (f32, f32)) -> LiftGammaGain {
        LiftGammaGain { lift: hue_colour(lift.0, lift.1), gamma: hue_colour(gamma.0, gamma.1), gain: hue_colour(gain.0, gain.1) }
    }
}

/// Grau, um `amount` Prozent in Richtung des Farbtons `hue` (Grad) verschoben. Die Verschiebung
/// ist über die drei Kanäle im Mittel null, die Helligkeit bleibt also ungefähr gleich.
fn hue_colour(hue: f32, amount: f32) -> [f32; 3] {
    let k = 0.5 * (amount / 100.0).clamp(0.0, 1.0);
    let h = hue.to_radians();
    let third = 120.0f32.to_radians();
    [0.5 + k * h.cos(), 0.5 + k * (h - third).cos(), 0.5 + k * (h - 2.0 * third).cos()]
}

/// Die Einstellungen der reinen Farb-Tools. `None` = Tool ist ausgeschaltet.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ColorSettings {
    pub warm_cool: Option<WarmCool>,
    /// Belichtung in Blenden (−4…4).
    pub exposure: Option<f32>,
    pub shoulder: Option<Shoulder>,
    pub teal_orange: Option<TealOrange>,
    pub lift_gamma_gain: Option<LiftGammaGain>,
}

/// Ein Farb-Tool, fertig zum Rechnen. Jedes Tool in [`tools`] liefert eines (oder eine
/// Abdunklung nach Lage, [`PositionOp`]).
pub trait ColourOp {
    /// Behandelt das Tool jeden Kanal für sich? Dann ruft der Kern nur [`ColourOp::channel`] und
    /// kann die Rechnung für die LUT pro Kanal vorziehen.
    fn per_channel(&self) -> bool {
        false
    }

    /// Kanal `i` mit dem Wert `v`, für Tools, die jeden Kanal für sich behandeln.
    fn channel(&self, _i: usize, v: f32) -> f32 {
        v
    }

    /// Das Tool auf eine sRGB-kodierte Farbe (ohne Alpha).
    fn apply(&self, c: [f32; 3]) -> [f32; 3] {
        [self.channel(0, c[0]), self.channel(1, c[1]), self.channel(2, c[2])]
    }

    /// Nur Faktoren pro Kanal im linearen Licht? Zwei solche direkt hintereinander werden zu einem.
    fn linear_gain(&self) -> Option<[f32; 3]> {
        None
    }
}

/// Ein Tool, das von der Lage des Pixels abhängt (die Vignette).
pub trait PositionOp {
    /// `c` sRGB-kodiert, `x` und `y` die Pixelmitte.
    fn pixel(&self, c: [f32; 3], x: f32, y: f32) -> [f32; 3];

    /// Wie die Grafikkarte dasselbe rechnet (siehe [`Gpu`]), oder `None`, wenn das Tool dort nicht
    /// läuft (dann rechnet die Vorschau die ganze Kette ohne Grafikkarte).
    fn gpu(&self) -> Option<Gpu> {
        None
    }
}

/// So viele Zahlen bekommt der GLSL-Code eines Tools ([`Gpu::values`]).
pub const GPU_VALUES: usize = 12;

/// Ein Tool, das von der Lage abhängt, auf der Grafikkarte.
///
/// `code` ist der Rumpf einer GLSL-Funktion `vec3 f(vec3 c, vec2 p, float u[12])`: `c` sRGB-kodiert,
/// `p` die Pixelmitte (Ursprung oben links), `u` die `values`. Zur Hand sind `ss` (wie
/// [`smoothstep`]), `luma`, `lin` (sRGB → linear) und `srgb` (linear → sRGB). Er soll dasselbe
/// rechnen wie [`PositionOp::pixel`].
pub struct Gpu {
    pub code: &'static str,
    pub values: [f32; GPU_VALUES],
}

/// Faktoren pro Kanal im linearen Licht (Warm/Cool, Belichtung).
pub struct Gain(pub [f32; 3]);

impl ColourOp for Gain {
    fn per_channel(&self) -> bool {
        true
    }

    fn channel(&self, i: usize, v: f32) -> f32 {
        linear_to_srgb(srgb_to_linear(v.max(0.0)) * self.0[i])
    }

    fn linear_gain(&self) -> Option<[f32; 3]> {
        Some(self.0)
    }
}

/// Hängt `op` an. Zwei Faktoren im linearen Licht direkt hintereinander werden zu einem.
fn push(ops: &mut Vec<Box<dyn ColourOp>>, op: Box<dyn ColourOp>) {
    if let (Some(a), Some(b)) = (ops.last().and_then(|last| last.linear_gain()), op.linear_gain()) {
        *ops.last_mut().expect("eben gelesen") = Box::new(Gain([a[0] * b[0], a[1] * b[1], a[2] * b[2]]));
        return;
    }
    ops.push(op);
}

/// Farb-Tools in einer festen Folge, vorberechnet.
pub struct Grade {
    ops: Vec<Box<dyn ColourOp>>,
    /// So viele Tools am Anfang behandeln jeden Kanal für sich.
    separable: usize,
}

impl Grade {
    /// Die feste Kette: Warm/Cool → Belichtung → Schulter → Teal-Orange → Lift/Gamma/Gain.
    pub fn new(s: &ColorSettings) -> Grade {
        let mut ops = Vec::new();
        if let Some(wc) = s.warm_cool {
            push(&mut ops, tools::warm_cool::op(wc));
        }
        if let Some(stops) = s.exposure {
            push(&mut ops, tools::exposure::op(stops));
        }
        ops.extend(s.shoulder.and_then(tools::shoulder::op));
        ops.extend(s.teal_orange.and_then(tools::teal_orange::op));
        ops.extend(s.lift_gamma_gain.map(tools::lift_gamma_gain::op));
        Grade::from_ops(ops)
    }

    fn from_ops(ops: Vec<Box<dyn ColourOp>>) -> Grade {
        let separable = ops.iter().take_while(|op| op.per_channel()).count();
        Grade { ops, separable }
    }

    /// Kein Farb-Tool ist an.
    pub fn is_identity(&self) -> bool {
        self.ops.is_empty()
    }

    /// Die Farb-Tools der Reihe nach auf eine sRGB-kodierte Farbe (ohne Alpha).
    pub fn apply(&self, c: [f32; 3]) -> [f32; 3] {
        self.across_channels([self.per_channel(0, c[0]), self.per_channel(1, c[1]), self.per_channel(2, c[2])])
    }

    /// Die Tools am Anfang, die jeden Kanal für sich behandeln (Kanal `i`, Wert `v`).
    fn per_channel(&self, i: usize, v: f32) -> f32 {
        self.ops[..self.separable].iter().fold(v, |v, op| op.channel(i, v))
    }

    /// Alle Tools danach, ab dem ersten, das alle drei Kanäle zusammen braucht.
    fn across_channels(&self, c: [f32; 3]) -> [f32; 3] {
        self.ops[self.separable..].iter().fold(c, |c, op| op.apply(c))
    }
}

/// Kantenlänge der 3D-LUT.
pub const LUT_SIZE: usize = 33;
const N: usize = LUT_SIZE;
/// Abstände der Nachbarn in der Tabelle: Rot, Grün, Blau.
const SX: usize = 3;
const SY: usize = 3 * N;
const SZ: usize = 3 * N * N;
/// So viele Werte umfasst ein Würfel aus acht Nachbarn, vom ersten bis zum letzten.
const CELL: usize = SX + SY + SZ + 3;

/// Eine 3D-LUT der Farb-Tools.
pub struct Lut {
    /// `N³` Einträge zu je drei Werten, Rot läuft am schnellsten.
    data: Vec<f32>,
    /// Eindeutig je gebauter LUT: So erkennt die Oberfläche, ob sie sie neu hochladen muss.
    id: u64,
}

impl Lut {
    /// Rechnet die LUT.
    ///
    /// Die Tools, die jeden Kanal für sich behandeln (und die teuren Potenzen enthalten), werden
    /// nur einmal je Kanal und Stützstelle gerechnet, nicht für alle `N³` Farben. Das Ergebnis
    /// ist an jeder Stützstelle dasselbe wie [`Grade::apply`].
    pub fn build(grade: &Grade) -> Lut {
        let step = 1.0 / (N - 1) as f32;
        let axis = |i: usize| -> Vec<f32> { (0..N).map(|k| grade.per_channel(i, k as f32 * step)).collect() };
        let (red, green, blue) = (axis(0), axis(1), axis(2));
        let mut data = Vec::with_capacity(N * N * N * 3);
        for b in &blue {
            for g in &green {
                for r in &red {
                    data.extend(grade.across_channels([*r, *g, *b]));
                }
            }
        }
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Lut { data, id: NEXT.fetch_add(1, Ordering::Relaxed) }
    }

    /// Lohnt die LUT für `pixels` Pixel, wenn sie eigens dafür gerechnet werden muss? Darunter
    /// ist die direkte Rechnung schneller.
    /// Die Tabelle: `LUT_SIZE³` Einträge zu je drei Werten, Rot läuft am schnellsten.
    pub fn data(&self) -> &[f32] {
        &self.data
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn worthwhile(pixels: usize) -> bool {
        pixels >= N * N * N / 2
    }

    /// Trilinear nachschlagen; `c` liegt in 0–1.
    #[inline(always)]
    pub fn sample(&self, c: [f32; 3]) -> [f32; 3] {
        let m = (N - 1) as f32;
        let (x, y, z) = (c[0] * m, c[1] * m, c[2] * m);
        let (x0, y0, z0) = ((x as usize).min(N - 2), (y as usize).min(N - 2), (z as usize).min(N - 2));
        let (fx, fy, fz) = (x - x0 as f32, y - y0 as f32, z - z0 as f32);
        let i = x0 * SX + y0 * SY + z0 * SZ;
        // Mit x0, y0, z0 ≤ N − 2 endet der Würfel spätestens bei 3·N³: immer in der Tabelle.
        let Some(d) = self.data.get(i..i + CELL).and_then(|d| <&[f32; CELL]>::try_from(d).ok()) else { return c };
        let mut out = [0.0f32; 3];
        for (k, o) in out.iter_mut().enumerate() {
            let c00 = d[k] + (d[SX + k] - d[k]) * fx;
            let c10 = d[SY + k] + (d[SY + SX + k] - d[SY + k]) * fx;
            let c01 = d[SZ + k] + (d[SZ + SX + k] - d[SZ + k]) * fx;
            let c11 = d[SZ + SY + k] + (d[SZ + SY + SX + k] - d[SZ + SY + k]) * fx;
            let c0 = c00 + (c10 - c00) * fy;
            let c1 = c01 + (c11 - c01) * fy;
            *o = c0 + (c1 - c0) * fz;
        }
        out
    }
}

/// Vignette: Abdunklung vom Rand her.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vignette {
    cx: f32,
    cy: f32,
    inner: f32,
    outer: f32,
    amount: f32,
}

impl Vignette {
    /// `centre` und `half_diagonal` in Pixeln des Koordinatensystems, in dem später die
    /// Pixelpositionen angegeben werden. Größe 10…200 (100 = volle Abdunklung in den Ecken),
    /// Stärke und Weichheit 0…100. `None`, wenn die Stärke null ist.
    pub fn new(centre: [f32; 2], half_diagonal: f32, size: f32, amount: f32, softness: f32) -> Option<Vignette> {
        if !(amount > 0.0) {
            return None;
        }
        let outer = half_diagonal * (size / 100.0).clamp(0.1, 2.0);
        let inner = outer * (1.0 - (softness / 100.0).clamp(0.0, 1.0));
        Some(Vignette { cx: centre[0], cy: centre[1], inner, outer, amount: (amount / 100.0).clamp(0.0, 1.0) })
    }

    /// Dunkelt `c` ab; `d2` ist der quadrierte Abstand des Pixels zur Mitte.
    #[inline(always)]
    fn darken(&self, c: [f32; 3], d2: f32) -> [f32; 3] {
        if d2 <= self.inner * self.inner {
            return c;
        }
        let k = 1.0 - self.amount * smoothstep(self.inner, self.outer, d2.sqrt());
        [c[0] * k, c[1] * k, c[2] * k]
    }
}

impl PositionOp for Vignette {
    fn pixel(&self, c: [f32; 3], x: f32, y: f32) -> [f32; 3] {
        let (dx, dy) = (x - self.cx, y - self.cy);
        self.darken(c, dx * dx + dy * dy)
    }
}

/// Liegt die Farbe im Würfel, den die LUT abdeckt?
#[inline(always)]
fn in_unit_cube(c: [f32; 3]) -> bool {
    c[0] >= 0.0 && c[0] <= 1.0 && c[1] >= 0.0 && c[1] <= 1.0 && c[2] >= 0.0 && c[2] <= 1.0
}

/// Gesamtstärke: von `src` um `mix` in Richtung `out`. Ein Ergebnis, das keine Zahl ist, lässt
/// den Kanal, wie er war.
#[inline(always)]
fn blend(src: [f32; 3], out: [f32; 3], mix: f32) -> [f32; 3] {
    let one = |s: f32, o: f32| {
        let v = s + (o - s) * mix;
        if v.is_finite() { v.max(0.0) } else { s }
    };
    [one(src[0], out[0]), one(src[1], out[1]), one(src[2], out[2])]
}

/// Alles, was für ein Pixel gebraucht wird: Farb-Tools (direkt oder über die LUT), Vignette und
/// Gesamtstärke.
pub struct Look<'a> {
    grade: &'a Grade,
    lut: Option<&'a Lut>,
    vignette: Option<Vignette>,
    mix: f32,
    /// Mindestens ein Farb-Tool ist an.
    colour: bool,
}

impl<'a> Look<'a> {
    /// `lut` ist die LUT zu `grade`, wenn sich das Vorrechnen lohnt; `mix` die Gesamtstärke 0…1.
    pub fn new(grade: &'a Grade, lut: Option<&'a Lut>, vignette: Option<Vignette>, mix: f32) -> Look<'a> {
        Look { grade, lut, vignette, mix: mix.clamp(0.0, 1.0), colour: !grade.is_identity() }
    }

    /// Ändert der Look überhaupt etwas?
    pub fn is_identity(&self) -> bool {
        self.mix <= 0.0 || (!self.colour && self.vignette.is_none())
    }

    /// Der Abstand einer Zeile zur Vignetten-Mitte, quadriert: einmal pro Zeile rechnen und an
    /// [`Look::pixel`] geben. `y` ist die Mitte der Pixelzeile.
    #[inline]
    pub fn row(&self, y: f32) -> f32 {
        let dy = y - self.vignette.map_or(0.0, |v| v.cy);
        dy * dy
    }

    /// Ein Pixel: `src` ist sRGB-kodiert und nicht vormultipliziert, `x` die Pixelmitte, `dy2`
    /// kommt aus [`Look::row`]. Ein Ergebnis, das keine Zahl ist, lässt den Kanal, wie er war.
    #[inline(always)]
    pub fn pixel(&self, src: [f32; 3], x: f32, dy2: f32) -> [f32; 3] {
        let mut out = src;
        if self.colour {
            out = match self.lut {
                Some(l) if in_unit_cube(src) => l.sample(src),
                // Werte über 1 (32-Bit-Dokumente) und kleine Bilder: direkt rechnen.
                _ => self.grade.apply(src),
            };
        }
        if let Some(v) = &self.vignette {
            let dx = x - v.cx;
            out = v.darken(out, dx * dx + dy2);
        }
        blend(src, out, self.mix)
    }
}

/// Ein Abschnitt einer [`Chain`].
enum Stage {
    /// Farb-Tools direkt hintereinander, mit ihrer LUT, wenn sie sich lohnt.
    Colour(Grade, Option<Rc<Lut>>),
    /// Ein Tool, das von der Lage des Pixels abhängt.
    Position(Box<dyn PositionOp>),
}

/// Eine frei sortierte Kette aus Tools von [`tools`], fertig zum Rechnen.
///
/// Farb-Tools, die direkt aufeinander folgen, teilen sich eine LUT. Ein Tool, das von der Lage
/// abhängt (die Vignette), trennt sie; jeder Abschnitt bekommt dann seine eigene.
pub struct Chain {
    stages: Vec<Stage>,
    mix: f32,
}

/// LUTs, die schon gebaut sind, nach den Tools und Werten ihres Abschnitts. So baut eine Kette, deren
/// Farb-Tools sich nicht geändert haben (etwa beim Ziehen der Vignette), keine LUT neu.
#[derive(Default)]
pub struct LutCache {
    luts: HashMap<String, Rc<Lut>>,
}

/// So viele LUTs hebt ein [`LutCache`] auf (je rund 430 KB).
const LUT_CACHE_MAX: usize = 24;

/// Ein Abschnitt der Kette für die Grafikkarte: eine LUT zum Nachschlagen, oder ein Tool, das von
/// der Lage abhängt ([`PositionOp::gpu`]).
pub enum GpuStage<'a> {
    Lut(&'a Lut),
    Op(Gpu),
}

impl Chain {
    /// `tools` sind die Tools mit ihren Werten, in der Reihenfolge von [`tools::ToolDef::params`].
    /// `frame` beschreibt die Fläche für Tools, die von der Lage abhängen. `mix` ist die
    /// Gesamtstärke 0…1, `pixels` die Zahl der Pixel, die gerechnet werden sollen: Danach
    /// entscheidet sich, ob sich die LUTs lohnen.
    pub fn new(tools: &[(&tools::ToolDef, &[f32])], frame: tools::Frame, mix: f32, pixels: usize) -> Chain {
        Chain::with_cache(tools, frame, mix, pixels, &mut LutCache::default())
    }

    /// Wie [`Chain::new`], nimmt aber LUTs aus `cache` und legt neue dort ab.
    pub fn with_cache(tools: &[(&tools::ToolDef, &[f32])], frame: tools::Frame, mix: f32, pixels: usize, cache: &mut LutCache) -> Chain {
        let mix = if mix > 0.0 { mix.min(1.0) } else { 0.0 };
        let use_lut = mix > 0.0 && Lut::worthwhile(pixels);
        let mut stages = Vec::new();
        let mut ops = Vec::new();
        // Schlüssel des offenen Abschnitts: die Tools und ihre Werte, bitgenau.
        let mut key = String::new();
        let mut close = |ops: &mut Vec<Box<dyn ColourOp>>, stages: &mut Vec<Stage>, key: &mut String| {
            if !ops.is_empty() {
                let grade = Grade::from_ops(std::mem::take(ops));
                let lut = use_lut.then(|| {
                    if cache.luts.len() >= LUT_CACHE_MAX && !cache.luts.contains_key(key.as_str()) {
                        cache.luts.clear();
                    }
                    cache.luts.entry(key.clone()).or_insert_with(|| Rc::new(Lut::build(&grade))).clone()
                });
                stages.push(Stage::Colour(grade, lut));
            }
            key.clear();
        };
        for (tool, values) in tools {
            // Ein Tool, das nichts tut, liefert nichts und trennt auch nichts.
            match (tool.build)(values, &frame) {
                Some(tools::Step::Colour(op)) => {
                    push(&mut ops, op);
                    key.push_str(tool.id);
                    values.iter().for_each(|v| key.push_str(&format!(":{:08x}", v.to_bits())));
                    key.push('|');
                }
                Some(tools::Step::Position(op)) => {
                    close(&mut ops, &mut stages, &mut key);
                    stages.push(Stage::Position(op));
                }
                None => {}
            }
        }
        close(&mut ops, &mut stages, &mut key);
        Chain { stages, mix }
    }

    /// Gesamtstärke 0…1.
    pub fn mix(&self) -> f32 {
        self.mix
    }

    /// Die Kette als Abschnitte für die Grafikkarte, oder `None`, wenn ein Abschnitt dort nicht läuft
    /// (ein Farb-Abschnitt ohne LUT, oder ein Tool ohne [`PositionOp::gpu`]).
    pub fn gpu_stages(&self) -> Option<Vec<GpuStage<'_>>> {
        if self.mix <= 0.0 {
            return Some(Vec::new());
        }
        self.stages
            .iter()
            .map(|stage| match stage {
                Stage::Colour(_, lut) => lut.as_deref().map(GpuStage::Lut),
                Stage::Position(op) => op.gpu().map(GpuStage::Op),
            })
            .collect()
    }

    /// Ändert die Kette überhaupt etwas?
    pub fn is_identity(&self) -> bool {
        self.mix <= 0.0 || self.stages.is_empty()
    }

    /// Ein Pixel: `src` ist sRGB-kodiert und nicht vormultipliziert, `x` und `y` sind die
    /// Pixelmitte. Ein Ergebnis, das keine Zahl ist, lässt den Kanal, wie er war.
    #[inline]
    pub fn pixel(&self, src: [f32; 3], x: f32, y: f32) -> [f32; 3] {
        let mut c = src;
        for stage in &self.stages {
            c = match stage {
                Stage::Colour(grade, lut) => match lut {
                    Some(l) if in_unit_cube(c) => l.sample(c),
                    _ => grade.apply(c),
                },
                Stage::Position(op) => op.pixel(c, x, y),
            };
        }
        blend(src, c, self.mix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> ColorSettings {
        ColorSettings {
            warm_cool: Some(WarmCool { temperature: 25.0, tint: 0.0 }),
            exposure: None,
            shoulder: Some(Shoulder { start: 70.0, amount: 30.0 }),
            teal_orange: Some(TealOrange { amount: 55.0, skin: 60.0 }),
            lift_gamma_gain: None,
        }
    }

    /// Ein kleiner, fester Zufallsgenerator (damit die Tests ohne Abhängigkeiten auskommen).
    fn colours(count: usize) -> Vec<[f32; 3]> {
        let mut s = 0x2545_F491_4F6C_DD1Du64;
        let mut next = || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s >> 40) as f32 / (1u64 << 24) as f32
        };
        (0..count).map(|_| [next(), next(), next()]).collect()
    }

    fn lut_error(settings: &ColorSettings) -> (f32, f32) {
        let grade = Grade::new(settings);
        let lut = Lut::build(&grade);
        let (mut max, mut sum) = (0.0f32, 0.0f64);
        let cs = colours(200_000);
        for c in &cs {
            let (a, b) = (grade.apply(*c), lut.sample(*c));
            for i in 0..3 {
                let e = (a[i] - b[i]).abs();
                max = max.max(e);
                sum += e as f64;
            }
        }
        (max, (sum / (cs.len() * 3) as f64) as f32)
    }

    #[test]
    fn everything_off_changes_nothing() {
        let grade = Grade::new(&ColorSettings::default());
        assert!(grade.is_identity());
        for c in colours(1000) {
            assert_eq!(grade.apply(c), c);
        }
        let look = Look::new(&grade, None, None, 1.0);
        assert!(look.is_identity());
        assert_eq!(look.pixel([0.2, 0.4, 0.6], 3.5, look.row(7.5)), [0.2, 0.4, 0.6]);
    }

    #[test]
    fn neutral_lift_gamma_gain_is_neutral() {
        let neutral = LiftGammaGain::from_hues((0.0, 0.0), (120.0, 0.0), (240.0, 0.0));
        assert_eq!(neutral.lift, [0.5; 3]);
        let grade = Grade::new(&ColorSettings { lift_gamma_gain: Some(neutral), ..Default::default() });
        for c in colours(1000) {
            let o = grade.apply(c);
            for i in 0..3 {
                assert!((o[i] - c[i]).abs() < 1e-5, "{c:?} -> {o:?}");
            }
        }
    }

    #[test]
    fn hue_colour_points_at_its_hue() {
        let red = hue_colour(0.0, 100.0);
        assert!(red[0] > red[1] && red[0] > red[2]);
        let green = hue_colour(120.0, 100.0);
        assert!(green[1] > green[0] && green[1] > green[2]);
        let blue = hue_colour(240.0, 100.0);
        assert!(blue[2] > blue[0] && blue[2] > blue[1]);
        for c in [red, green, blue] {
            assert!(c.iter().all(|v| (0.0..=1.0).contains(v)));
            assert!(((c[0] + c[1] + c[2]) / 3.0 - 0.5).abs() < 1e-5);
        }
    }

    #[test]
    fn mix_blends_and_zero_mix_is_identity() {
        let grade = Grade::new(&defaults());
        let full = Look::new(&grade, None, None, 1.0);
        let half = Look::new(&grade, None, None, 0.5);
        let none = Look::new(&grade, None, None, 0.0);
        assert!(none.is_identity());
        let c = [0.3, 0.5, 0.7];
        let (a, b) = (full.pixel(c, 0.5, 0.0), half.pixel(c, 0.5, 0.0));
        for i in 0..3 {
            assert!((b[i] - (c[i] + a[i]) / 2.0).abs() < 1e-6);
        }
        assert_eq!(none.pixel(c, 0.5, 0.0), c);
    }

    #[test]
    fn vignette_darkens_corners_not_the_centre() {
        let grade = Grade::new(&ColorSettings::default());
        let v = Vignette::new([50.0, 50.0], 70.7, 115.0, 35.0, 50.0);
        let look = Look::new(&grade, None, v, 1.0);
        let c = [0.5, 0.5, 0.5];
        assert_eq!(look.pixel(c, 50.0, look.row(50.0)), c);
        let corner = look.pixel(c, 0.5, look.row(0.5));
        assert!(corner[0] < 0.45 && corner[0] > 0.3, "{corner:?}");
        assert!(Vignette::new([0.0, 0.0], 10.0, 100.0, 0.0, 50.0).is_none());
    }

    #[test]
    fn lut_grid_points_are_exact() {
        let mut all = defaults();
        all.exposure = Some(-0.7);
        all.lift_gamma_gain = Some(LiftGammaGain::from_hues((200.0, 40.0), (30.0, 30.0), (60.0, 40.0)));
        let grade = Grade::new(&all);
        let n = LUT_SIZE;
        let lut = Lut::build(&grade);
        for b in 0..n {
            for g in 0..n {
                for r in 0..n {
                    let c = [r, g, b].map(|k| k as f32 / (n - 1) as f32);
                    let (direct, table) = (grade.apply(c), lut.sample(c));
                    for i in 0..3 {
                        assert!((direct[i] - table[i]).abs() < 1e-5, "{c:?}: {direct:?} vs {table:?}");
                    }
                }
            }
        }
        assert!(Lut::worthwhile(1024 * 256));
        assert!(!Lut::worthwhile(100 * 100));
        // Farben, die keine Zahl sind, bleiben stehen.
        let look = Look::new(&grade, Some(&lut), None, 1.0);
        let odd = look.pixel([f32::NAN, 0.5, 0.5], 0.5, 0.0);
        assert!(odd[0].is_nan() && odd[1].is_finite());
    }

    type Tools = Vec<(&'static tools::ToolDef, Vec<f32>)>;

    fn t(id: &str, values: &[f32]) -> (&'static tools::ToolDef, Vec<f32>) {
        (tools::find(id).expect(id), values.to_vec())
    }

    fn chain(ts: &Tools, frame: tools::Frame, mix: f32, pixels: usize) -> Chain {
        let refs: Vec<(&tools::ToolDef, &[f32])> = ts.iter().map(|(d, v)| (*d, v.as_slice())).collect();
        Chain::new(&refs, frame, mix, pixels)
    }

    const NOWHERE: tools::Frame = tools::Frame { width: 1.0, height: 1.0 };

    /// Die Vorgaben der festen Kette als frei sortierte, mit allen Tools an. `vignette` ist Stärke,
    /// Größe, Weichheit (die übrigen Regler der Vignette neutral: rund und mittig).
    fn fixed_order(vignette: [f32; 3]) -> Tools {
        vec![
            t("warmCool", &[25.0, 0.0]),
            t("exposure", &[-0.7]),
            t("shoulder", &[70.0, 30.0]),
            t("tealOrange", &[55.0, 60.0]),
            t("liftGammaGain", &[200.0, 40.0, 30.0, 30.0, 60.0, 40.0]),
            t("vignette", &[vignette[0], vignette[1], vignette[2], 0.0, 0.0, 0.0, 1.0, 0.0]),
        ]
    }

    #[test]
    fn a_chain_in_the_fixed_order_matches_the_fixed_look() {
        let mut all = defaults();
        all.exposure = Some(-0.7);
        all.lift_gamma_gain = Some(LiftGammaGain::from_hues((200.0, 40.0), (30.0, 30.0), (60.0, 40.0)));
        let frame = tools::Frame::of(80.0, 50.0);
        let grade = Grade::new(&all);
        let lut = Lut::build(&grade);
        let vignette = Vignette::new(frame.centre(), frame.half_diagonal(), 90.0, 60.0, 40.0);
        let tools = fixed_order([60.0, 90.0, 40.0]);
        // Einmal direkt gerechnet (wenige Pixel), einmal über die LUT (viele).
        for (pixels, lut) in [(16, None), (1 << 20, Some(&lut))] {
            let look = Look::new(&grade, lut, vignette, 0.8);
            let chain = chain(&tools, frame, 0.8, pixels);
            for (k, c) in colours(2000).into_iter().enumerate() {
                let (x, y) = ((k % 80) as f32 + 0.5, (k / 80 % 50) as f32 + 0.5);
                // Die Vignette der Kette rechnet als Ellipse in eigener Form: bis auf Rundung gleich.
                let (a, b) = (chain.pixel(c, x, y), look.pixel(c, x, look.row(y)));
                assert!((0..3).all(|i| (a[i] - b[i]).abs() < 1e-5), "{c:?} at {x}, {y}, {pixels} pixels: {a:?} vs {b:?}");
            }
        }
    }

    #[test]
    fn the_order_of_the_tools_matters() {
        let lift = t("exposure", &[2.0]);
        let shoulder = t("shoulder", &[50.0, 100.0]);
        let a = chain(&vec![lift.clone(), shoulder.clone()], NOWHERE, 1.0, 1);
        let b = chain(&vec![shoulder, lift], NOWHERE, 1.0, 1);
        let c = [0.6, 0.5, 0.4];
        let (pa, pb) = (a.pixel(c, 0.5, 0.5), b.pixel(c, 0.5, 0.5));
        // Schulter zuletzt: die Lichter bleiben unter 1. Belichtung zuletzt: sie clippen darüber.
        assert!(pa[0] < 1.0 && pb[0] > 1.0, "{pa:?} vs {pb:?}");

        // Vignette vor oder nach der Belichtung: im Rand verschieden, in der Mitte gleich.
        let frame = tools::Frame::of(100.0, 100.0);
        let v = t("vignette", &[80.0, 100.0, 100.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        let before = chain(&vec![v.clone(), t("exposure", &[1.0])], frame, 1.0, 1);
        let after = chain(&vec![t("exposure", &[1.0]), v], frame, 1.0, 1);
        let g = [0.3, 0.3, 0.3];
        assert_eq!(before.pixel(g, 50.0, 50.0), after.pixel(g, 50.0, 50.0));
        assert_ne!(before.pixel(g, 5.0, 5.0), after.pixel(g, 5.0, 5.0));
    }

    #[test]
    fn tools_can_repeat() {
        let twice = chain(&vec![t("exposure", &[1.0]), t("exposure", &[1.0])], NOWHERE, 1.0, 1);
        let once = chain(&vec![t("exposure", &[2.0])], NOWHERE, 1.0, 1);
        for c in colours(500) {
            let (a, b) = (twice.pixel(c, 0.5, 0.5), once.pixel(c, 0.5, 0.5));
            for i in 0..3 {
                assert!((a[i] - b[i]).abs() < 1e-5, "{c:?}: {a:?} vs {b:?}");
            }
        }
        // Zweimal Teal-Orange ist stärker als einmal.
        let to = t("tealOrange", &[60.0, 0.0]);
        let one = chain(&vec![to.clone()], NOWHERE, 1.0, 1).pixel([0.25, 0.25, 0.25], 0.5, 0.5);
        let two = chain(&vec![to.clone(), to], NOWHERE, 1.0, 1).pixel([0.25, 0.25, 0.25], 0.5, 0.5);
        assert!(two[2] - two[0] > one[2] - one[0], "{one:?} vs {two:?}");
    }

    #[test]
    fn an_empty_chain_or_zero_strength_changes_nothing() {
        let empty = chain(&vec![], NOWHERE, 1.0, 1 << 20);
        assert!(empty.is_identity());
        assert_eq!(empty.pixel([0.2, 0.4, 0.6], 0.5, 0.5), [0.2, 0.4, 0.6]);
        let tools = fixed_order([35.0, 115.0, 50.0]);
        for mix in [0.0, -1.0, f32::NAN] {
            let none = chain(&tools, NOWHERE, mix, 1 << 20);
            assert!(none.is_identity());
            assert_eq!(none.pixel([0.2, 0.4, 0.6], 0.5, 0.5), [0.2, 0.4, 0.6]);
        }
        // Tools, die nichts tun, fallen ganz weg.
        let idle = vec![t("shoulder", &[50.0, 0.0]), t("tealOrange", &[0.0, 50.0]), t("vignette", &[0.0, 100.0, 50.0, 0.0, 0.0, 0.0, 1.0, 0.0])];
        assert!(chain(&idle, NOWHERE, 1.0, 1).is_identity());
    }

    #[test]
    fn each_section_of_a_chain_gets_a_close_lut() {
        let tools = vec![
            t("tealOrange", &[70.0, 60.0]),
            t("exposure", &[0.5]),
            t("vignette", &[50.0, 80.0, 60.0, 0.0, 0.0, 0.0, 1.0, 0.0]),
            t("shoulder", &[60.0, 70.0]),
            t("liftGammaGain", &[200.0, 40.0, 30.0, 30.0, 60.0, 40.0]),
            t("warmCool", &[-30.0, 10.0]),
        ];
        let frame = tools::Frame::of(128.0, 128.0);
        let direct = chain(&tools, frame, 1.0, 1);
        let table = chain(&tools, frame, 1.0, 1 << 20);
        assert_eq!(table.stages.len(), 3);
        assert!(table.stages.iter().all(|s| !matches!(s, Stage::Colour(_, None))));
        let (mut max, mut sum) = (0.0f32, 0.0f64);
        let cs = colours(50_000);
        for (k, c) in cs.iter().enumerate() {
            let (x, y) = ((k % 128) as f32 + 0.5, (k / 128 % 128) as f32 + 0.5);
            let (a, b) = (direct.pixel(*c, x, y), table.pixel(*c, x, y));
            for i in 0..3 {
                let e = (a[i] - b[i]).abs();
                max = max.max(e);
                sum += e as f64;
            }
        }
        let mean = (sum / (cs.len() * 3) as f64) as f32;
        println!("Kette mit zwei LUTs: max error {max:.5}, mean {mean:.6}");
        assert!(mean < 0.001, "mean error {mean}");
        assert!(max < 0.03, "max error {max}");
    }

    #[test]
    fn the_cache_reuses_luts_and_the_gpu_sees_every_stage() {
        let mut cache = LutCache::default();
        let frame = tools::Frame::of(64.0, 48.0);
        let make = |strength: f32, cache: &mut LutCache| {
            let tools = vec![t("warmCool", &[30.0, 0.0]), t("vignette", &[strength, 60.0, 50.0, 0.0, 0.0, 0.0, 1.0, 0.0]), t("shoulder", &[60.0, 40.0])];
            let refs: Vec<(&tools::ToolDef, &[f32])> = tools.iter().map(|(d, v)| (*d, v.as_slice())).collect();
            Chain::with_cache(&refs, frame, 1.0, 1 << 20, cache)
        };
        let (a, b) = (make(40.0, &mut cache), make(70.0, &mut cache));
        let ids = |c: &Chain| c.gpu_stages().expect("GPU").iter().filter_map(|s| if let GpuStage::Lut(l) = s { Some(l.id()) } else { None }).collect::<Vec<_>>();
        // Nur die Vignette hat sich geändert: dieselben LUTs.
        assert_eq!(ids(&a), ids(&b));
        assert_eq!(ids(&a).len(), 2);
        let stages = a.gpu_stages().unwrap();
        assert!(matches!(&stages[1], GpuStage::Op(g) if (g.values[5] - 0.4).abs() < 1e-6));
        // Mit Cache gerechnet: dieselben Pixel wie ohne.
        let fresh = {
            let tools = vec![t("warmCool", &[30.0, 0.0]), t("vignette", &[40.0, 60.0, 50.0, 0.0, 0.0, 0.0, 1.0, 0.0]), t("shoulder", &[60.0, 40.0])];
            chain(&tools, frame, 1.0, 1 << 20)
        };
        for (k, c) in colours(500).into_iter().enumerate() {
            let (x, y) = ((k % 64) as f32 + 0.5, (k / 64 % 48) as f32 + 0.5);
            assert_eq!(a.pixel(c, x, y), fresh.pixel(c, x, y));
        }
    }

    #[test]
    fn lut_stays_close_to_the_direct_result() {
        let mut all = defaults();
        all.exposure = Some(0.5);
        all.lift_gamma_gain = Some(LiftGammaGain::from_hues((200.0, 40.0), (30.0, 30.0), (60.0, 40.0)));
        for (name, s) in [("defaults", defaults()), ("everything on", all)] {
            let (max, mean) = lut_error(&s);
            println!("LUT 33³, {name}: max error {max:.5}, mean {mean:.6}");
            // Im Mittel weit unter einer 8-Bit-Stufe (1/255 ≈ 0.0039), im schlimmsten Fall
            // wenige Stufen.
            assert!(mean < 0.0005, "mean error {mean}");
            assert!(max < 0.02, "max error {max}");
        }
    }
}
