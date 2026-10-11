//! Look-Kette (Prototyp) als PhotoCraft-Filter-Plug-in (Plug-in-ABI v1, siehe
//! `docs/plugins.md` im PhotoCraft-Repository).
//!
//! Dieser Adapter übersetzt nur: das Manifest mit den Parametern, das Parameter-JSON des Hosts
//! in die Einstellungen des Kerns, und die verschachtelten `f32`-Samples (RGB oder RGBA mit
//! nicht vormultipliziertem Alpha) in die Farben, auf denen der Kern rechnet.
//!
//! Was PhotoCrafts erzeugter Dialog vorgibt:
//! * Er beschriftet die Regler aus den Parameternamen (`warmCoolTemperatur` → „Warm Cool
//!   Temperatur"), deshalb gibt es hier keine Umlaute.
//! * Er kennt nur Zahl (Schieberegler, zwei Zeilen), Ganzzahl (Zahlenfeld, eine Zeile), Schalter
//!   und Auswahl, und er scrollt nicht: Alles muss untereinander ins Fenster passen. Deshalb
//!   sind nur vier Parameter Schieberegler, Lift/Gain kommt als Farbton + Betrag ohne Gamma, und
//!   die Vignette sitzt fest in der Mitte der Arbeitsfläche.
//!
//! Installieren: Filter ▸ Plug-ins ▸ Install Plug-in…, oder die Datei in den Ordner legen, der
//! unter Preferences ▸ Plug-ins ▸ Additional Plug-ins Folder eingetragen ist.
//!
//! Fehlercodes von `pc_filter`: 1 = Aufruf passt nicht zusammen (Puffergröße, Kanäle),
//! 2 = das Ziel ist kein RGB-Bild (Graustufen, CMYK, Lab, Alphakanal, Maskierungsmodus).

use look_kette_core::{ColorSettings, Grade, LiftGammaGain, Look, Lut, Shoulder, TealOrange, Vignette, WarmCool};

pub use look_kette_core::json;

/// Das Manifest. Die Reihenfolge der Parameter ist die Reihenfolge im Dialog.
pub const MANIFEST: &str = r#"{
  "id": "de.gronenberg.look-kette",
  "name": "Look-Kette (Prototyp)",
  "version": "0.2.0",
  "kind": "filter",
  "author": "Marc Gronenberg",
  "description": "Feste Kette aus sechs Grading-Tools, jedes mit eigenem Schalter.",
  "params": {
    "warmCool": {"type": "bool", "default": true},
    "warmCoolTemperatur": {"type": "number", "min": -100, "max": 100, "default": 25},
    "warmCoolFarbstich": {"type": "int", "min": -100, "max": 100, "default": 0},

    "belichtung": {"type": "bool", "default": false},
    "belichtungBlenden": {"type": "number", "min": -4, "max": 4, "default": 0.5},

    "schulter": {"type": "bool", "default": true},
    "schulterEinsatzpunkt": {"type": "int", "min": 0, "max": 100, "default": 70},
    "schulterBetrag": {"type": "int", "min": 0, "max": 100, "default": 30},

    "tealOrange": {"type": "bool", "default": true},
    "tealOrangeBetrag": {"type": "number", "min": 0, "max": 100, "default": 55},
    "tealOrangeHautschutz": {"type": "int", "min": 0, "max": 100, "default": 60},

    "liftGain": {"type": "bool", "default": false},
    "liftFarbton": {"type": "int", "min": 0, "max": 360, "default": 200},
    "liftBetrag": {"type": "int", "min": 0, "max": 100, "default": 20},
    "gainFarbton": {"type": "int", "min": 0, "max": 360, "default": 45},
    "gainBetrag": {"type": "int", "min": 0, "max": 100, "default": 20},

    "vignette": {"type": "bool", "default": true},
    "vignetteRadius": {"type": "int", "min": 10, "max": 200, "default": 115},
    "vignetteBetrag": {"type": "int", "min": 0, "max": 100, "default": 35},
    "vignetteWeichheit": {"type": "int", "min": 0, "max": 100, "default": 50},

    "deckkraft": {"type": "number", "min": 0, "max": 100, "default": 100}
  },
  "overlap": 0,
  "area": "content"
}"#;

/// Bit 8 des `format`-Arguments: der letzte Kanal ist Alpha.
const HAS_ALPHA: u32 = 1 << 8;
/// Farbmodi des `format`-Arguments (Bits 16–23), nummeriert wie im PSD-Format.
const MODE_INDEXED: u32 = 2;
const MODE_RGB: u32 = 3;

