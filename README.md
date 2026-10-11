# Look-Kette (Prototyp)

Ein Grading-Effekt mit sechs Tools in fester Reihenfolge, jedes einzeln abschaltbar:
Warm/Cool → Belichtung → Schulter → Teal-Orange → Lift/Gamma/Gain → Vignette, am Ende eine
Gesamtstärke. Derselbe Kern läuft als Plug-in in EffectCraft und in PhotoCraft.

Dieselben Tools gibt es auch frei sortiert, jedes beliebig oft: in **Looksmaxxer**, der Werkbank
für PhotoCraft (siehe [looksmaxxer-app/LIESMICH.md](looksmaxxer-app/LIESMICH.md)).

## Aufbau

| Ordner | Inhalt |
|---|---|
| `core/` | Die 3D-LUT, die feste Kette (`Grade`, `Look`) und die frei sortierte (`Chain`), dazu ein kleiner JSON-Leser. Kennt keine der Apps. |
| `core/src/tools/` | Die Tools, eines pro Datei, mit Name, Beschreibung, Reglern und Rechnung. Die Liste `ALL` in `tools/mod.rs` bestimmt, welche es gibt. |
| `effectcraft/` | Adapter für EffectCrafts Plug-in-API v1 (`ec_render`): Manifest, Parameter, vormultipliziertes RGBA. |
| `photocraft/` | Adapter für PhotoCrafts Plug-in-ABI v1 (`pc_filter`): Manifest, Parameter-JSON, RGB/RGBA mit nicht vormultipliziertem Alpha. |
| `looksmaxxer/` | Die frei sortierte Kette als ein Modul für zwei Zwecke: PhotoCraft-Plug-in „Looksmaxxer" (`pc_filter`) und Vorschau im Looksmaxxer-Fenster (`lk_render`, `lk_tools`). Die Kette kommt in 15 festen Plätzen an (`slot1` = Tool, `slot1A`…`slot1C` = Regler, je drei als 16-Bit-Stufen in eine Zahl gepackt), weil PhotoCrafts Parameter keinen Text kennen und höchstens 64 sein dürfen. Manifest und Tool-Liste entstehen aus `tools::ALL`. |
| `looksmaxxer-app/` | Looksmaxxer selbst: ein Rust-Programm mit Fenster (wry/tao, die Web-Ansicht des Systems) für macOS, Windows und Linux. Bettet die Oberfläche (`ui/`) und `looksmaxxer.wasm` ein und spricht mit PhotoCraft über dessen Steuerkanal. |

**Tools in Looksmaxxer** ändern, hinzufügen und entfernen: nur in `core/src/tools/` (Anleitung oben
in `tools/mod.rs`). Plug-in-Manifest, Parameter-Leser, Werkzeugkasten und Regler der Oberfläche
holen sich alles von dort; danach `looksmaxxer.wasm` und `looksmaxxer-app` neu bauen.

Die feste Kette für EffectCraft und das Plug-in „Look-Kette (Prototyp)" nutzen dieselbe Rechnung
aus den Tool-Dateien. Neue Regler brauchen dort zusätzlich einen Eintrag im Manifest des Adapters.

## Bauen

Braucht Rust 1.88 oder neuer.

```sh
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown \
  -p look-kette-effectcraft -p look-kette-photocraft -p looksmaxxer
cargo test
```

Ergebnis in `target/wasm32-unknown-unknown/release/`: `look_kette_effectcraft.wasm`,
`look_kette_photocraft.wasm` und `looksmaxxer.wasm`.

- `look_kette_photocraft.wasm` ist das Plug-in mit dem festen Dialog (siehe „Laden").
- `looksmaxxer.wasm` steckt im Programm von Looksmaxxer. Danach dieses bauen:
  `cargo build --release -p looksmaxxer-app`; Ergebnis `target/release/looksmaxxer`, für macOS nach
  `Looksmaxxer.app/Contents/MacOS/looksmaxxer` (siehe [looksmaxxer-app/LIESMICH.md](looksmaxxer-app/LIESMICH.md)).

## Laden

**EffectCraft:** Effect ▸ Load Effect Plug-in…, danach im Panel Effects & Presets unter
Color Correction. Dauerhaft: die Datei nach
`~/Library/Application Support/EffectCraft/Plug-ins` kopieren (wird beim Start geladen).

**PhotoCraft mit eigenem Fenster:** Looksmaxxer starten (siehe
[looksmaxxer-app/LIESMICH.md](looksmaxxer-app/LIESMICH.md)). Die App installiert ihr Plug-in selbst.

**PhotoCraft mit dem festen Dialog:** Filter ▸ Plug-ins ▸ Install Plug-in…, danach unter Filter ▸ Plug-ins. Dauerhaft:
unter Preferences ▸ Plug-ins einen „Additional Plug-ins Folder" eintragen und die Datei dort
ablegen.

## Unterschiede zwischen den beiden Plug-ins

| | EffectCraft | PhotoCraft |
|---|---|---|
| Lift/Gamma/Gain | drei Farbfelder (Grau = neutral) | Lift und Gain je als Farbton (0–360°, 0 = Rot) und Betrag; Gamma bleibt neutral |
| Mitte der Vignette | Punkt auf der Ebene | fest in der Mitte der Arbeitsfläche |
| Regler | Schieberegler in den Effect Controls | vier Schieberegler, der Rest Zahlenfelder |
| Beschriftung | frei | aus den Parameternamen erzeugt, deshalb ohne Umlaute |
| Farbmodus | RGB | nur RGB; Graustufen, CMYK, Lab, Alphakanäle und der Maskierungsmodus werden abgelehnt (Fehlercode 2) |

Der Grund für die knappere PhotoCraft-Fassung: Der erzeugte Dialog scrollt nicht. Ein
Schieberegler braucht zwei Zeilen, ein Zahlenfeld eine, und alles muss untereinander ins Fenster
passen.

## Bekannte Grenzen

- Beide Hosts führen Plug-ins in einem Interpreter aus. In EffectCraft rechnet ein Frame auf
  einem Kern; PhotoCraft teilt das Bild in Streifen und rechnet sie parallel.
- Die Farb-Tools laufen über eine 3D-LUT mit 33 Stützstellen je Kanal. Gegenüber der direkten
  Rechnung weicht das im Mittel um weniger als eine Zehntel-Stufe (8 Bit) ab, in einzelnen
  Farben um bis zu zwei bis drei Stufen (`cargo test -p look-kette-core -- --nocapture` gibt die
  Werte aus). Kleine Puffer und Werte über 1 werden direkt gerechnet.
- Die Pixel werden als sRGB-kodiert behandelt, unabhängig vom Farbprofil des Dokuments.

## Schrift

Die Oberfläche von Looksmaxxer ist für die Schrift [Switzer](https://www.fontshare.com/fonts/switzer)
gemacht. Deren Lizenz erlaubt nicht, die Datei weiterzugeben, deshalb liegt sie nicht im Repository.
Wer sie hat, legt sie als `looksmaxxer-app/ui/switzer.woff2` ab; sonst nimmt die Oberfläche die
Systemschrift.

## Lizenz

MIT oder Apache-2.0, nach Wahl ([LICENSE-MIT](LICENSE-MIT), [LICENSE-APACHE](LICENSE-APACHE)).
