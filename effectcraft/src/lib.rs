//! Look-Kette (Prototyp) als EffectCraft-Effekt-Plug-in (Plug-in-API v1, siehe
//! `docs/plugins.md` im EffectCraft-Repository).
//!
//! Dieser Adapter übersetzt nur: das Manifest mit den Parametern, die flache Parameterliste des
//! Hosts in die Einstellungen des Kerns, und vormultipliziertes RGBA in die nicht
//! vormultiplizierten Farben, auf denen der Kern rechnet.
//!
//! Laden: Effect ▸ Load Effect Plug-in… oder die Datei in den `Plug-ins`-Ordner neben den
//! Einstellungen legen (wird beim Start geladen).

use std::cell::RefCell;

use look_kette_core::{ColorSettings, Grade, LiftGammaGain, Look, Lut, Shoulder, TealOrange, Vignette, WarmCool};

/// Das Manifest. Die Parameter kommen in genau dieser Reihenfolge als flache `f64`-Liste an
/// (Checkbox 1 Wert, Slider 1, Farbe 4, Punkt 2), siehe die Indizes in [`P`].
const MANIFEST: &str = r#"{
  "api": 1,
  "id": "de.gronenberg.look-kette",
  "name": "Look-Kette (Prototyp)",
  "category": "Color Correction",
  "version": "0.2.0",
  "author": "Marc Gronenberg",
  "description": "Feste Kette aus sechs Grading-Tools, jedes mit eigenem Schalter.",
  "params": [
    {"id": "wc_on", "name": "WARM/COOL", "type": "checkbox", "default": true},
    {"id": "wc_temp", "name": "· Temperatur", "type": "slider", "default": 25, "min": -100, "max": 100, "decimals": 1},
    {"id": "wc_tint", "name": "· Tönung", "type": "slider", "default": 0, "min": -100, "max": 100, "decimals": 1},

    {"id": "ex_on", "name": "BELICHTUNG", "type": "checkbox", "default": false},
    {"id": "ex_stops", "name": "· Blenden", "type": "slider", "default": 0.5, "min": -4, "max": 4, "decimals": 2},

    {"id": "sh_on", "name": "SCHULTER", "type": "checkbox", "default": true},
    {"id": "sh_start", "name": "· Einsatzpunkt", "type": "slider", "default": 70, "min": 0, "max": 100, "decimals": 1},
    {"id": "sh_amount", "name": "· Stärke", "type": "slider", "default": 30, "min": 0, "max": 100, "decimals": 1},

    {"id": "to_on", "name": "TEAL-ORANGE", "type": "checkbox", "default": true},
    {"id": "to_amount", "name": "· Stärke", "type": "slider", "default": 55, "min": 0, "max": 100, "decimals": 1},
    {"id": "to_skin", "name": "· Hautschutz", "type": "slider", "default": 60, "min": 0, "max": 100, "decimals": 1},

    {"id": "lgg_on", "name": "LIFT/GAMMA/GAIN", "type": "checkbox", "default": false},
    {"id": "lgg_lift", "name": "· Lift (Grau = neutral)", "type": "color", "default": [0.5, 0.5, 0.5, 1.0]},
    {"id": "lgg_gamma", "name": "· Gamma (Grau = neutral)", "type": "color", "default": [0.5, 0.5, 0.5, 1.0]},
    {"id": "lgg_gain", "name": "· Gain (Grau = neutral)", "type": "color", "default": [0.5, 0.5, 0.5, 1.0]},

    {"id": "vg_on", "name": "VIGNETTE", "type": "checkbox", "default": true},
    {"id": "vg_center", "name": "· Mitte", "type": "point", "default": [0.5, 0.5]},
    {"id": "vg_size", "name": "· Größe", "type": "slider", "default": 115, "min": 10, "max": 200, "decimals": 1},
    {"id": "vg_amount", "name": "· Stärke", "type": "slider", "default": 35, "min": 0, "max": 100, "decimals": 1},
    {"id": "vg_soft", "name": "· Weichheit", "type": "slider", "default": 50, "min": 0, "max": 100, "decimals": 1},

    {"id": "mix", "name": "GESAMTSTÄRKE", "type": "slider", "default": 100, "min": 0, "max": 100, "decimals": 1}
  ]
}"#;

/// Indizes in der flachen Parameterliste.
#[allow(non_snake_case)]
mod P {
    pub const WC_ON: usize = 0;
    pub const WC_TEMP: usize = 1;
    pub const WC_TINT: usize = 2;
    pub const EX_ON: usize = 3;
    pub const EX_STOPS: usize = 4;
    pub const SH_ON: usize = 5;
    pub const SH_START: usize = 6;
    pub const SH_AMOUNT: usize = 7;
    pub const TO_ON: usize = 8;
    pub const TO_AMOUNT: usize = 9;
    pub const TO_SKIN: usize = 10;
    pub const LGG_ON: usize = 11;
    pub const LGG_LIFT: usize = 12;
    pub const LGG_GAMMA: usize = 16;
    pub const LGG_GAIN: usize = 20;
    /// Bis hier (exklusiv) reichen die reinen Farb-Parameter: der Schlüssel der LUT.
    pub const COLOR_END: usize = 24;
    pub const VG_ON: usize = 24;
    pub const VG_CENTER: usize = 25;
    pub const VG_SIZE: usize = 27;
    pub const VG_AMOUNT: usize = 28;
    pub const VG_SOFT: usize = 29;
    pub const MIX: usize = 30;
    pub const COUNT: usize = 31;
}