/// Was der Host pro Aufruf mitgibt: die Einstellungen und die Lage des Puffers im Dokument.
#[derive(Debug, PartialEq)]
pub struct Request {
    pub colour: ColorSettings,
    /// Radius, Betrag, Weichheit. `None` = aus.
    pub vignette: Option<[f32; 3]>,
    /// Gesamtstärke 0…1.
    pub mix: f32,
    /// Dokumentposition des ersten Samples im Puffer.
    pub origin: [f32; 2],
    /// Größe der Arbeitsfläche; fehlt sie, gilt die Puffergröße.
    pub canvas: Option<[f32; 2]>,
}

impl Request {
    /// Liest das Parameter-JSON des Hosts. Fehlende Werte nehmen die Vorgaben des Manifests (der
    /// Host füllt sie ohnehin auf, so bleibt der Filter aber auch ohne ihn benutzbar).
    pub fn parse(text: &str) -> Request {
        let top = json::members(text);
        let num = |key: &str, default: f32| json::number(&top, key).map_or(default, |v| v as f32);
        let on = |key: &str, default: bool| json::boolean(&top, key).unwrap_or(default);
        let image = json::object(&top, "_image");
        let inum = |key: &str| image.as_ref().and_then(|m| json::number(m, key)).map(|v| v as f32);

        Request {
            colour: ColorSettings {
                warm_cool: on("warmCool", true).then(|| WarmCool { temperature: num("warmCoolTemperatur", 25.0), tint: num("warmCoolFarbstich", 0.0) }),
                exposure: on("belichtung", false).then(|| num("belichtungBlenden", 0.5)),
                shoulder: on("schulter", true).then(|| Shoulder { start: num("schulterEinsatzpunkt", 70.0), amount: num("schulterBetrag", 30.0) }),
                teal_orange: on("tealOrange", true).then(|| TealOrange { amount: num("tealOrangeBetrag", 55.0), skin: num("tealOrangeHautschutz", 60.0) }),
                // Gamma bleibt neutral: Der Dialog hat dafür keinen Platz.
                lift_gamma_gain: on("liftGain", false)
                    .then(|| LiftGammaGain::from_hues((num("liftFarbton", 200.0), num("liftBetrag", 20.0)), (0.0, 0.0), (num("gainFarbton", 45.0), num("gainBetrag", 20.0)))),
            },
            vignette: on("vignette", true).then(|| [num("vignetteRadius", 115.0), num("vignetteBetrag", 35.0), num("vignetteWeichheit", 50.0)]),
            mix: (num("deckkraft", 100.0) / 100.0).clamp(0.0, 1.0),
            origin: [inum("x").unwrap_or(0.0), inum("y").unwrap_or(0.0)],
            canvas: match (inum("canvasWidth"), inum("canvasHeight")) {
                (Some(w), Some(h)) if w > 0.0 && h > 0.0 => Some([w, h]),
                _ => None,
            },
        }
    }
}

