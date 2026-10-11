//! Looksmaxxer: die Look-Kette mit frei sortierten Tools.
//!
//! Ein WebAssembly-Modul für zwei Zwecke:
//! * als PhotoCraft-Filter-Plug-in (Plug-in-ABI v1, `pc_*`): Das Looksmaxxer-Fenster installiert
//!   es in das laufende PhotoCraft und führt es bei „Apply" auf der aktiven Ebene aus;
//! * als Vorschau-Filter im Fenster (`lk_*`): Die Oberfläche rechnet damit die Vorschau, mit genau
//!   derselben Funktion ([`filter`]). Die Vorschau zeigt also, was „Apply" ergibt, bis auf die
//!   Rundung auf 8 Bit und die Größe der Vorschau.
//!
//! PhotoCrafts Plug-in-Parameter kennen nur Zahl, Ganzzahl, Schalter und Auswahl, keinen Text, und
//! höchstens [`PHOTOCRAFT_MAX_PARAMS`] davon. Die Kette kommt deshalb in [`SLOTS`] festen Plätzen an:
//! `slot1` … `slot15` nennen das Tool (oder `none`), `slot1A` … `slot1C` tragen seine Regler,
//! gepackt: Jeder Regler wird auf seine Schrittweite gerundet und als 16-Bit-Stufe gespeichert
//! ([`level`]), drei Stufen in einer Zahl (`A` = Regler 1–3, `B` = 4–6, `C` = 7–9; die erste Stufe in
//! den unteren 16 Bit). Gespeichert wird die Stufe + 1; 0 heißt „nicht gesetzt“ und nimmt die
//! Vorgabe des Reglers (so ergibt PhotoCrafts eigener Dialog mit seinen Nullen vernünftige Werte).
//! 48 Bit hält eine Kommazahl exakt. Gepackt wird nur hier ([`pack`], `lk_pack`): Die Oberfläche
//! schickt die Reglerwerte im Klartext und bekommt die gepackten Parameter zurück. Welche Tools
//! es gibt und welche Regler sie haben, steht allein in `look_kette_core::tools`; Manifest
//! ([`manifest`]) und Tool-Liste der Oberfläche ([`tools_json`], `lk_tools`) entstehen daraus.
//! Ausgeschaltete Tools lässt das Fenster einfach weg.
//!
//! PhotoCrafts erzeugter Dialog für diese Parameter (Filter ▸ Plug-ins ▸ Looksmaxxer) ist nur
//! eine Notlösung; gedacht ist das Plug-in für das Fenster.
//!
//! Fehlercodes von `pc_filter` und `lk_render`: 1 = Aufruf passt nicht zusammen (Puffergröße,
//! Kanäle), 2 = das Ziel ist kein RGB-Bild (Graustufen, CMYK, Lab, Alphakanal, Maskierungsmodus).

use std::cell::RefCell;
use std::fmt::Write;
use std::sync::OnceLock;

use look_kette_core::{json, tools, Chain, GpuStage, LutCache};

/// So viele Parameter erlaubt PhotoCraft (0.7) einem Plug-in.
pub const PHOTOCRAFT_MAX_PARAMS: usize = 64;
/// So viele Regler stecken in einer Zahl (je 16 Bit).
pub const PER_NUMBER: usize = 3;
/// So viele Zahlen hat ein Platz.
pub const NUMBERS: usize = tools::MAX_PARAMS.div_ceil(PER_NUMBER);
/// So viele Tools fasst die Kette: je Platz das Tool und [`NUMBERS`] Zahlen, dazu die Gesamtstärke.
pub const SLOTS: usize = (PHOTOCRAFT_MAX_PARAMS - 1) / (1 + NUMBERS);
/// Die größte gepackte Zahl, 2⁴⁸ − 1.
const PACKED_MAX: f64 = 281_474_976_710_655.0;

/// Die Zahlen eines Platzes heißen `slot1A`, `slot1B` …
fn letter(k: usize) -> char {
    char::from(b'A' + k as u8)
}

/// Minimum, Maximum und Schrittweite als Dezimalzahlen, so wie die Oberfläche sie liest: 0,1 ist
/// als 32-Bit-Zahl nicht exakt, und sonst würde aus der Stufe für 0 nicht genau 0.
fn grid(p: &tools::Param) -> (f64, f64, f64) {
    let decimal = |v: f32| (v as f64 * 1e6).round() / 1e6;
    (decimal(p.min), decimal(p.max), decimal(p.step))
}