/// Die flache Parameterliste des Hosts als Einstellungen des Kerns.
fn colour_settings(p: &[f64]) -> ColorSettings {
    let on = |i: usize| p[i] != 0.0;
    let f = |i: usize| p[i] as f32;
    let rgb = |i: usize| [f(i), f(i + 1), f(i + 2)];
    ColorSettings {
        warm_cool: on(P::WC_ON).then(|| WarmCool { temperature: f(P::WC_TEMP), tint: f(P::WC_TINT) }),
        exposure: on(P::EX_ON).then(|| f(P::EX_STOPS)),
        shoulder: on(P::SH_ON).then(|| Shoulder { start: f(P::SH_START), amount: f(P::SH_AMOUNT) }),
        teal_orange: on(P::TO_ON).then(|| TealOrange { amount: f(P::TO_AMOUNT), skin: f(P::TO_SKIN) }),
        lift_gamma_gain: on(P::LGG_ON).then(|| LiftGammaGain { lift: rgb(P::LGG_LIFT), gamma: rgb(P::LGG_GAMMA), gain: rgb(P::LGG_GAIN) }),
    }
}

thread_local! {
    /// Ein wiederverwendeter, 8-Byte-ausgerichteter Puffer für Parameter und Pixel.
    static BUF: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
    /// Die zuletzt gerechnete LUT mit ihren Parametern (der Host hält Instanzen zwischen den
    /// Frames vor). Sie hängt nur von den Parametern ab, das Ergebnis bleibt also deterministisch.
    static LUT: RefCell<Option<(Vec<f64>, Lut)>> = const { RefCell::new(None) };
}

#[unsafe(no_mangle)]
pub extern "C" fn ec_api_version() -> i32 {
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn ec_manifest_ptr() -> i32 {
    MANIFEST.as_ptr() as i32
}

#[unsafe(no_mangle)]
pub extern "C" fn ec_manifest_len() -> i32 {
    MANIFEST.len() as i32
}

#[unsafe(no_mangle)]
pub extern "C" fn ec_alloc(bytes: i32) -> i32 {
    BUF.with(|b| {
        let mut b = b.borrow_mut();
        let words = (bytes.max(0) as usize).div_ceil(8);
        if b.len() < words {
            b.resize(words, 0);
        }
        b.as_mut_ptr() as i32
    })
}

/// # Safety
/// Der Host übergibt Zeiger in den Puffer, den `ec_alloc` geliefert hat.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ec_render(pixels: i32, width: i32, height: i32, params: i32, nparams: i32, _time: f64, _scale: f64) -> i32 {
    if (nparams as usize) < P::COUNT || width <= 0 || height <= 0 {
        return 1;
    }
    let (w, h) = (width as usize, height as usize);
    let p = unsafe { std::slice::from_raw_parts(params as *const f64, nparams as usize) };
    let px = unsafe { std::slice::from_raw_parts_mut(pixels as *mut [f32; 4], w * h) };

    let grade = Grade::new(&colour_settings(p));
    // Vignette um den Mittelpunkt (der Host liefert ihn schon in Pufferpixeln).
    let vignette = (p[P::VG_ON] != 0.0)
        .then(|| {
            let half_diagonal = 0.5 * ((w * w + h * h) as f32).sqrt();
            Vignette::new([p[P::VG_CENTER] as f32, p[P::VG_CENTER + 1] as f32], half_diagonal, p[P::VG_SIZE] as f32, p[P::VG_AMOUNT] as f32, p[P::VG_SOFT] as f32)
        })
        .flatten();
    let mix = (p[P::MIX] as f32 / 100.0).clamp(0.0, 1.0);

    LUT.with(|slot| {
        let mut slot = slot.borrow_mut();
        // Kleine Puffer werden direkt gerechnet: Dort lohnt das Vorrechnen nicht.
        let use_lut = mix > 0.0 && !grade.is_identity() && Lut::worthwhile(px.len());
        let key = &p[..P::COLOR_END];
        if use_lut && slot.as_ref().is_none_or(|(k, _)| k != key) {
            *slot = Some((key.to_vec(), Lut::build(&grade)));
        }
        let look = Look::new(&grade, if use_lut { slot.as_ref().map(|(_, l)| l) } else { None }, vignette, mix);
        if look.is_identity() {
            return;
        }
        for (row, line) in px.chunks_exact_mut(w).enumerate() {
            let dy2 = look.row(row as f32 + 0.5);
            for (col, c) in line.iter_mut().enumerate() {
                let a = c[3];
                if a <= 0.0 {
                    continue;
                }
                // Vormultiplikation aufheben, graden, wieder vormultiplizieren.
                let src = if a == 1.0 { [c[0], c[1], c[2]] } else { [c[0] / a, c[1] / a, c[2] / a] };
                let out = look.pixel(src, col as f32 + 0.5, dy2);
                c[0] = out[0] * a;
                c[1] = out[1] * a;
                c[2] = out[2] * a;
            }
        }
    });
    0
}
