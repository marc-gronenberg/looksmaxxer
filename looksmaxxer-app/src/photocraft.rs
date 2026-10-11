//! PhotoCraft über seinen Steuerkanal (JSON-Zeilen über 127.0.0.1, mit Token; siehe
//! docs/control-protocol.md im PhotoCraft-Repository), und ein Ersatz ohne PhotoCraft zum
//! Ausprobieren ([`Demo`]).

use std::io::{self, BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::Command;
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};

pub const PLUGIN_ID: &str = "de.gronenberg.looksmaxxer";
#[cfg(target_os = "macos")]
const BUNDLE: &str = "ai.storyteller.photocraft";
const PROCESS: &str = "PhotoCraft";
/// Die Vorschau wird höchstens so groß (längere Seite, in Pixeln). Angewendet wird immer auf das
/// Dokument in voller Größe.
pub const PREVIEW_MAX: u64 = 1800;
/// PhotoCraft nimmt höchstens 1 MiB je Anfrage an.
const MAX_REQUEST: usize = 1 << 20;

/// Etwas, das der Mensch am Fenster lösen kann. `code` wählt in der Oberfläche die Anleitung.
#[derive(Debug)]
pub struct Problem {
    pub message: String,
    pub code: &'static str,
}

impl Problem {
    pub fn new(message: impl Into<String>) -> Problem {
        Problem { message: message.into(), code: "error" }
    }

    fn code(message: impl Into<String>, code: &'static str) -> Problem {
        Problem { message: message.into(), code }
    }
}

pub type Result<T> = std::result::Result<T, Problem>;

/// Was das Fenster von PhotoCraft braucht. [`PhotoCraft`] spricht mit dem echten, [`Demo`] tut so.
pub trait Host: Send {
    fn status(&mut self) -> Result<Value>;
    /// Das aktive Dokument (wie in `status`) und eine Vorschau davon als PNG.
    fn fetch_preview(&mut self) -> Result<(Value, Vec<u8>)>;
    /// Wendet das Plug-in mit `params` an, wenn das Dokument noch `expect` ist (Name, Revision).
    fn apply(&mut self, params: &Value, expect: &Value) -> Result<Value>;
    fn launch(&mut self) -> Result<()>;
    fn restart(&mut self) -> Result<()>;
    /// Beim Start: PhotoCraft mitstarten, wenn es noch gar nicht läuft.
    fn start(&mut self) {}
}

pub struct PhotoCraft {
    port: u16,
    token_file: PathBuf,
    work_dir: PathBuf,
    /// Das Plug-in, das in PhotoCraft installiert wird (dasselbe Modul wie die Vorschau).
    plugin: &'static [u8],
    plugin_ready: bool,
    schema: Option<Map<String, Value>>,
    plugin_version: String,
}

impl PhotoCraft {
    pub fn new(port: u16, token_file: PathBuf, work_dir: PathBuf, plugin: &'static [u8]) -> PhotoCraft {
        PhotoCraft { port, token_file, work_dir, plugin, plugin_ready: false, schema: None, plugin_version: String::new() }
    }

    // --- Transport ---------------------------------------------------------------------------