/// Die Stufe eines Reglerwerts: Schritte ab dem Minimum, gerundet und im Bereich.
pub fn level(p: &tools::Param, v: f32) -> u64 {
    let (min, max, step) = grid(p);
    ((v as f64 - min) / step).round().clamp(0.0, ((max - min) / step).round()) as u64
}

/// Der Reglerwert einer Stufe.
pub fn value(p: &tools::Param, level: u64) -> f32 {
    let (min, max, step) = grid(p);
    (min + level as f64 * step).min(max) as f32
}

/// Packt Parameter im Klartext (`"slot1":"vignette","slot1_0":-37,"slot1_1":140,…,"strength":80`,
/// Regler in ihrer Reihenfolge; fehlende nehmen die Vorgabe) in die gepackte Form für [`filter`] und
/// PhotoCraft. Unbekannte Tools werden zu `none`.
pub fn pack(plain: &str) -> String {
    let top = json::members(plain);
    let mut out = vec![format!(r#""strength":{}"#, json::number(&top, "strength").unwrap_or(100.0))];
    for i in 1..=SLOTS {
        let Some(tool) = json::string(&top, &format!("slot{i}")).and_then(tools::find) else {
            out.push(format!(r#""slot{i}":"none""#));
            continue;
        };
        let mut numbers = [0u64; NUMBERS];
        for (k, p) in tool.params.iter().enumerate() {
            let v = json::number(&top, &format!("slot{i}_{k}")).map_or(p.default, |v| v as f32);
            numbers[k / PER_NUMBER] |= (level(p, v) + 1) << (16 * (k % PER_NUMBER));
        }
        out.push(format!(r#""slot{i}":"{}""#, tool.id));
        out.extend(numbers.iter().enumerate().map(|(n, v)| format!(r#""slot{i}{}":{v}"#, letter(n))));
    }
    format!("{{{}}}", out.join(","))
}

/// Die Ellipse der Vignette für den Griff auf dem Bild, aus `width`, `height` und den Reglern der
/// Vignette (Schlüssel wie in `tools/vignette.rs`): `{"cx","cy","rx","ry","inner"}` in Pixeln.
pub fn shape(text: &str) -> String {
    let top = json::members(text);
    let num = |k: &str, d: f32| json::number(&top, k).map_or(d, |v| v as f32);
    let frame = tools::Frame::of(num("width", 1.0), num("height", 1.0));
    let s = tools::vignette::Shape::new(&frame, num("size", 65.0), num("softness", 50.0), num("centerX", 0.0), num("centerY", 0.0), num("aspect", 1.0), num("anamorphic", 0.0) > 0.5);
    format!(r#"{{"cx":{},"cy":{},"rx":{},"ry":{},"inner":{}}}"#, s.cx, s.cy, s.rx, s.ry, s.inner)
}

/// Das Manifest für PhotoCraft, aus [`tools::ALL`] erzeugt.
pub fn manifest() -> &'static str {
    static MANIFEST: OnceLock<String> = OnceLock::new();
    MANIFEST.get_or_init(|| {
        let options: Vec<String> = std::iter::once("none").chain(tools::ALL.iter().map(|t| t.id)).map(|id| format!("\"{id}\"")).collect();
        let mut params = String::new();
        for i in 1..=SLOTS {
            let _ = writeln!(params, r#"    "slot{i}": {{"type": "choice", "options": [{}], "default": "none"}},"#, options.join(", "));
            for k in 0..NUMBERS {
                let _ = writeln!(params, r#"    "slot{i}{}": {{"type": "number", "min": 0, "max": {PACKED_MAX}, "default": 0}},"#, letter(k));
            }
        }
        format!(
            r#"{{
  "id": "de.gronenberg.looksmaxxer",
  "name": "Looksmaxxer",
  "version": "{}",
  "kind": "filter",
  "author": "Marc Gronenberg",
  "description": "Grading tools in free order, set up in the Looksmaxxer window.",
  "params": {{
{params}    "strength": {{"type": "number", "min": 0, "max": 100, "default": 100}}
  }},
  "overlap": 0,
  "area": "content"
}}"#,
            env!("CARGO_PKG_VERSION")
        )
    })
}

/// Die Tools für die Oberfläche, als JSON: Plätze, und je Tool Kennung, Name, Beschreibung und
/// Regler (die Feldnamen sind die der Oberfläche).
pub fn tools_json() -> &'static str {
    static TOOLS: OnceLock<String> = OnceLock::new();
    TOOLS.get_or_init(|| {
        let text = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
        let tools: Vec<String> = tools::ALL
            .iter()
            .map(|t| {
                let params: Vec<String> = t
                    .params
                    .iter()
                    .map(|p| {
                        format!(
                            r#"{{"k":{},"label":{},"min":{},"max":{},"step":{},"def":{},"unit":{},"digits":{},"signed":{},"hue":{},"group":{},"neutral":{},"toggle":{}}}"#,
                            text(p.key), text(p.label), p.min, p.max, p.step, p.default, text(p.unit), p.digits, p.signed, p.hue, text(p.group), p.neutral, p.toggle
                        )
                    })
                    .collect();
                format!(r#"{{"id":{},"name":{},"what":{},"swatch":{},"overlay":{},"params":[{}]}}"#, text(t.id), text(t.name), text(t.description), text(t.swatch.name()), text(t.overlay), params.join(","))
            })
            .collect();
        format!(r#"{{"slots":{SLOTS},"tools":[{}]}}"#, tools.join(","))
    })
}

/// Bit 8 des `format`-Arguments: der letzte Kanal ist Alpha.
const HAS_ALPHA: u32 = 1 << 8;
/// Farbmodi des `format`-Arguments (Bits 16–23), nummeriert wie im PSD-Format.
const MODE_INDEXED: u32 = 2;
const MODE_RGB: u32 = 3;

/// Was der Host pro Aufruf mitgibt.
pub struct Request {
    /// Die Tools mit ihren Werten, in ihrer Reihenfolge; leere Plätze und unbekannte Tools
    /// übersprungen.
    pub tools: Vec<(&'static tools::ToolDef, Vec<f32>)>,
    /// Gesamtstärke 0…1.
    pub mix: f32,
    /// Dokumentposition des ersten Samples im Puffer.
    pub origin: [f32; 2],
    /// Größe der Arbeitsfläche; fehlt sie, gilt die Puffergröße.
    pub canvas: Option<[f32; 2]>,
}

impl Request {
    /// Liest das Parameter-JSON. Unbekannte Tools und leere Plätze werden übersprungen.
    pub fn parse(text: &str) -> Request {
        let top = json::members(text);
        let image = json::object(&top, "_image");
        let inum = |key: &str| image.as_ref().and_then(|m| json::number(m, key)).map(|v| v as f32);
        let tools = (1..=SLOTS)
            .filter_map(|i| {
                let tool = tools::find(json::string(&top, &format!("slot{i}"))?)?;
                // Eine Zahl, die fehlt oder nicht passt, und eine Stufe 0 lassen den Regler auf der Vorgabe.
                let number = |n: usize| json::number(&top, &format!("slot{i}{}", letter(n))).filter(|v| (0.0..=PACKED_MAX).contains(v) && v.fract() == 0.0).map_or(0, |v| v as u64);
                let values = tool
                    .params
                    .iter()
                    .enumerate()
                    .map(|(k, p)| match number(k / PER_NUMBER) >> (16 * (k % PER_NUMBER)) & 0xffff {
                        0 => p.default,
                        stored => value(p, stored - 1),
                    })
                    .collect();
                Some((tool, values))
            })
            .collect();
        Request {
            tools,
            mix: (json::number(&top, "strength").map_or(100.0, |v| v as f32) / 100.0).clamp(0.0, 1.0),
            origin: [inum("x").unwrap_or(0.0), inum("y").unwrap_or(0.0)],
            canvas: match (inum("canvasWidth"), inum("canvasHeight")) {
                (Some(w), Some(h)) if w > 0.0 && h > 0.0 => Some([w, h]),
                _ => None,
            },
        }
    }
}

/// Die Kette auf einem Puffer aus `width × height × channels` verschachtelten `f32`-Samples (RGB
/// oder RGBA, Alpha nicht vormultipliziert). Rückgabe wie `pc_filter`: 0 = in Ordnung.
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
    let canvas = req.canvas.unwrap_or([width as f32, height as f32]);
    // Tools, die von der Lage abhängen, beziehen sich auf die ganze Arbeitsfläche. Jeder Aufruf läuft
    // in PhotoCraft in einer frischen Instanz; die LUTs werden also für jeden Streifen neu
    // gerechnet, und bei kleinen Puffern rechnet die Kette direkt.
    let tools: Vec<(&tools::ToolDef, &[f32])> = req.tools.iter().map(|(t, v)| (*t, v.as_slice())).collect();
    // In PhotoCraft ist jede Instanz frisch; in der Vorschau spart der Zwischenspeicher die LUTs.
    let chain = CACHE.with(|cache| Chain::with_cache(&tools, tools::Frame::of(canvas[0], canvas[1]), req.mix, width * height, &mut cache.borrow_mut()));
    if chain.is_identity() {
        return 0;
    }
    for (row, line) in samples.chunks_exact_mut(width * channels).enumerate() {
        // Positionen im Dokument, damit die Vignette über die Streifen hinweg nahtlos ist.
        let y = req.origin[1] + row as f32 + 0.5;
        for (col, c) in line.chunks_exact_mut(channels).enumerate() {
            // Ganz durchsichtige Pixel haben keine Farbe, die man graden könnte.
            if alpha && c[3] <= 0.0 {
                continue;
            }
            let out = chain.pixel([c[0], c[1], c[2]], req.origin[0] + col as f32 + 0.5, y);
            c[..3].copy_from_slice(&out);
        }
    }
    0
}

// --- PhotoCraft (Plug-in-ABI v1) ------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn pc_abi_version() -> u32 {
    1
}

/// `len << 32 | ptr` des UTF-8-Manifests.
#[unsafe(no_mangle)]
pub extern "C" fn pc_manifest() -> u64 {
    len_ptr(manifest())
}

/// `len << 32 | ptr` eines Texts im Speicher des Moduls.
fn len_ptr(text: &str) -> u64 {
    ((text.len() as u64) << 32) | (text.as_ptr() as usize as u64 & 0xffff_ffff)
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
    // SAFETY: siehe oben.
    let text = unsafe { params_text(params, params_len) };
    filter(samples, width as usize, height as usize, channels as usize, format, text)
}

/// # Safety
/// `params` zeigt auf `params_len` Bytes oder ist null.
unsafe fn params_text<'a>(params: *const u8, params_len: u32) -> &'a str {
    if params.is_null() || params_len == 0 {
        return "";
    }
    // SAFETY: siehe oben.
    std::str::from_utf8(unsafe { std::slice::from_raw_parts(params, params_len as usize) }).unwrap_or("")
}

// --- Vorschau im Browser --------------------------------------------------------------------

/// `format` für [`filter`]: 8 Bit, mit Alpha, RGB.
const RGBA8: u32 = 8 | HAS_ALPHA | MODE_RGB << 16;

thread_local! {
    /// Der Arbeitspuffer in `f32`, zwischen den Aufrufen wiederverwendet.
    static WORK: RefCell<Vec<f32>> = const { RefCell::new(Vec::new()) };
}

thread_local! {
    /// Die letzte Antwort von `lk_pack`/`lk_shape`; gilt bis zum nächsten Aufruf.
    static OUT: RefCell<String> = const { RefCell::new(String::new()) };
}

/// Ruft `f` mit dem UTF-8-Text an `ptr` (`len` Bytes) und gibt `len << 32 | ptr` der Antwort.
///
/// # Safety
/// `ptr` zeigt auf `len` Bytes oder ist null.
unsafe fn answer(ptr: *const u8, len: u32, f: fn(&str) -> String) -> u64 {
    // SAFETY: siehe oben.
    let text = unsafe { params_text(ptr, len) };
    OUT.with(|out| {
        let mut out = out.borrow_mut();
        *out = f(text);
        len_ptr(&out)
    })
}

/// [`pack`] für die Oberfläche: Klartext-Parameter rein, gepackte raus (`len << 32 | ptr`).
///
/// # Safety
/// `ptr` zeigt auf `len` Bytes UTF-8 oder ist null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lk_pack(ptr: *const u8, len: u32) -> u64 {
    // SAFETY: siehe oben.
    unsafe { answer(ptr, len, pack) }
}

thread_local! {
    /// LUTs, die die Vorschau schon gebaut hat.
    static CACHE: RefCell<LutCache> = RefCell::new(LutCache::default());
    /// Die Kette der letzten `lk_gpu`-Antwort: Sie hält die LUTs am Leben, deren Speicher die
    /// Oberfläche gerade hochlädt.
    static GPU_CHAIN: RefCell<Option<Chain>> = const { RefCell::new(None) };
}

/// Die Kette für die Grafikkarte, für eine Fläche von `width × height` Pixeln. Antwort (`len << 32 |
/// ptr`): `{"mix":…,"stages":[{"lut":id,"ptr":…,"len":…},{"code":GLSL,"values":[…12 Werte]}]}`, die Zeiger in
/// den Speicher des Moduls, gültig bis zum nächsten Aufruf; `"stages":null`, wenn die Kette auf der
/// Grafikkarte nicht läuft.
///
/// # Safety
/// `ptr` zeigt auf `len` Bytes UTF-8 (gepackte Parameter wie für [`filter`]) oder ist null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lk_gpu(ptr: *const u8, len: u32, width: u32, height: u32) -> u64 {
    // SAFETY: siehe oben.
    let req = Request::parse(unsafe { params_text(ptr, len) });
    let tools: Vec<(&tools::ToolDef, &[f32])> = req.tools.iter().map(|(t, v)| (*t, v.as_slice())).collect();
    // usize::MAX Pixel: immer mit LUT, die Grafikkarte schlägt nur nach.
    let chain = CACHE.with(|cache| Chain::with_cache(&tools, tools::Frame::of(width as f32, height as f32), req.mix, usize::MAX, &mut cache.borrow_mut()));
    let stages = chain.gpu_stages().map(|stages| {
        stages
            .iter()
            .map(|s| match s {
                GpuStage::Lut(l) => format!(r#"{{"lut":{},"ptr":{},"len":{}}}"#, l.id(), l.data().as_ptr() as usize, l.data().len()),
                GpuStage::Op(g) => {
                    let text = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n");
                    format!(r#"{{"code":"{}","lib":"{}","values":[{}]}}"#, text(g.code), text(g.lib), g.values.map(|v| v.to_string()).join(","))
                }
            })
            .collect::<Vec<_>>()
            .join(",")
    });
    let text = format!(r#"{{"mix":{},"stages":{}}}"#, chain.mix(), stages.map_or("null".into(), |s| format!("[{s}]")));
    GPU_CHAIN.with(|g| *g.borrow_mut() = Some(chain));
    OUT.with(|out| {
        let mut out = out.borrow_mut();
        *out = text;
        len_ptr(&out)
    })
}

/// [`shape`] für die Oberfläche (`len << 32 | ptr`).
///
/// # Safety
/// `ptr` zeigt auf `len` Bytes UTF-8 oder ist null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lk_shape(ptr: *const u8, len: u32) -> u64 {
    // SAFETY: siehe oben.
    unsafe { answer(ptr, len, shape) }
}

/// `len << 32 | ptr` von [`tools_json`]: die Tools für die Oberfläche.
#[unsafe(no_mangle)]
pub extern "C" fn lk_tools() -> u64 {
    len_ptr(tools_json())
}

/// Ein Block von `bytes` Bytes für Bild oder Parameter, oder 0.
#[unsafe(no_mangle)]
pub extern "C" fn lk_alloc(bytes: u32) -> *mut u8 {
    pc_alloc(bytes)
}

/// Gibt einen Block von [`lk_alloc`] wieder frei.
///
/// # Safety
/// `ptr` stammt aus `lk_alloc(bytes)` mit demselben `bytes` und wird danach nicht mehr benutzt.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lk_free(ptr: *mut u8, bytes: u32) {
    if ptr.is_null() {
        return;
    }
    if let Ok(layout) = std::alloc::Layout::from_size_align((bytes as usize).max(8), 8) {
        // SAFETY: siehe oben.
        unsafe { std::alloc::dealloc(ptr, layout) }
    }
}

/// Rechnet die Kette von `src` nach `dst` (je `width × height × 4` Bytes RGBA, wie sie
/// `getImageData` liefert). `params` sind `params_len` Bytes UTF-8-JSON, dieselben wie für
/// PhotoCraft. Rückgabe 0 = in Ordnung, sonst der Fehlercode von [`filter`].
///
/// # Safety
/// `src`, `dst` und `params` zeigen auf Blöcke der genannten Größe; `src` und `dst` dürfen
/// derselbe Block sein, sich aber nicht nur teilweise überlappen.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lk_render(src: *const u8, dst: *mut u8, width: u32, height: u32, params: *const u8, params_len: u32) -> i32 {
    let (w, h) = (width as usize, height as usize);
    let Some(len) = w.checked_mul(h).and_then(|n| n.checked_mul(4)) else { return 1 };
    if src.is_null() || dst.is_null() || len == 0 {
        return 1;
    }
    // SAFETY: siehe oben.
    let text = unsafe { params_text(params, params_len) };
    WORK.with(|work| {
        let mut work = work.borrow_mut();
        work.clear();
        // SAFETY: siehe oben. Die Quelle wird vollständig gelesen, bevor das Ziel geschrieben wird.
        work.extend(unsafe { std::slice::from_raw_parts(src, len) }.iter().map(|v| *v as f32 / 255.0));
        let code = filter(&mut work, w, h, 4, RGBA8, text);
        if code != 0 {
            return code;
        }
        // SAFETY: siehe oben.
        let out = unsafe { std::slice::from_raw_parts_mut(dst, len) };
        for (o, v) in out.iter_mut().zip(work.iter()) {
            *o = (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        }
        0
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const RGB8: u32 = 8 | MODE_RGB << 16;

    fn image(w: usize, h: usize) -> Vec<f32> {
        (0..w * h * 3).map(|i| ((i * 37) % 101) as f32 / 100.0).collect()
    }

    /// Die gepackten Mitglieder (ohne Klammern) für diese Tools, über [`pack`] wie in der
    /// Oberfläche. Regler, die fehlen, nehmen ihre Vorgabe.
    fn slots(tools: &[(&str, &[f32])]) -> String {
        let plain: Vec<String> = tools
            .iter()
            .enumerate()
            .flat_map(|(i, (id, values))| std::iter::once(format!(r#""slot{}":"{id}""#, i + 1)).chain(values.iter().enumerate().map(move |(k, v)| format!(r#""slot{}_{k}":{v}"#, i + 1))))
            .collect();
        let packed = pack(&format!("{{{}}}", plain.join(",")));
        packed[1..packed.len() - 1].to_string()
    }

    #[test]
    fn every_value_survives_packing() {
        for tool in tools::ALL {
            for p in tool.params {
                // 16 Bit je Regler reichen für jede Schrittweite.
                // 16 Bit je Regler, die Stufe 0 ist für „nicht gesetzt“ reserviert.
                assert!(level(p, p.max) + 1 < 1 << 16, "{}.{}: {} Stufen", tool.id, p.key, level(p, p.max));
                for v in [p.min, p.max, p.default, p.neutral] {
                    assert!((value(p, level(p, v)) - v).abs() < 1e-4, "{}.{} = {v}", tool.id, p.key);
                }
            }
        }
        let r = Request::parse(&format!("{{{}}}", slots(&[("vignette", &[-37.0, 140.0, 25.0, -12.3, 44.4, 80.0, 3.17, 1.0])])));
        let got = &r.tools[0].1;
        for (g, e) in got.iter().zip([-37.0, 140.0, 25.0, -12.3, 44.4, 80.0, 3.17, 1.0]) {
            assert!((g - e).abs() < 1e-4, "{got:?}");
        }
    }

    #[test]
    fn the_manifest_is_valid_json_with_all_slots() {
        let top = json::members(manifest());
        assert_eq!(json::string(&top, "id"), Some("de.gronenberg.looksmaxxer"));
        assert_eq!(json::string(&top, "kind"), Some("filter"));
        let params = json::object(&top, "params").expect("params");
        assert_eq!(params.len(), SLOTS * (1 + NUMBERS) + 1);
        assert!(params.len() <= PHOTOCRAFT_MAX_PARAMS, "{} Parameter, PhotoCraft nimmt höchstens {PHOTOCRAFT_MAX_PARAMS}", params.len());
        for (key, spec) in &params {
            assert!(key.bytes().all(|b| b.is_ascii_alphanumeric()), "{key}");
            assert!(json::string(&json::members(spec), "type").is_some(), "{key}");
        }
        // Jedes Tool steht als Auswahl im Manifest.
        let options = json::members(params[0].1).into_iter().find(|(k, _)| *k == "options").expect("options").1;
        for tool in tools::ALL {
            assert!(options.contains(&format!("\"{}\"", tool.id)), "{}", tool.id);
        }
    }

    #[test]
    fn the_tool_list_for_the_window_is_valid_json() {
        let top = json::members(tools_json());
        assert_eq!(json::number(&top, "slots"), Some(SLOTS as f64));
        let list = top.iter().find(|(k, _)| *k == "tools").expect("tools").1;
        for tool in tools::ALL {
            assert!(list.contains(&format!(r#"{{"id":"{}","name":"{}""#, tool.id, tool.name)), "{}", tool.id);
        }
        // Jedes Tool als eigenes Objekt lesbar, mit allen Reglern.
        let first = &list[1..list.find("]}").expect("Ende der Regler") + 2];
        let m = json::members(first);
        assert_eq!(json::string(&m, "id"), Some(tools::ALL[0].id));
    }

    #[test]
    fn parse_reads_the_slots_in_order_and_skips_empty_ones() {
        let chain = slots(&[("vignette", &[90.0, 40.0, 20.0]), ("none", &[]), ("exposure", &[-1.5]), ("bogus", &[]), ("liftGammaGain", &[200.0, 40.0])]);
        // Platz 6: Exposure ohne Zahlen, also mit der Vorgabe; Platz 7 mit einer Zahl, die nicht passt.
        let chain = chain.replace(r#""slot6":"none""#, r#""slot6":"exposure""#).replace(r#""slot7":"none""#, r#""slot7":"exposure","slot7A":-3"#).replace(r#""strength":100"#, r#""strength":50"#);
        let r = Request::parse(&format!(r#"{{{chain},"_image":{{"x":1024,"y":256,"canvasWidth":1532,"canvasHeight":832}}}}"#));
        let ids: Vec<&str> = r.tools.iter().map(|(t, _)| t.id).collect();
        assert_eq!(ids, ["vignette", "exposure", "liftGammaGain", "exposure", "exposure"]);
        assert_eq!(r.tools[0].1, [90.0, 40.0, 20.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        assert_eq!(r.tools[1].1, [-1.5]);
        assert_eq!(r.tools[2].1, [200.0, 40.0, 30.0, 0.0, 45.0, 20.0]);
        assert_eq!(r.tools[3].1, [0.5]);
        assert_eq!(r.tools[4].1, [0.5]);
        assert_eq!(r.mix, 0.5);
        assert_eq!(r.origin, [1024.0, 256.0]);
        assert_eq!(r.canvas, Some([1532.0, 832.0]));
        let empty = Request::parse("");
        assert!(empty.tools.is_empty());
        assert_eq!(empty.mix, 1.0);
    }

    #[test]
    fn zero_means_default_so_photocrafts_own_dialog_is_sane() {
        // PhotoCrafts Dialog schickt für jede Zahl seine Vorgabe 0.
        let r = Request::parse(r#"{"slot1":"vignette","slot1A":0,"slot1B":0,"slot1C":0}"#);
        let vignette = tools::find("vignette").expect("vignette");
        assert_eq!(r.tools[0].1, vignette.defaults());
    }

    #[test]
    fn the_shape_for_the_ring_is_the_one_the_vignette_uses() {
        let text = shape(r#"{"width":160,"height":90,"size":50,"softness":40,"centerX":10,"centerY":-5,"aspect":2,"anamorphic":1}"#);
        let m = json::members(&text);
        let s = tools::vignette::Shape::new(&tools::Frame::of(160.0, 90.0), 50.0, 40.0, 10.0, -5.0, 2.0, true);
        for (k, v) in [("cx", s.cx), ("cy", s.cy), ("rx", s.rx), ("ry", s.ry), ("inner", s.inner)] {
            assert!((json::number(&m, k).expect(k) as f32 - v).abs() < 1e-3, "{k}");
        }
    }

    #[test]
    fn an_empty_chain_changes_nothing() {
        let orig = image(10, 10);
        let mut px = orig.clone();
        assert_eq!(filter(&mut px, 10, 10, 3, RGB8, r#"{"slot1":"none"}"#), 0);
        assert_eq!(px, orig);
        assert_eq!(filter(&mut px, 10, 10, 3, RGB8, &format!("{{{}}}", slots(&[("exposure", &[1.0])]).replace(r#""strength":100"#, r#""strength":0"#))), 0);
        assert_eq!(px, orig);
    }

    #[test]
    fn the_order_of_the_slots_matters() {
        let a = format!("{{{}}}", slots(&[("exposure", &[2.0]), ("shoulder", &[50.0, 100.0])]));
        let b = format!("{{{}}}", slots(&[("shoulder", &[50.0, 100.0]), ("exposure", &[2.0])]));
        let (mut pa, mut pb) = (vec![0.6f32, 0.5, 0.4], vec![0.6f32, 0.5, 0.4]);
        assert_eq!(filter(&mut pa, 1, 1, 3, RGB8, &a), 0);
        assert_eq!(filter(&mut pb, 1, 1, 3, RGB8, &b), 0);
        assert!(pa[0] < 1.0 && pb[0] > 1.0, "{pa:?} vs {pb:?}");
    }

    #[test]
    fn rejects_what_is_not_rgb_and_keeps_alpha() {
        let mut gray = vec![0.5f32; 16];
        assert_eq!(filter(&mut gray, 4, 4, 1, 8 | 1 << 16, "{}"), 2);
        let mut short = vec![0.5f32; 10];
        assert_eq!(filter(&mut short, 4, 4, 4, RGBA8, "{}"), 1);
        let mut px = vec![0.3, 0.5, 0.7, 1.0, 0.3, 0.5, 0.7, 0.0];
        assert_eq!(filter(&mut px, 2, 1, 4, RGBA8, &format!("{{{}}}", slots(&[("warmCool", &[60.0])]))), 0);
        assert_eq!([px[3], px[7]], [1.0, 0.0]);
        assert_eq!(&px[4..7], &[0.3, 0.5, 0.7], "transparent pixel untouched");
        assert_ne!(&px[0..3], &[0.3, 0.5, 0.7], "opaque pixel graded");
    }

    #[test]
    fn bands_give_the_same_result_as_the_whole_image() {
        // Vignette zwischen zwei Farb-Abschnitten: Ein Bild in einem Stück und in zwei
        // Streifen (wie PhotoCraft es aufteilt) muss gleich herauskommen.
        let chain = slots(&[("tealOrange", &[70.0]), ("vignette", &[80.0, 60.0]), ("shoulder", &[40.0, 60.0])]);
        let params = |y: usize, h: usize| format!(r#"{{{chain},"_image":{{"x":0,"y":{y},"width":8,"height":{h},"canvasWidth":8,"canvasHeight":4}}}}"#);
        let mut whole = image(8, 4);
        assert_eq!(filter(&mut whole, 8, 4, 3, RGB8, &params(0, 4)), 0);
        let all = image(8, 4);
        let (mut top, mut bottom) = (all[..48].to_vec(), all[48..].to_vec());
        assert_eq!(filter(&mut top, 8, 2, 3, RGB8, &params(0, 2)), 0);
        assert_eq!(filter(&mut bottom, 8, 2, 3, RGB8, &params(2, 2)), 0);
        top.extend(bottom);
        assert_eq!(whole, top);
    }

    #[test]
    fn the_browser_preview_uses_the_same_filter() {
        let params = format!("{{{}}}", slots(&[("warmCool", &[40.0]), ("vignette", &[80.0, 50.0, 50.0])]));
        let (w, h) = (6u32, 4u32);
        let src: Vec<u8> = (0..w * h * 4).map(|i| if i % 4 == 3 { 255 } else { (i * 29 % 256) as u8 }).collect();
        let mut dst = vec![0u8; src.len()];
        // SAFETY: Puffer der richtigen Größe.
        let code = unsafe { lk_render(src.as_ptr(), dst.as_mut_ptr(), w, h, params.as_ptr(), params.len() as u32) };
        assert_eq!(code, 0);
        let mut expect: Vec<f32> = src.iter().map(|v| *v as f32 / 255.0).collect();
        assert_eq!(filter(&mut expect, w as usize, h as usize, 4, RGBA8, &params), 0);
        for (d, e) in dst.iter().zip(&expect) {
            assert_eq!(*d, (e.clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
        }
    }
}
