//! Grain: Filmkorn, für Fotos und Video.
//!
//! Zwei Verfahren:
//! - **Standard:** Rauschen mit Korngröße (Wertrauschen auf einem Gitter, zwei Größen
//!   überlagert), als Blendenschwankung aufgetragen. Schwarz bleibt schwarz. Gerechnet direkt auf
//!   den sRGB-Werten mit Gamma 2,2 statt über das lineare Licht: fast dasselbe, aber ohne sechs
//!   Potenzen pro Pixel.
//! - **High Definition:** das physikalische Modell nach Newson et al. 2017 („Realistic Film Grain
//!   Rendering“): Silberkörner als Scheibchen, zufällig verteilt (Poisson), so dicht, dass sie im
//!   Mittel genau den Tonwert bedecken. Jedes Pixel ist der bedeckte Anteil, gemessen an
//!   [`SAMPLES`] Stellen um die Pixelmitte. Die Körner gelten der Helligkeit; die Farbabweichung
//!   der Kanäle kommt aus dem Standardrauschen. Echter, aber deutlich langsamer.
//!
//! Beide hängen nur von Position, Kanal und `Frame::seed` ab, nie von einer Reihenfolge: Streifen
//! (PhotoCraft) setzen sich nahtlos zusammen, und die Grafikkarte rechnet bitgleich mit (der
//! Zufall ist eine Ganzzahl-Hashfunktion, die GLSL genauso kann). Die Größe gilt in Promille der
//! Bildhöhe, damit verkleinerte Vorschau und volles Bild dasselbe Korn zeigen.
//!
//! Colour mischt einfarbiges Korn (alle Kanäle gleich) mit Korn je Kanal; das Blau-Korn ist wie
//! beim Farbfilm gröber. Shadows, Mids und Highlights bestimmen, wie stark es in welchem Tonwert ist.

use std::f32::consts::PI;

use super::{Frame, Param, Step, Swatch, ToolDef};
use crate::{luma, Gpu, PositionOp};

pub const TOOL: ToolDef = ToolDef {
    id: "grain",
    name: "Grain",
    description: "Film grain for photos and video. High Definition renders single silver grains: more authentic, but much slower.",
    params: &[
        Param { key: "amount", label: "Amount", default: 35.0, ..Param::PERCENT },
        Param { key: "size", label: "Size", min: 0.2, max: 5.0, step: 0.01, default: 1.0, neutral: 1.0, unit: "", digits: 2, ..Param::PERCENT },
        Param { key: "softness", label: "Softness", default: 30.0, ..Param::PERCENT },
        Param { key: "colour", label: "Color", default: 40.0, ..Param::PERCENT },
        Param { key: "shadows", label: "Shadows", group: "Response", default: 60.0, ..Param::PERCENT },
        Param { key: "mids", label: "Mids", default: 100.0, ..Param::PERCENT },
        Param { key: "highlights", label: "Highlights", default: 40.0, ..Param::PERCENT },
        Param { key: "hd", label: "High Definition", ..Param::TOGGLE },
    ],
    swatch: Swatch::Flat,
    overlay: "",
    build,
};

/// Messstellen je Pixel und Kanal im High-Definition-Verfahren.
pub const SAMPLES: usize = 8;
/// Die Messstellen um die Pixelmitte, in Vielfachen von `sigma`: paarweise gespiegelt, also im
/// Mittel genau in der Mitte. Dieselben stehen im GLSL-Code.
const OFFSETS: [[f32; 2]; SAMPLES] = [[0.53, 0.32], [-0.53, -0.32], [0.81, -0.47], [-0.81, 0.47], [0.14, 0.96], [-0.14, -0.96], [-1.04, 0.61], [1.04, -0.61]];
/// Höchstens so viele Körner je Zelle (bei Tonwert 0,99 sind es im Mittel 5,9).
const MAX_GRAINS: u32 = 12;
/// Damit negative Zellen (am Bildrand) als vorzeichenlose Zahl gehasht werden können.
const CELL_OFFSET: i32 = 65536;
const SQRT6: f32 = 2.449_489_8;
const MONO: u32 = 23757;
/// High Definition streut bei gleichem Amount rund 3,7-mal so stark wie Standard (in PhotoCraft
/// gemessen, Grau 128): So gleich stark, der Schalter ändert die Art des Korns, nicht die Menge.
const HD_GAIN: f32 = 0.27;

