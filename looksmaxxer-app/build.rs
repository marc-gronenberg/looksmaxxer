//! Looksmaxxer bettet `looksmaxxer.wasm` ein (Vorschau und PhotoCraft-Plug-in). Es muss vorher
//! gebaut sein:
//!
//!   cargo build --release --target wasm32-unknown-unknown -p looksmaxxer
//!
//! Die Schrift `ui/switzer.woff2` (Switzer von Fontshare) liegt nicht im Repository, ihre Lizenz
//! erlaubt das Weitergeben nicht. Fehlt sie, nimmt die Oberfläche die Systemschrift.

fn main() {
    let module = concat!(env!("CARGO_MANIFEST_DIR"), "/../target/wasm32-unknown-unknown/release/looksmaxxer.wasm");
    println!("cargo:rerun-if-changed={module}");
    if !std::path::Path::new(module).exists() {
        panic!("\n\nlooksmaxxer.wasm fehlt. Zuerst bauen:\n  cargo build --release --target wasm32-unknown-unknown -p looksmaxxer\n\n");
    }
    let font = concat!(env!("CARGO_MANIFEST_DIR"), "/ui/switzer.woff2");
    println!("cargo:rerun-if-changed={font}");
    let out = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("font.woff2");
    std::fs::write(out, std::fs::read(font).unwrap_or_default()).expect("OUT_DIR beschreibbar");
}
