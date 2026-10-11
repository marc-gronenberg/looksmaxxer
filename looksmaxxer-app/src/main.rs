//! Looksmaxxer: die Werkbank für die Look-Kette in PhotoCraft.
//!
//! Ein Fenster mit der Web-Ansicht des Systems (WebKit auf macOS, WebView2 auf Windows,
//! WebKitGTK auf Linux). Die Oberfläche (`ui/index.html`) rechnet die Vorschau selbst mit
//! `looksmaxxer.wasm`; alles, was PhotoCraft betrifft, fragt sie unter `/api/…` bei diesem Programm
//! an. Diese Adressen gibt es nur innerhalb des Fensters (eigenes Schema `looksmaxxer:`), kein
//! anderes Programm und keine Webseite erreicht sie.
//!
//! Optionen: `--demo BILD.png` (ohne PhotoCraft ausprobieren), `--control-port 47878`, `--verbose`
//! (jede Anfrage der Oberfläche auf die Standardfehlerausgabe).
//!
//! Auf macOS schließt „Fenster schließen" (und Apply/Cancel) nur das Fenster; Looksmaxxer bleibt im
//! Dock, ein Klick darauf öffnet es wieder, Cmd+Q beendet. Auf Windows und Linux beendet das
//! Schließen das Programm.

mod photocraft;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use serde_json::{json, Value};
use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::window::{Window, WindowBuilder};
use wry::http::{header, Request, Response};
use wry::WebViewBuilder;

use photocraft::{Demo, Host, PhotoCraft, Problem};

const INDEX: &[u8] = include_bytes!("../ui/index.html");
const FONT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/font.woff2")); // leer ohne Schrift, siehe build.rs
/// Vorschau und PhotoCraft-Plug-in in einem; `build.rs` sagt, wie es entsteht.
const MODULE: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/wasm32-unknown-unknown/release/looksmaxxer.wasm"));

/// So groß darf eine Anfrage der Oberfläche sein (die Bibliothek der Looks ist die größte).
const MAX_BODY: usize = 512 * 1024;
/// So viele eigene Looks hebt die Bibliothek auf.
const MAX_LOOKS: usize = 200;

#[cfg(windows)]
const START_URL: &str = "http://looksmaxxer.localhost/";
#[cfg(not(windows))]
const START_URL: &str = "looksmaxxer://localhost/";

const CSP: &str = "default-src 'none'; script-src 'unsafe-inline' 'wasm-unsafe-eval'; style-src 'unsafe-inline'; img-src 'self' blob: data:; font-src 'self'; connect-src 'self'; worker-src blob:; base-uri 'none'; form-action 'none'";

/// Der private Ordner für Token, Arbeitsdateien und die Bibliothek.
fn state_dir() -> PathBuf {
    let home = || std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    if cfg!(target_os = "macos") {
        home().join("Library/Application Support/Looksmaxxer")
    } else if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_default().join("Looksmaxxer")
    } else {
        std::env::var_os("XDG_STATE_HOME").map_or_else(|| home().join(".local/state"), PathBuf::from).join("looksmaxxer")
    }
}

/// Die eigenen Looks und der letzte Stand des Fensters, als JSON im privaten Ordner.
struct Library {
    path: PathBuf,
    guard: Mutex<()>,
}

impl Library {
    fn read(&self) -> Value {
        let _held = lock(&self.guard);
        self.load()
    }

    /// Die Datei lesen; nur mit gehaltener Sperre aufrufen.
    fn load(&self) -> Value {
        let data: Value = std::fs::read(&self.path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        json!({
            "custom": if data["custom"].is_array() { data["custom"].clone() } else { json!([]) },
            "last": if data["last"].is_object() { data["last"].clone() } else { Value::Null },
        })
    }

    /// Übernimmt `custom` und/oder `last`. Was die Oberfläche hineinschreibt, liest nur sie wieder;
    /// geprüft wird hier nur die Form.
    fn write(&self, body: &Value) -> Result<(), Problem> {
        // Lesen und Schreiben unter derselben Sperre: Sonst legt ein gleichzeitiger zweiter
        // Speichervorgang (etwa der letzte Stand) die alte Look-Liste zurück.
        let _held = lock(&self.guard);
        let mut data = self.load();
        if let Some(custom) = body.get("custom") {
            if !custom.as_array().is_some_and(|l| l.len() <= MAX_LOOKS && l.iter().all(Value::is_object)) {
                return Err(Problem::new(format!("The look library must be a list of at most {MAX_LOOKS} looks.")));
            }
            data["custom"] = custom.clone();
        }
        if let Some(last) = body.get("last") {
            if !(last.is_object() || last.is_null()) {
                return Err(Problem::new("The last state must be an object."));
            }
            data["last"] = last.clone();
        }
        let tmp = self.path.with_extension("tmp");
        let saved = std::fs::write(&tmp, serde_json::to_vec_pretty(&data).unwrap_or_default()).and_then(|()| {
            private(&tmp, 0o600);
            std::fs::rename(&tmp, &self.path)
        });
        saved.map_err(|e| Problem::new(format!("The looks couldn't be saved: {e}")))
    }
}

/// Nur für den eigenen Benutzer lesbar (Unix; auf Windows ist der Ordner ohnehin privat).
fn private(path: &Path, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
}

fn lock<T: ?Sized>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

struct App {
    host: Mutex<Box<dyn Host>>,
    library: Library,
}

fn respond(status: u16, content_type: &str, body: Vec<u8>) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "no-store")
        .body(body)
        .expect("gültige Antwort")
}