fn build(v: &[f32], frame: &Frame) -> Option<Step> {
    let amount = (v[0] / 100.0).clamp(0.0, 1.0);
    let response = [v[4], v[5], v[6]].map(|r| (r / 100.0).clamp(0.0, 1.0));
    if amount == 0.0 || response == [0.0; 3] {
        return None;
    }
    let softness = (v[2] / 100.0).clamp(0.0, 1.0);
    // Gitterabstand in Pixeln, mindestens ein halbes Pixel.
    let size = (v[1].clamp(0.2, 5.0) * frame.height / 1000.0).max(0.5);
    let radius = (size * 0.5).max(0.3);
    Some(Step::Position(Box::new(Grain {
        size,
        fine: (1.0 - softness) * 0.8,
        colour: (v[3] / 100.0).clamp(0.0, 1.0),
        response,
        amount,
        hd: v[7] > 0.5,
        // 24 Bit: als f32 exakt, die Grafikkarte bekommt dieselbe Zahl.
        seed: pcg(frame.seed) >> 8,
        radius,
        sigma: (radius * (0.2 + softness)).max(0.5),
    })))
}

struct Grain {
    size: f32,
    /// Gewicht der feinen Lage im Standardverfahren.
    fine: f32,
    colour: f32,
    response: [f32; 3],
    amount: f32,
    hd: bool,
    seed: u32,
    radius: f32,
    sigma: f32,
}

fn pcg(x: u32) -> u32 {
    let s = x.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
    let w = ((s >> ((s >> 28) + 4)) ^ s).wrapping_mul(277_803_737);
    (w >> 22) ^ w
}

fn h3(a: i32, b: i32, c: u32) -> u32 {
    pcg(((a + CELL_OFFSET) as u32) ^ pcg(((b + CELL_OFFSET) as u32) ^ pcg(c)))
}

/// 0…1, in 2⁻²⁴-Schritten (in f32 exakt).
fn unit(h: u32) -> f32 {
    (h >> 8) as f32 / 16_777_216.0
}

/// Ein Gitterwert mit Mittel 0 und Streuung 1.
fn lattice(ix: i32, iy: i32, salt: u32) -> f32 {
    let h = h3(ix, iy, salt);
    (unit(h) + unit(pcg(h)) - 1.0) * SQRT6
}