/// Der Filter auf einem Puffer aus `width × height × channels` verschachtelten Samples.
/// Rückgabe wie `pc_filter`: 0 = in Ordnung.
pub fn filter(samples: &mut [f32], width: usize, height: usize, channels: usize, format: u32, params: &str) -> i32 {
    let alpha = format & HAS_ALPHA != 0;
    let mode = (format >> 16) & 0xff;
    if width == 0 || height == 0 || samples.len() != width * height * channels {
        return 1;
    }
    if !(mode == MODE_RGB || mode == MODE_INDEXED) || channels != if alpha { 4 } else { 3 } {
        return 2;
    }
    let req = Request::parse(params);
    let grade = Grade::new(&req.colour);
    let canvas = req.canvas.unwrap_or([width as f32, height as f32]);
    // Die Vignette sitzt in der Mitte der Arbeitsfläche.
    let vignette = req.vignette.and_then(|[radius, amount, softness]| {
        let half_diagonal = 0.5 * (canvas[0] * canvas[0] + canvas[1] * canvas[1]).sqrt();
        Vignette::new([canvas[0] / 2.0, canvas[1] / 2.0], half_diagonal, radius, amount, softness)
    });
    if Look::new(&grade, None, vignette, req.mix).is_identity() {
        return 0;
    }
    // Jeder Aufruf läuft in einer frischen Instanz, die LUT wird also für jeden Streifen neu
    // gerechnet; bei kleinen Puffern rechnet der Filter deshalb direkt.
    let lut = (!grade.is_identity() && Lut::worthwhile(width * height)).then(|| Lut::build(&grade));
    let look = Look::new(&grade, lut.as_ref(), vignette, req.mix);
    for (row, line) in samples.chunks_exact_mut(width * channels).enumerate() {
        // Positionen im Dokument, damit die Vignette über die Streifen hinweg nahtlos ist.
        let dy2 = look.row(req.origin[1] + row as f32 + 0.5);
        for (col, c) in line.chunks_exact_mut(channels).enumerate() {
            // Ganz durchsichtige Pixel haben keine Farbe, die man graden könnte.
            if alpha && c[3] <= 0.0 {
                continue;
            }
            let out = look.pixel([c[0], c[1], c[2]], req.origin[0] + col as f32 + 0.5, dy2);
            c[..3].copy_from_slice(&out);
        }
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn pc_abi_version() -> u32 {
    1
}

/// `len << 32 | ptr` des UTF-8-Manifests.
#[unsafe(no_mangle)]
pub extern "C" fn pc_manifest() -> u64 {
    ((MANIFEST.len() as u64) << 32) | (MANIFEST.as_ptr() as usize as u64 & 0xffff_ffff)
}

/// Ein Block von `size` Bytes, 8-Byte-ausgerichtet, oder 0. Er wird nie freigegeben: Jeder
/// Aufruf bekommt eine frische Instanz.
#[unsafe(no_mangle)]
pub extern "C" fn pc_alloc(size: u32) -> *mut u8 {
    let Ok(layout) = std::alloc::Layout::from_size_align((size as usize).max(8), 8) else { return std::ptr::null_mut() };
    // SAFETY: Die Größe ist nicht null.
    unsafe { std::alloc::alloc_zeroed(layout) }
}

/// # Safety
/// Der Host übergibt Blöcke, die er von `pc_alloc` bekommen hat: `buf_len` Bytes Samples und
/// `params_len` Bytes UTF-8-JSON.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pc_filter(buf: *mut f32, buf_len: u32, width: u32, height: u32, channels: u32, format: u32, params: *const u8, params_len: u32) -> i32 {
    if buf.is_null() || (buf as usize) % 4 != 0 {
        return 1;
    }
    // SAFETY: siehe oben.
    let samples = unsafe { std::slice::from_raw_parts_mut(buf, buf_len as usize / 4) };
    let text = if params.is_null() || params_len == 0 {
        ""
    } else {
        // SAFETY: siehe oben.
        std::str::from_utf8(unsafe { std::slice::from_raw_parts(params, params_len as usize) }).unwrap_or("")
    };
    filter(samples, width as usize, height as usize, channels as usize, format, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RGBA8: u32 = 8 | HAS_ALPHA | MODE_RGB << 16;

    #[test]
    fn the_manifest_is_valid_json_with_all_parameters() {
        let top = json::members(MANIFEST);
        assert_eq!(json::string(&top, "id"), Some("de.gronenberg.look-kette"));
        assert_eq!(json::string(&top, "kind"), Some("filter"));
        let params = json::object(&top, "params").expect("params");
        assert_eq!(params.len(), 21);
        for (key, spec) in &params {
            assert!(key.bytes().all(|b| b.is_ascii_alphanumeric()), "{key}");
            assert!(json::string(&json::members(spec), "type").is_some(), "{key}");
        }
    }

    #[test]
    fn parse_reads_host_params_and_falls_back_to_defaults() {
        let r = Request::parse(
            r#"{"warmCool":false,"belichtung":true,"belichtungBlenden":-1.5,"vignetteRadius":80,"liftGain":true,"liftBetrag":40,"deckkraft":50.0,
                "_image":{"x":1024,"y":256,"width":508,"height":256,"overlap":0,"canvasWidth":1532,"canvasHeight":832,"mode":"rgb","depth":8,"alpha":true,"channels":4}}"#,
        );
        assert_eq!(r.colour.warm_cool, None);
        assert_eq!(r.colour.exposure, Some(-1.5));
        assert_eq!(r.colour.shoulder, Some(Shoulder { start: 70.0, amount: 30.0 }));
        assert_eq!(r.vignette, Some([80.0, 35.0, 50.0]));
        let lgg = r.colour.lift_gamma_gain.expect("lift/gain on");
        assert_eq!(lgg.gamma, [0.5; 3]);
        assert!(lgg.lift[2] > 0.5 && lgg.lift[0] < 0.5, "lift towards blue-green: {:?}", lgg.lift);
        assert!(lgg.gain[0] > 0.5 && lgg.gain[2] < 0.5, "gain towards orange: {:?}", lgg.gain);
        assert_eq!(r.mix, 0.5);
        assert_eq!(r.origin, [1024.0, 256.0]);
        assert_eq!(r.canvas, Some([1532.0, 832.0]));
        // Ohne JSON: die Vorgaben des Manifests.
        let d = Request::parse("");
        assert_eq!(d.colour.warm_cool, Some(WarmCool { temperature: 25.0, tint: 0.0 }));
        assert_eq!(d.mix, 1.0);
        assert_eq!(d.canvas, None);
    }

    #[test]
    fn rejects_what_is_not_rgb() {
        let mut gray = vec![0.5f32; 16];
        assert_eq!(filter(&mut gray, 4, 4, 1, 8 | 1 << 16, "{}"), 2);
        let mut cmyk = vec![0.5f32; 64];
        assert_eq!(filter(&mut cmyk, 4, 4, 4, 8 | 4 << 16, "{}"), 2);
        let mut short = vec![0.5f32; 10];
        assert_eq!(filter(&mut short, 4, 4, 4, RGBA8, "{}"), 1);
        assert!(gray.iter().chain(&cmyk).all(|v| *v == 0.5));
    }

    #[test]
    fn keeps_alpha_and_skips_transparent_pixels() {
        let mut px = vec![0.3, 0.5, 0.7, 1.0, 0.3, 0.5, 0.7, 0.25, 0.3, 0.5, 0.7, 0.0, 0.9, 0.9, 0.9, 1.0];
        assert_eq!(filter(&mut px, 2, 2, 4, RGBA8, "{}"), 0);
        assert_eq!([px[3], px[7], px[11], px[15]], [1.0, 0.25, 0.0, 1.0]);
        assert_eq!(&px[8..11], &[0.3, 0.5, 0.7], "transparent pixel untouched");
        assert_ne!(&px[0..3], &[0.3, 0.5, 0.7], "opaque pixel graded");
        // Alpha ist nicht vormultipliziert: gleiche Farbe, gleiches Ergebnis, bis auf die Lage
        // in der Vignette (hier aus).
        let mut a = vec![0.3, 0.5, 0.7, 1.0, 0.3, 0.5, 0.7, 0.25];
        assert_eq!(filter(&mut a, 2, 1, 4, RGBA8, r#"{"vignette":false}"#), 0);
        assert_eq!(a[0..3], a[4..7]);
    }

    #[test]
    fn everything_off_changes_nothing() {
        let off = r#"{"warmCool":false,"belichtung":false,"schulter":false,"tealOrange":false,"liftGain":false,"vignette":false}"#;
        let orig: Vec<f32> = (0..300).map(|i| (i % 97) as f32 / 96.0).collect();
        let mut px = orig.clone();
        assert_eq!(filter(&mut px, 10, 10, 3, 8 | MODE_RGB << 16, off), 0);
        assert_eq!(px, orig);
        let mut px = orig.clone();
        assert_eq!(filter(&mut px, 10, 10, 3, 8 | MODE_RGB << 16, r#"{"deckkraft":0}"#), 0);
        assert_eq!(px, orig);
    }

    #[test]
    fn the_vignette_is_seamless_across_bands() {
        // Ein 8 × 4 großes Bild in einem Stück und in zwei Streifen: gleiches Ergebnis.
        let base = r#""warmCool":false,"schulter":false,"tealOrange":false,"vignetteRadius":60,"vignetteBetrag":80"#;
        let image = |x: usize, y: usize, w: usize, h: usize| format!(r#"{{{base},"_image":{{"x":{x},"y":{y},"width":{w},"height":{h},"canvasWidth":8,"canvasHeight":4}}}}"#);
        let mut whole = vec![0.5f32; 8 * 4 * 3];
        assert_eq!(filter(&mut whole, 8, 4, 3, 8 | MODE_RGB << 16, &image(0, 0, 8, 4)), 0);
        let mut top = vec![0.5f32; 8 * 2 * 3];
        let mut bottom = vec![0.5f32; 8 * 2 * 3];
        assert_eq!(filter(&mut top, 8, 2, 3, 8 | MODE_RGB << 16, &image(0, 0, 8, 2)), 0);
        assert_eq!(filter(&mut bottom, 8, 2, 3, 8 | MODE_RGB << 16, &image(0, 2, 8, 2)), 0);
        top.extend(bottom);
        assert_eq!(whole, top);
        assert!(whole[0] < 0.5 && whole[0] > 0.05, "corner darkened: {}", whole[0]);
    }

}