fn json_response(status: u16, value: Value) -> Response<Vec<u8>> {
    respond(status, "application/json; charset=utf-8", value.to_string().into_bytes())
}

fn problem(e: Problem) -> Response<Vec<u8>> {
    json_response(409, json!({"ok": false, "error": e.message, "code": e.code}))
}

/// `{"ok": true, …value}`.
fn ok(value: Value) -> Response<Vec<u8>> {
    let mut out = json!({"ok": true});
    if let (Some(out), Some(value)) = (out.as_object_mut(), value.as_object()) {
        out.extend(value.clone());
    }
    json_response(200, out)
}

/// Eine Anfrage der Oberfläche. Läuft in einem eigenen Faden: Was PhotoCraft betrifft, dauert
/// mitunter Sekunden.
fn handle(app: &App, request: Request<Vec<u8>>) -> Response<Vec<u8>> {
    let path = request.uri().path();
    match (request.method().as_str(), path) {
        ("GET", "/") => {
            let mut page = respond(200, "text/html; charset=utf-8", INDEX.to_vec());
            page.headers_mut().insert(header::CONTENT_SECURITY_POLICY, CSP.parse().expect("gültiger Header"));
            page
        }
        ("GET", "/looksmaxxer.wasm") => respond(200, "application/wasm", MODULE.to_vec()),
        ("GET", "/switzer.woff2") if !FONT.is_empty() => respond(200, "font/woff2", FONT.to_vec()),
        ("GET", "/api/status") => match lock(&app.host).status() {
            Ok(status) => ok(status),
            Err(e) => problem(e),
        },
        ("GET", "/api/library") => ok(app.library.read()),
        ("POST", _) => {
            let body: Value = if request.body().is_empty() { json!({}) } else { serde_json::from_slice(request.body()).unwrap_or(Value::Null) };
            if request.body().len() > MAX_BODY || !body.is_object() {
                return problem(Problem::new("The request isn't valid JSON."));
            }
            let result = match path {
                "/api/launch" => lock(&app.host).launch().map(|()| json!({})),
                "/api/restart" => lock(&app.host).restart().map(|()| json!({})),
                "/api/apply" => lock(&app.host).apply(&body["params"], &body["document"]).map(|after| json!({"document": after})),
                "/api/library" => app.library.write(&body).map(|()| json!({})),
                "/api/preview" => {
                    return match lock(&app.host).fetch_preview() {
                        Ok((doc, png)) => {
                            let mut image = respond(200, "image/png", png);
                            image.headers_mut().insert("X-Look-Document", photocraft::base64(doc.to_string().as_bytes()).parse().expect("Base64 ist ein gültiger Header"));
                            image
                        }
                        Err(e) => problem(e),
                    };
                }
                _ => return json_response(404, json!({"ok": false, "error": "Not found."})),
            };
            match result {
                Ok(value) => ok(value),
                Err(e) => problem(e),
            }
        }
        _ => respond(404, "text/plain; charset=utf-8", b"Not found.".to_vec()),
    }
}

enum UserEvent {
    /// Die Oberfläche ist fertig (Apply oder Cancel).
    Close,
}

/// Das Menü auf macOS: Ohne Bearbeiten-Menü gingen Kopieren und Einfügen im Fenster nicht.
#[cfg(target_os = "macos")]
fn menu() -> muda::Menu {
    use muda::{Menu, PredefinedMenuItem as Item, Submenu};
    let app = Submenu::with_items("Looksmaxxer", true, &[&Item::hide(None), &Item::hide_others(None), &Item::separator(), &Item::quit(None)]).expect("Menü");
    let edit = Submenu::with_items("Edit", true, &[&Item::undo(None), &Item::redo(None), &Item::separator(), &Item::cut(None), &Item::copy(None), &Item::paste(None), &Item::select_all(None)]).expect("Menü");
    let menu = Menu::with_items(&[&app, &edit]).expect("Menü");
    menu.init_for_nsapp();
    menu
}