    fn address(&self) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], self.port))
    }

    fn token(&self) -> Result<String> {
        let token = std::fs::read_to_string(&self.token_file).unwrap_or_default().trim().to_string();
        if token.is_empty() {
            return Err(Problem::code("PhotoCraft was not started by Looksmaxxer.", "foreign_channel"));
        }
        Ok(token)
    }

    /// Eine Anfrage, eine Antwort. Jede Anfrage nimmt eine frische Verbindung: PhotoCraft schließt
    /// unbenutzte nach 30 Sekunden, und so wird nie eine Anfrage zweimal geschickt.
    fn call(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let line = json!({"id": 1, "method": method, "params": params}).to_string() + "\n";
        if line.len() > MAX_REQUEST {
            return Err(Problem::new("The request to PhotoCraft is too large."));
        }
        let Ok(stream) = TcpStream::connect_timeout(&self.address(), Duration::from_secs(3)) else {
            self.plugin_ready = false;
            return Err(Problem::code("PhotoCraft isn't running with its control channel.", "not_connected"));
        };
        let token = self.token()?;
        let broke = |e: io::Error| Problem::new(format!("The connection to PhotoCraft broke off during “{method}” ({e}). Check in PhotoCraft whether the step happened."));
        stream.set_read_timeout(Some(Duration::from_secs(10))).map_err(broke)?;
        let mut writer = stream.try_clone().map_err(broke)?;
        let mut reader = BufReader::new(stream);
        writer.write_all((json!({"id": "auth", "method": "auth", "params": {"token": token}}).to_string() + "\n").as_bytes()).map_err(broke)?;
        let auth = read_reply(&mut reader, &json!("auth")).map_err(broke)?;
        if auth["ok"] != true {
            self.plugin_ready = false;
            return Err(Problem::code("PhotoCraft's control channel belongs to another program.", "foreign_channel"));
        }
        reader.get_ref().set_read_timeout(Some(timeout)).map_err(broke)?;
        writer.write_all(line.as_bytes()).map_err(broke)?;
        let reply = read_reply(&mut reader, &json!(1)).map_err(broke)?;
        if reply["ok"] != true {
            return Err(Problem::new(format!("PhotoCraft says: {}", reply["error"].as_str().unwrap_or("unknown error"))));
        }
        Ok(reply["result"].clone())
    }

    fn execute(&mut self, command: &str, params: Value, timeout: Duration) -> Result<Value> {
        self.call("engine.execute", json!({"command": command, "params": params}), timeout)
    }

    fn reachable(&self) -> bool {
        TcpStream::connect_timeout(&self.address(), Duration::from_millis(500)).is_ok()
    }

    /// Erreichbar und mit unserem Token.
    fn usable(&mut self) -> bool {
        self.inspect().is_ok()
    }

    fn inspect(&mut self) -> Result<Value> {
        self.call("ui.inspect", json!({}), Duration::from_secs(20))
    }

    // --- Plug-in -----------------------------------------------------------------------------

    /// Installiert das Plug-in in das laufende PhotoCraft (für diese Sitzung von PhotoCraft) und
    /// merkt sich die Parameter aus seinem Manifest.
    fn ensure_plugin(&mut self) -> Result<()> {
        if self.plugin_ready {
            return Ok(());
        }
        let info = self.execute("plugin.install", json!({"data": base64(self.plugin), "replace": true}), Duration::from_secs(180))?;
        let (Some(PLUGIN_ID), Some(schema)) = (info["id"].as_str(), info["paramsSchema"].as_object()) else {
            return Err(Problem::new("PhotoCraft didn't install the plug-in as expected."));
        };
        self.schema = Some(schema.clone());
        self.plugin_version = info["version"].as_str().unwrap_or_default().to_string();
        self.plugin_ready = true;
        Ok(())
    }

    /// Nur Parameter aus dem Manifest, mit passendem Typ und im erlaubten Bereich.
    fn clean_params(&self, params: &Value) -> Result<Value> {
        let (Some(params), Some(schema)) = (params.as_object(), &self.schema) else {
            return Err(Problem::new("The settings are incomplete."));
        };
        let mut out = Map::new();
        for (key, spec) in schema {
            let Some(value) = params.get(key) else { continue };
            match spec["type"].as_str() {
                Some("bool") => {
                    let Some(on) = value.as_bool() else { return Err(Problem::new(format!("“{key}” must be on or off."))) };
                    out.insert(key.clone(), json!(on));
                }
                Some(kind @ ("number" | "int")) => {
                    let Some(v) = value.as_f64().filter(|v| v.is_finite()) else { return Err(Problem::new(format!("“{key}” must be a number."))) };
                    let v = v.clamp(spec["min"].as_f64().unwrap_or(v), spec["max"].as_f64().unwrap_or(v));
                    // Nicht runden: Die Zahlen tragen gepackte Regler (looksmaxxer::level).
                    out.insert(key.clone(), if kind == "int" { json!(v.round() as i64) } else { json!(v) });
                }
                Some("choice") => {
                    if !spec["options"].as_array().is_some_and(|o| o.contains(value)) {
                        return Err(Problem::new(format!("“{key}” has an unknown value.")));
                    }
                    out.insert(key.clone(), value.clone());
                }
                _ => {}
            }
        }
        Ok(Value::Object(out))
    }

    // --- Zustand -----------------------------------------------------------------------------

    /// Was in PhotoCraft gerade im Weg ist.
    fn blockers(ui: &Value) -> Option<String> {
        if let Some(dialog) = ui["dialogs"].as_array().and_then(|d| d.last()) {
            let title = dialog["title"].as_str().filter(|t| !t.is_empty()).unwrap_or("a dialog");
            return Some(format!("“{title}” is open in PhotoCraft. Close it to carry on."));
        }
        if ui["jobs"]["running"].as_bool() == Some(true) || ui["jobs"]["opening"].as_bool() == Some(true) {
            return Some("PhotoCraft is busy computing or opening something. Wait until it's done.".into());
        }
        None
    }

    fn layers<'a>(tree: &'a Value, out: &mut Vec<&'a Value>) {
        for layer in tree.as_array().into_iter().flatten() {
            out.push(layer);
            let children = if layer["children"].is_array() { &layer["children"] } else { &layer["layers"] };
            Self::layers(children, out);
        }
    }

    /// Das aktive Dokument, so wie es die Oberfläche braucht.
    fn describe(ui: &Value) -> Option<Value> {
        let doc = &ui["document"];
        let active_index = &ui["session"]["active"];
        if !doc.is_object() || active_index.is_null() {
            return None;
        }
        let mut layers = Vec::new();
        Self::layers(&doc["layers"], &mut layers);
        let active = layers.iter().find(|l| l["id"] == doc["activeLayer"]);
        let empty = active.is_some_and(|l| {
            let b = &l["bounds"];
            b[2].as_f64().unwrap_or(0.0) == 0.0 && b[3].as_f64().unwrap_or(0.0) == 0.0
        });
        Some(json!({
            "index": active_index, "name": doc["name"], "width": doc["width"], "height": doc["height"],
            "mode": doc["mode"], "depth": doc["depth"], "revision": doc["revision"],
            "layerCount": layers.len(), "activeLayer": active.map(|l| l["name"].clone()),
            "activeLayerEmpty": empty, "hasSelection": doc["hasSelection"].as_bool() == Some(true),
        }))
    }

    /// Schließt das Vorschau-Duplikat und aktiviert wieder das Original. Geschlossen wird nur ein
    /// Dokument, das genau den Namen des Duplikats trägt.
    fn close_copy(&mut self, copy_name: &str, original: &Value) -> Result<()> {
        let docs = |ui: &Value| ui["session"]["documents"].as_array().cloned().unwrap_or_default();
        let ui = self.inspect()?;
        if let Some(d) = docs(&ui).iter().find(|d| d["name"] == copy_name) {
            self.execute("file.close", json!({"document": d["index"]}), Duration::from_secs(180))?;
        }
        let ui = self.inspect()?;
        let list = docs(&ui);
        if list.iter().any(|d| d["name"] == copy_name) {
            return Err(Problem::new(format!("The preview copy “{copy_name}” is still open in PhotoCraft. Close it there without saving.")));
        }
        let target = list
            .iter()
            .find(|d| d["index"] == original["index"] && d["name"] == original["name"])
            .or_else(|| list.iter().find(|d| d["name"] == original["name"]));
        if let Some(target) = target
            && ui["session"]["active"] != target["index"]
        {
            self.execute("document.activate", json!({"document": target["index"]}), Duration::from_secs(180))?;
        }
        Ok(())
    }

    /// Das aktive Dokument, wenn nichts im Weg ist.
    fn active_document(&mut self) -> Result<(Value, Value)> {
        let ui = self.inspect()?;
        let Some(doc) = Self::describe(&ui) else { return Err(Problem::code("No image is open in PhotoCraft.", "no_document")) };
        if let Some(blocked) = Self::blockers(&ui) {
            return Err(Problem::code(blocked, "blocked"));
        }
        Ok((ui, doc))
    }

    fn quit_photocraft(&self) -> Result<()> {
        #[cfg(target_os = "macos")]
        let _ = Command::new("osascript").args(["-e", &format!(r#"tell application id "{BUNDLE}" to quit"#)]).output();
        // Ohne /F: PhotoCraft bekommt die Bitte, sich zu schließen, und fragt nach ungesicherten
        // Änderungen.
        #[cfg(windows)]
        let _ = Command::new("taskkill").args(["/IM", &format!("{PROCESS}.exe")]).output();
        // Ein Signal könnte PhotoCraft ohne Rückfrage beenden; das bleibt dem Menschen überlassen.
        #[cfg(not(any(target_os = "macos", windows)))]
        return Err(Problem::code("Quit PhotoCraft yourself (save your work first), then click again.", "no_channel"));
        #[cfg(any(target_os = "macos", windows))]
        Ok(())
    }
}

impl Host for PhotoCraft {
    fn status(&mut self) -> Result<Value> {
        if !self.reachable() {
            return Ok(json!({"connected": false, "photocraftRunning": running()}));
        }
        let ui = match self.inspect() {
            Err(e) if e.code == "foreign_channel" => return Ok(json!({"connected": false, "photocraftRunning": true, "foreign": true})),
            other => other?,
        };
        self.ensure_plugin()?;
        Ok(json!({
            "connected": true, "document": Self::describe(&ui), "blocked": Self::blockers(&ui),
            "pluginVersion": self.plugin_version, "previewMax": PREVIEW_MAX,
        }))
    }

    /// PhotoCrafts Steuerkanal kann ein Dokument nur speichern, und das macht die Zieldatei zum
    /// Speicherort des Dokuments. Deshalb wird nie das Original gespeichert: Looksmaxxer dupliziert
    /// das Dokument (auf eine Ebene reduziert), verkleinert und speichert das Duplikat, schließt es
    /// und aktiviert wieder das Original.
    fn fetch_preview(&mut self) -> Result<(Value, Vec<u8>)> {
        let (_, original) = self.active_document()?;
        let nonce = format!("{:08x}", SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.subsec_nanos()) ^ std::process::id());
        let copy_name = format!("Looksmaxxer preview {nonce}");
        let file_name = format!("preview-{nonce}.png");
        let _ = std::fs::create_dir_all(&self.work_dir);
        for old in std::fs::read_dir(&self.work_dir).into_iter().flatten().flatten() {
            if old.file_name().to_string_lossy().starts_with("preview-") {
                let _ = std::fs::remove_file(old.path());
            }
        }
        let long = Duration::from_secs(180);
        self.execute("image.duplicate", json!({"name": copy_name, "mergedOnly": true}), long)?;
        let saved = (|| {
            let copy = self.execute("document.inspect", json!({}), long)?;
            if copy["name"] != copy_name.as_str() {
                return Err(Problem::new("PhotoCraft didn't activate the preview copy. Nothing was saved."));
            }
            let (width, height) = (copy["width"].as_u64().unwrap_or(0), copy["height"].as_u64().unwrap_or(0));
            if width.max(height) > PREVIEW_MAX {
                let side = if width >= height { "width" } else { "height" };
                self.execute("image.imageSize", json!({side: PREVIEW_MAX, "resample": "bilinear"}), long)?;
                if self.execute("document.inspect", json!({}), long)?["name"] != copy_name.as_str() {
                    return Err(Problem::new("PhotoCraft switched the active document. Nothing was saved."));
                }
            }
            // Gespeichert wird nur, wenn sicher das Duplikat aktiv ist.
            self.call("app.save", json!({"path": file_name}), long)?;
            std::fs::read(self.work_dir.join(&file_name)).map_err(|e| Problem::new(format!("The preview file couldn't be read: {e}")))
        })();
        let closed = self.close_copy(&copy_name, &original);
        let png = saved?;
        closed?;
        let _ = std::fs::remove_file(self.work_dir.join(&file_name));
        if !png.starts_with(b"\x89PNG") {
            return Err(Problem::new("PhotoCraft didn't deliver a PNG file."));
        }
        Ok((original, png))
    }

    /// Das ist ein Schritt im Protokoll von PhotoCraft; Cmd+Z nimmt ihn dort zurück.
    fn apply(&mut self, params: &Value, expect: &Value) -> Result<Value> {
        let (_, doc) = self.active_document()?;
        if doc["name"] != expect["name"] || doc["revision"] != expect["revision"] {
            return Err(Problem::code("The document in PhotoCraft has changed since the preview. Reload the image, then apply.", "stale"));
        }
        if doc["mode"] != "Rgb" {
            return Err(Problem::new("Looksmaxxer only works on RGB documents."));
        }
        self.plugin_ready = false; // PhotoCraft kann inzwischen neu gestartet worden sein
        self.ensure_plugin()?;
        let clean = self.clean_params(params)?;
        self.execute("plugin.run", json!({"id": PLUGIN_ID, "params": clean}), Duration::from_secs(600))?;
        let ui = self.inspect()?;
        match Self::describe(&ui) {
            Some(after) if after["name"] == doc["name"] && after["revision"] != doc["revision"] => Ok(after),
            _ => Err(Problem::new("PhotoCraft accepted the plug-in, but the document is unchanged.")),
        }
    }

    /// Startet PhotoCraft mit Steuerkanal. Ein laufendes PhotoCraft wird nicht angefasst.
    fn launch(&mut self) -> Result<()> {
        if self.reachable() {
            return Ok(());
        }
        if running() {
            return Err(Problem::code("PhotoCraft is already running, but without its control channel. Save your work, quit PhotoCraft, then click again.", "no_channel"));
        }
        let work = self.work_dir.to_string_lossy().to_string();
        let flags = ["--control".to_string(), self.port.to_string(), "--control-token-file".into(), self.token_file.to_string_lossy().into(), "--automation-read-root".into(), work.clone(), "--automation-write-root".into(), work];
        #[cfg(target_os = "macos")]
        {
            let done = Command::new("open").args(["-b", BUNDLE, "--args"]).args(&flags).output().map_err(|e| Problem::new(format!("PhotoCraft couldn't be started: {e}")))?;
            if !done.status.success() {
                return Err(Problem::new(format!("PhotoCraft couldn't be started: {}", String::from_utf8_lossy(&done.stderr).trim())));
            }
        }
        // shortcut: auf Windows und Linux muss PhotoCraft im Suchpfad liegen; sonst von Hand mit
        // denselben Optionen starten (LIESMICH). Besser: den Installationsort von PhotoCraft kennen.
        #[cfg(not(target_os = "macos"))]
        Command::new(PROCESS)
            .args(&flags)
            .spawn()
            .map_err(|e| Problem::code(format!("PhotoCraft couldn't be started ({e}). Put PhotoCraft on the PATH, or start it yourself with its control channel (see LIESMICH)."), "not_connected"))?;
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if self.reachable() && self.token_file.exists() {
                return Ok(());
            }
            sleep(Duration::from_millis(400));
        }
        Err(Problem::code("PhotoCraft started, but its control channel doesn't answer. Is a second PhotoCraft running?", "not_connected"))
    }

    /// Bittet ein PhotoCraft, das ohne unseren Steuerkanal läuft, sich zu beenden, und startet es
    /// mit Steuerkanal neu. PhotoCraft fragt dabei selbst nach ungesicherten Änderungen; wer dort
    /// abbricht, bricht auch den Neustart ab.
    fn restart(&mut self) -> Result<()> {
        if self.usable() {
            return Ok(());
        }
        if running() {
            self.quit_photocraft()?;
            let deadline = Instant::now() + Duration::from_secs(90);
            while running() && Instant::now() < deadline {
                sleep(Duration::from_millis(500));
            }
            if running() {
                return Err(Problem::code("PhotoCraft is still open. Answer its question about saving, or quit it yourself, then click again.", "no_channel"));
            }
            sleep(Duration::from_millis(500));
        }
        self.launch()
    }

    fn start(&mut self) {
        if !self.reachable() && !running() {
            if let Err(e) = self.launch() {
                eprintln!("PhotoCraft: {}", e.message);
            }
        }
    }
}