/// Wertrauschen: Gitterwerte im Abstand `spacing`, weich dazwischen.
fn value(x: f32, y: f32, spacing: f32, salt: u32) -> f32 {
    let (gx, gy) = (x / spacing, y / spacing);
    let (x0, y0) = (gx.floor(), gy.floor());
    let (fx, fy) = (gx - x0, gy - y0);
    let (wx, wy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let (ix, iy) = (x0 as i32, y0 as i32);
    let (a, b, c, d) = (lattice(ix, iy, salt), lattice(ix + 1, iy, salt), lattice(ix, iy + 1, salt), lattice(ix + 1, iy + 1, salt));
    let top = a + (b - a) * wx;
    let bottom = c + (d - c) * wx;
    top + (bottom - top) * wy
}

/// Standardverfahren: zwei Lagen, die feine im Abstand 0,43 (kein gemeinsames Gitter).
fn fast(x: f32, y: f32, size: f32, fine: f32, salt: u32) -> f32 {
    (value(x, y, size, salt) + fine * value(x, y, size * 0.43, salt ^ 40503)) / (1.0 + fine * fine).sqrt()
}

/// Poisson-verteilte Zahl mit Mittel `m` (`p0` = e^−m), aus der Gleichverteilten `u`.
fn poisson(m: f32, p0: f32, u: f32) -> u32 {
    let mut p = p0;
    let mut s = p;
    let mut k = 0;
    while u > s && k < MAX_GRAINS {
        k += 1;
        p *= m / k as f32;
        s += p;
    }
    k
}

/// High Definition: der Anteil der Messstellen um (x, y), die ein Korn bedeckt, bei Tonwert `u`.
/// Zellen sind so groß wie ein Korn breit (2 r): Jede Stelle prüft höchstens 2 × 2 Zellen.
fn cover(x: f32, y: f32, u: f32, r: f32, sigma: f32, salt: u32) -> f32 {
    let u = u.clamp(0.0, 0.99);
    if u <= 0.0 {
        return 0.0;
    }
    let cell = 2.0 * r;
    // Mittlere Körnerzahl je Zelle: Boolesches Modell, Bedeckung 1 − e^(−λπr²) = u.
    let m = -(1.0 - u).ln() * 4.0 / PI;
    let p0 = (-m).exp();
    let mut hit = 0;
    for o in OFFSETS {
        let (qx, qy) = (x + o[0] * sigma, y + o[1] * sigma);
        let (x0, x1) = (((qx - r) / cell).floor() as i32, ((qx + r) / cell).floor() as i32);
        let (y0, y1) = (((qy - r) / cell).floor() as i32, ((qy + r) / cell).floor() as i32);
        let covered = (y0..=y1).any(|cy| {
            (x0..=x1).any(|cx| {
                let mut h = h3(cx, cy, salt);
                (0..poisson(m, p0, unit(h))).any(|_| {
                    h = pcg(h);
                    let gx = (cx as f32 + unit(h)) * cell;
                    h = pcg(h);
                    let gy = (cy as f32 + unit(h)) * cell;
                    (qx - gx) * (qx - gx) + (qy - gy) * (qy - gy) < r * r
                })
            })
        });
        hit += covered as u32;
    }
    hit as f32 / SAMPLES as f32
}

impl Grain {
    /// Stärke beim Tonwert `l` (0…1): Shadows bei 0, Mids bei 0,5, Highlights bei 1, linear dazwischen.
    fn strength(&self, l: f32) -> f32 {
        let [s, m, h] = self.response;
        (s * (1.0 - 2.0 * l).max(0.0) + m * (1.0 - (2.0 * l - 1.0).abs()) + h * (2.0 * l - 1.0).max(0.0)) * self.amount
    }

    /// Einfarbiges Korn und Korn je Kanal gemischt, mit gleicher Streuung.
    fn mix(&self, mono: f32, channel: f32) -> f32 {
        let c = self.colour;
        (mono * (1.0 - c) + channel * c) / ((1.0 - c) * (1.0 - c) + c * c).sqrt()
    }
}

impl PositionOp for Grain {
    fn pixel(&self, c: [f32; 3], x: f32, y: f32) -> [f32; 3] {
        let l = luma(c).clamp(0.0, 1.0);
        let k = self.strength(l);
        if k == 0.0 {
            return c;
        }
        let scale = |i: usize| if i == 2 { 1.3 } else { 1.0 };
        let salt = |i: usize| self.seed.wrapping_add(i as u32 * 7919);
        let mut out = c;
        if !self.hd {
            let mono = if self.colour < 1.0 { fast(x, y, self.size, self.fine, self.seed.wrapping_add(MONO)) } else { 0.0 };
            for (i, o) in out.iter_mut().enumerate() {
                let own = if self.colour > 0.0 { fast(x, y, self.size * scale(i), self.fine, salt(i)) } else { 0.0 };
                // Blenden im linearen Licht, auf sRGB-Werte umgerechnet (Gamma 2,2).
                *o *= (self.mix(mono, own) * (0.5 / 2.2) * k).exp2();
            }
        } else {
            // Die Körner nur einmal, für die Helligkeit: PhotoCraft erlaubt jedem Plug-in nur eine
            // bestimmte Rechenmenge pro Pixel, dreimal (je Kanal) wäre zu viel. Die Farbabweichung
            // je Kanal (wie die weicheren Farbstoffwolken im Film) kommt aus dem Standardrauschen,
            // so stark wie das Korn bei diesem Tonwert im Mittel schwankt.
            let mono = cover(x, y, l, self.radius, self.sigma, self.seed.wrapping_add(MONO)) - l.min(0.99);
            let spread = (l.min(0.99) * (1.0 - l.min(0.99)) / SAMPLES as f32).sqrt();
            for (i, o) in out.iter_mut().enumerate() {
                let own = if self.colour > 0.0 { fast(x, y, self.size * scale(i), self.fine, salt(i)) * spread } else { 0.0 };
                *o += (mono * (1.0 - self.colour) + own * self.colour) * k * HD_GAIN;
            }
        }
        out
    }

    fn gpu(&self) -> Option<Gpu> {
        Some(Gpu {
            lib: GLSL,
            code: "float l = clamp(luma(c), 0.0, 1.0);
  float k = (u[3] * max(1.0 - 2.0 * l, 0.0) + u[4] * (1.0 - abs(2.0 * l - 1.0)) + u[5] * max(2.0 * l - 1.0, 0.0)) * u[6];
  if (k == 0.0) return c;
  uint seed = uint(u[8]);
  float col = u[2];
  vec3 o = c;
  if (u[7] < 0.5) {
    float mono = col < 1.0 ? grain_fast(p, u[0], u[1], seed + 23757u) : 0.0;
    for (int i = 0; i < 3; i++) {
      float own = col > 0.0 ? grain_fast(p, u[0] * (i == 2 ? 1.3 : 1.0), u[1], seed + uint(i) * 7919u) : 0.0;
      o[i] = c[i] * exp2(grain_mix(mono, own, col) * (0.5 / 2.2) * k);
    }
  } else {
    float lc = min(l, 0.99);
    float mono = grain_cover(p, l, u[9], u[10], seed + 23757u) - lc;
    float spread = sqrt(lc * (1.0 - lc) / 8.0);
    for (int i = 0; i < 3; i++) {
      float own = col > 0.0 ? grain_fast(p, u[0] * (i == 2 ? 1.3 : 1.0), u[1], seed + uint(i) * 7919u) * spread : 0.0;
      o[i] = c[i] + (mono * (1.0 - col) + own * col) * k * 0.27;
    }
  }
  return o;",
            values: [
                self.size,
                self.fine,
                self.colour,
                self.response[0],
                self.response[1],
                self.response[2],
                self.amount,
                if self.hd { 1.0 } else { 0.0 },
                self.seed as f32,
                self.radius,
                self.sigma,
                0.0,
            ],
        })
    }
}

/// Dieselben Funktionen wie oben, für die Grafikkarte.
const GLSL: &str = "uint grain_pcg(uint x) { uint s = x * 747796405u + 2891336453u; uint w = ((s >> ((s >> 28u) + 4u)) ^ s) * 277803737u; return (w >> 22u) ^ w; }
uint grain_h3(int a, int b, uint c) { return grain_pcg(uint(a + 65536) ^ grain_pcg(uint(b + 65536) ^ grain_pcg(c))); }
float grain_unit(uint h) { return float(h >> 8u) / 16777216.0; }
float grain_lattice(int ix, int iy, uint salt) { uint h = grain_h3(ix, iy, salt); return (grain_unit(h) + grain_unit(grain_pcg(h)) - 1.0) * 2.4494898; }
float grain_value(vec2 p, float spacing, uint salt) {
  vec2 g = p / spacing, i = floor(g), f = g - i, w = f * f * (3.0 - 2.0 * f);
  int x = int(i.x), y = int(i.y);
  float a = grain_lattice(x, y, salt), b = grain_lattice(x + 1, y, salt), c = grain_lattice(x, y + 1, salt), d = grain_lattice(x + 1, y + 1, salt);
  float top = a + (b - a) * w.x, bottom = c + (d - c) * w.x;
  return top + (bottom - top) * w.y;
}
float grain_fast(vec2 p, float size, float fine, uint salt) { return (grain_value(p, size, salt) + fine * grain_value(p, size * 0.43, salt ^ 40503u)) / sqrt(1.0 + fine * fine); }
float grain_mix(float mono, float own, float c) { return (mono * (1.0 - c) + own * c) / sqrt((1.0 - c) * (1.0 - c) + c * c); }
int grain_poisson(float m, float p0, float u) { float p = p0, s = p; int k = 0; while (u > s && k < 12) { k++; p *= m / float(k); s += p; } return k; }
const vec2 GRAIN_OFFSETS[8] = vec2[8](vec2(0.53, 0.32), vec2(-0.53, -0.32), vec2(0.81, -0.47), vec2(-0.81, 0.47), vec2(0.14, 0.96), vec2(-0.14, -0.96), vec2(-1.04, 0.61), vec2(1.04, -0.61));
float grain_cover(vec2 p, float u, float r, float sigma, uint salt) {
  u = clamp(u, 0.0, 0.99);
  if (u <= 0.0) return 0.0;
  float cell = 2.0 * r, m = -log(1.0 - u) * 4.0 / 3.14159265, p0 = exp(-m);
  int hit = 0;
  for (int k = 0; k < 8; k++) {
    vec2 q = p + GRAIN_OFFSETS[k] * sigma;
    int x0 = int(floor((q.x - r) / cell)), x1 = int(floor((q.x + r) / cell)), y0 = int(floor((q.y - r) / cell)), y1 = int(floor((q.y + r) / cell));
    bool covered = false;
    for (int cy = y0; cy <= y1 && !covered; cy++) for (int cx = x0; cx <= x1 && !covered; cx++) {
      uint h = grain_h3(cx, cy, salt);
      int n = grain_poisson(m, p0, grain_unit(h));
      for (int j = 0; j < n; j++) {
        h = grain_pcg(h); float gx = (float(cx) + grain_unit(h)) * cell;
        h = grain_pcg(h); float gy = (float(cy) + grain_unit(h)) * cell;
        vec2 d = q - vec2(gx, gy);
        if (dot(d, d) < r * r) { covered = true; break; }
      }
    }
    if (covered) hit++;
  }
  return float(hit) / 8.0;
}";

#[cfg(test)]
mod tests {
    use super::*;

    fn grain(hd: bool, colour: f32, seed: u32) -> Box<dyn PositionOp> {
        match build(&[100.0, 1.0, 30.0, colour, 100.0, 100.0, 100.0, if hd { 1.0 } else { 0.0 }], &Frame { width: 400.0, height: 400.0, seed }) {
            Some(Step::Position(op)) => op,
            _ => panic!("kein Korn"),
        }
    }

    /// Mittel und Streuung des Rot-Kanals über 200 × 200 Pixel.
    fn stats(op: &dyn PositionOp, grey: f32) -> (f32, f32) {
        let values: Vec<f32> = (0..40_000).map(|i| op.pixel([grey; 3], (i % 200) as f32 + 0.5, (i / 200) as f32 + 0.5)[0]).collect();
        let mean = values.iter().sum::<f32>() / values.len() as f32;
        (mean, (values.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / values.len() as f32).sqrt())
    }

    #[test]
    fn grain_keeps_the_tone_and_adds_texture() {
        for hd in [false, true] {
            let (mean, spread) = stats(grain(hd, 40.0, 0).as_ref(), 0.5);
            assert!((mean - 0.5).abs() < 0.02, "hd {hd}: Mittel {mean}");
            assert!(spread > 0.02, "hd {hd}: Streuung {spread}");
        }
    }

    #[test]
    fn the_boolean_model_covers_the_tone() {
        for u in [0.1, 0.5, 0.9] {
            let n = 20_000;
            let mean = (0..n).map(|i| cover((i % 200) as f32 * 3.7, (i / 200) as f32 * 3.7, u, 1.2, 0.6, 7)).sum::<f32>() / n as f32;
            assert!((mean - u).abs() < 0.02, "Tonwert {u}: bedeckt {mean}");
        }
    }

    #[test]
    fn same_place_same_grain_new_frame_new_grain() {
        let (a, b, c) = (grain(false, 40.0, 0), grain(false, 40.0, 0), grain(false, 40.0, 1));
        let px = [0.4, 0.5, 0.6];
        assert_eq!(a.pixel(px, 10.5, 20.5), b.pixel(px, 10.5, 20.5));
        assert_ne!(a.pixel(px, 10.5, 20.5), c.pixel(px, 10.5, 20.5));
        // Einfarbig: alle Kanäle schwanken gleich.
        let mono = grain(false, 0.0, 0).pixel([0.5; 3], 33.5, 7.5);
        assert!(mono[0] == mono[1] && mono[1] == mono[2]);
    }

    #[test]
    fn black_stays_black_in_the_standard_mode() {
        assert_eq!(grain(false, 40.0, 0).pixel([0.0; 3], 3.5, 3.5), [0.0; 3]);
    }
}