/// Fenster zu: auf macOS nur verstecken (Looksmaxxer bleibt im Dock), sonst beenden.
fn close(window: &Window, control_flow: &mut ControlFlow) {
    if cfg!(target_os = "macos") {
        window.set_visible(false);
    } else {
        *control_flow = ControlFlow::Exit;
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let (mut demo, mut control_port, mut verbose) = (None, 47878u16, false);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--demo" => demo = args.next().map(PathBuf::from),
            "--verbose" => verbose = true,
            "--control-port" => control_port = args.next().and_then(|p| p.parse().ok()).unwrap_or(control_port),
            other if other.starts_with("-psn_") => {} // macOS gibt beim Start aus dem Finder eine Prozessnummer mit
            other => {
                eprintln!("Unbekannte Option {other}. Möglich: --demo BILD.png, --control-port PORT, --verbose");
                std::process::exit(2);
            }
        }
    }

    let base = state_dir();
    let work = base.join("work");
    if let Err(e) = std::fs::create_dir_all(&work) {
        eprintln!("{}: {e}", work.display());
        std::process::exit(1);
    }
    private(&base, 0o700);
    let host: Box<dyn Host> = match demo {
        Some(png) => Box::new(Demo::new(&png).unwrap_or_else(|e| {
            eprintln!("{e}");
            std::process::exit(2)
        })),
        None => Box::new(PhotoCraft::new(control_port, base.join("photocraft-control.token"), work, MODULE)),
    };
    let app = Arc::new(App { host: Mutex::new(host), library: Library { path: base.join("looks.json"), guard: Mutex::new(()) } });

    // PhotoCraft gleich mitstarten, wenn es noch gar nicht läuft. Läuft es ohne Steuerkanal,
    // entscheidet der Mensch im Fenster über den Neustart.
    let starter = app.clone();
    std::thread::spawn(move || lock(&starter.host).start());

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    #[cfg(target_os = "macos")]
    let _menu = menu();
    let window = WindowBuilder::new()
        .with_title("Looksmaxxer")
        .with_inner_size(LogicalSize::new(1440.0, 900.0))
        .with_min_inner_size(LogicalSize::new(900.0, 600.0))
        .build(&event_loop)
        .expect("Fenster");

    let proxy = event_loop.create_proxy();
    let builder = WebViewBuilder::new()
        .with_url(START_URL)
        .with_background_color((14, 15, 16, 255))
        .with_asynchronous_custom_protocol("looksmaxxer".into(), move |_, request, responder| {
            let app = app.clone();
            std::thread::spawn(move || {
                let what = format!("{} {}", request.method(), request.uri().path());
                let response = handle(&app, request);
                if verbose {
                    eprintln!("{what} → {}", response.status().as_u16());
                }
                responder.respond(response);
            });
        })
        .with_ipc_handler(move |message| {
            if message.body() == "close" {
                let _ = proxy.send_event(UserEvent::Close);
            }
        });
    #[cfg(not(any(target_os = "macos", windows)))]
    let webview = {
        use tao::platform::unix::WindowExtUnix;
        use wry::WebViewBuilderExtUnix;
        builder.build_gtk(window.default_vbox().expect("GTK-Fenster"))
    };
    #[cfg(any(target_os = "macos", windows))]
    let webview = builder.build(&window);
    let webview = webview.expect("Web-Ansicht");

    event_loop.run(move |event, _, control_flow| {
        let _ = &webview; // die Web-Ansicht lebt so lange wie die Schleife
        *control_flow = ControlFlow::Wait;
        match event {
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } | Event::UserEvent(UserEvent::Close) => close(&window, control_flow),
            // Klick aufs Dock-Symbol oder ein zweiter Start: das Fenster wieder zeigen.
            #[cfg(target_os = "macos")]
            Event::Reopen { .. } => {
                window.set_visible(true);
                window.set_focus();
                let _ = webview.evaluate_script("window.looksmaxxer && window.looksmaxxer.reopen()");
            }
            _ => {}
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simultaneous_saves_keep_both_looks_and_last_state() {
        let dir = std::env::temp_dir().join(format!("looksmaxxer-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let library = Arc::new(Library { path: dir.join("looks.json"), guard: Mutex::new(()) });
        // Viele Fäden schreiben abwechselnd die Look-Liste und den letzten Stand.
        let threads: Vec<_> = (0..16)
            .map(|i| {
                let library = library.clone();
                std::thread::spawn(move || {
                    let body = if i % 2 == 0 { json!({"custom": [{"id": "look", "name": "Mine"}]}) } else { json!({"last": {"strength": i}}) };
                    library.write(&body).unwrap();
                })
            })
            .collect();
        threads.into_iter().for_each(|t| t.join().unwrap());
        let data = library.read();
        assert_eq!(data["custom"][0]["name"], "Mine");
        assert!(data["last"]["strength"].is_number());
        assert!(library.write(&json!({"custom": "nope"})).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