/// Läuft ein PhotoCraft, mit oder ohne Steuerkanal?
fn running() -> bool {
    #[cfg(windows)]
    return Command::new("tasklist")
        .args(["/FI", &format!("IMAGENAME eq {PROCESS}.exe"), "/NH"])
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains(&format!("{PROCESS}.exe")));
    #[cfg(not(windows))]
    Command::new("pgrep").args(["-ix", PROCESS]).output().is_ok_and(|o| o.status.success())
}

fn read_reply(reader: &mut BufReader<TcpStream>, id: &Value) -> io::Result<Value> {
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "no answer"));
        }
        let reply: Value = serde_json::from_str(&line).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        if reply.get("id") == Some(id) {
            return Ok(reply);
        }
    }
}

/// Standard-Base64 mit Auffüllung.
pub fn base64(data: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, b)| n | (*b as u32) << (16 - 8 * i));
        for i in 0..4 {
            out.push(if i <= chunk.len() { ABC[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    out
}

/// Ohne PhotoCraft: Ein PNG steht für das Dokument, „Apply" tut nur so. Zum Ausprobieren der
/// Oberfläche (`--demo BILD.png`).
pub struct Demo {
    png: Vec<u8>,
    doc: Value,
}

impl Demo {
    pub fn new(path: &std::path::Path) -> std::result::Result<Demo, String> {
        let png = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        if !png.starts_with(b"\x89PNG") || png.get(12..16) != Some(b"IHDR") {
            return Err(format!("Kein PNG: {}", path.display()));
        }
        let size = |at: usize| u32::from_be_bytes([png[at], png[at + 1], png[at + 2], png[at + 3]]);
        let name = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let doc = json!({
            "index": 0, "name": name, "width": size(16), "height": size(20), "mode": "Rgb", "depth": 8, "revision": 1,
            "layerCount": 1, "activeLayer": "Background", "activeLayerEmpty": false, "hasSelection": false,
        });
        Ok(Demo { png, doc })
    }
}

impl Host for Demo {
    fn status(&mut self) -> Result<Value> {
        Ok(json!({"connected": true, "document": self.doc, "blocked": null, "pluginVersion": "demo", "previewMax": PREVIEW_MAX, "demo": true}))
    }

    fn fetch_preview(&mut self) -> Result<(Value, Vec<u8>)> {
        Ok((self.doc.clone(), self.png.clone()))
    }

    fn apply(&mut self, params: &Value, _: &Value) -> Result<Value> {
        if !params.is_object() {
            return Err(Problem::new("The settings are incomplete."));
        }
        sleep(Duration::from_millis(600));
        self.doc["revision"] = json!(self.doc["revision"].as_i64().unwrap_or(0) + 1);
        Ok(self.doc.clone())
    }

    fn launch(&mut self) -> Result<()> {
        Ok(())
    }

    fn restart(&mut self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_standard() {
        for (raw, coded) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("fooba", "Zm9vYmE="), ("foobar", "Zm9vYmFy")] {
            assert_eq!(base64(raw.as_bytes()), coded);
        }
        assert_eq!(base64(&[0xff, 0xfe, 0x00]), "//4A");
    }

    #[test]
    fn clean_params_keeps_only_known_values_in_range() {
        let mut pc = PhotoCraft::new(1, PathBuf::new(), PathBuf::new(), &[]);
        pc.schema = serde_json::from_str::<Value>(r#"{"slot1":{"type":"choice","options":["none","exposure"]},"slot1A":{"type":"number","min":-360,"max":360},"on":{"type":"bool"}}"#).unwrap().as_object().cloned();
        let clean = pc.clean_params(&json!({"slot1": "exposure", "slot1A": 999.12345, "on": true, "extra": 1})).unwrap();
        assert_eq!(clean, json!({"slot1": "exposure", "slot1A": 360.0, "on": true}));
        assert!(pc.clean_params(&json!({"slot1": "rm -rf"})).is_err());
        assert!(pc.clean_params(&json!({"slot1A": "1"})).is_err());
        assert!(pc.clean_params(&json!([1])).is_err());
    }
}
