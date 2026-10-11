# Looksmaxxer (0.2.0)

Eine Werkbank für die Look-Kette in PhotoCraft, im Stil von Magic Bullet Looks: Bild holen, Tools
in beliebiger Reihenfolge zu einer Kette zusammenstecken, Vorschau sehen, in PhotoCraft anwenden.
Die Oberfläche ist englisch.

Looksmaxxer ist ein einziges Rust-Programm mit eigenem Fenster. Es braucht weder Python noch einen
bestimmten Browser und läuft wie PhotoCraft auf macOS, Windows und Linux. Der Quellcode liegt in
`Look-Kette Quellcode/looksmaxxer-app`.

## Starten

`Looksmaxxer.app` doppelklicken. Die App darf auch in den Programme-Ordner oder ins Dock.

- Läuft PhotoCraft noch nicht, startet Looksmaxxer es mit eingeschaltetem Steuerkanal.
- Läuft PhotoCraft ohne Steuerkanal, oder mit dem Steuerkanal eines anderen Programms (etwa des
  alten Look-Fensters), bietet das Fenster „Restart PhotoCraft" an. PhotoCraft fragt dabei selbst
  nach ungesicherten Änderungen. Der Steuerkanal lässt sich nur beim Start einschalten.
- Auf macOS bleibt Looksmaxxer nach dem Schließen des Fensters (auch nach Apply) im Dock. Klick
  aufs Symbol öffnet das Fenster wieder, Cmd+Q beendet.
- Beim ersten Start kann macOS nachfragen, ob die App geöffnet werden darf (dann Rechtsklick auf
  die App ▸ Öffnen), und beim Neustart von PhotoCraft, ob Looksmaxxer PhotoCraft steuern darf.

**Windows und Linux:** Das Programm `looksmaxxer` (Windows: `looksmaxxer.exe`) starten. Das
Fenster schließen beendet es. Damit Looksmaxxer PhotoCraft selbst starten kann, muss `PhotoCraft`
im Suchpfad liegen; sonst PhotoCraft von Hand so starten (Pfade siehe „Sicherheit"):

```sh
PhotoCraft --control 47878 --control-token-file <Ordner>/photocraft-control.token --automation-read-root <Ordner>/work --automation-write-root <Ordner>/work
```

Auf Linux beendet Looksmaxxer PhotoCraft für einen Neustart nicht selbst (ein Signal könnte
ungesicherte Arbeit verwerfen); dort PhotoCraft selbst beenden und dann im Fenster klicken. Windows
braucht die WebView2-Laufzeit (bei Windows 10 und 11 dabei), Linux WebKitGTK 4.1.

## Bedienung

| Bereich | Was er tut |
|---|---|
| **Looks** (Leiste links) | Fertige Looks und eigene. Die Bildchen zeigen dein Bild mit dem Look. „Save current look" speichert die Kette als eigenen Look; Doppelklick benennt ihn um, × löscht ihn. |
| **Tools** (Leiste rechts) | Warm/Cool, Exposure, Contrast, Shoulder, Teal-Orange, Lift/Gamma/Gain, Black & White, Gradient, Grad Exposure, Vignette. Klick hängt das Tool hinten an, Ziehen setzt es an eine Stelle der Kette. Jedes Tool darf mehrfach vorkommen. |
| **Kette** (unten) | Von links nach rechts gerechnet. Ziehen sortiert um, aus der Kette ziehen entfernt. ON/OFF schaltet ein Tool ab, ohne es zu entfernen. Die Kachel zeigt, was das Tool mit Graukeil und Farbkeil macht. Tastatur: Alt+←/→ verschiebt, Entf entfernt. |
| **Controls** (rechts) | Die Regler des gewählten Tools. Doppelklick setzt einen Regler auf seinen neutralen Wert (meist 0). |
| **Vignette** | Auf dem Bild liegt ein gestrichelter Ring: im Ring ziehen verschiebt, die vier Griffe ändern Größe und Form. Dazu Strength (positiv abdunkeln, negativ aufhellen), Size, Softness, Center X/Y, Highlights (schützt helle Stellen), Aspect und Anamorphic (Ellipse folgt dem Bildformat). |
| **Gradient** | Ein Verlaufsfilter: am Startpunkt (gefüllter Griff) volle Farbe, zum Endpunkt (hohler Griff) hin auslaufend, dahinter nichts. Beide Griffe lassen sich auf dem Bild ziehen. Dazu Falloff (0.5 = gleichmäßig, darunter weicher, darüber schneller auslaufend), Amount, Exposure Comp. (heller oder dunkler, wo der Filter wirkt), Hue und Saturation der Farbe. |
| **Grad Exposure** | Wie Gradient, aber in Blendenstufen: Stops wirken am Startpunkt voll und laufen zum Endpunkt hin aus (etwa −1 EV, um einen hellen Himmel abzudunkeln). Color Balance (Hue, Saturation) tönt dabei, ohne die Helligkeit zu ändern. |
| **Before** | Bild ohne Look. Taste V gedrückt halten geht auch. |
| **Strength** | Gesamtstärke der Kette. |
| **Apply / Cancel** | Apply wendet die Kette auf die aktive Ebene in PhotoCraft an und schließt das Fenster. Das ist ein Schritt im Protokoll von PhotoCraft; Cmd+Z nimmt ihn dort zurück. Cancel schließt ohne Änderung. |

Die Kette fasst bis zu 15 Tools. Der letzte Stand (Kette, Stärke, Look) und die eigenen Looks
liegen in `looks.json` im privaten Ordner (siehe „Sicherheit") und sind beim nächsten Start wieder
da.

## Tools ändern, hinzufügen, entfernen

Jedes Tool ist eine Datei in `Look-Kette Quellcode/core/src/tools/`. Die Anleitung steht oben in
`tools/mod.rs`; kurz: Datei anlegen (Vorlage `exposure.rs`), in die Liste `ALL` eintragen, neu
bauen (siehe unten). Werkzeugkasten, Regler, Plug-in und Vorschau übernehmen es von selbst.

## Grenzen

- Die Vorschau zeigt das ganze Bild, auf höchstens 1800 Pixel verkleinert, und rechnet nur so
  viele Pixel, wie der Bildschirm zeigt. Sie läuft auf der Grafikkarte (WebGL2): Rust baut aus den
  Farb-Tools eine LUT (zwischengespeichert, solange sich ihre Regler nicht ändern), die Karte
  schlägt nach und rechnet die Vignette. Ohne WebGL2 rechnet die Vorschau auf mehreren Kernen.
  Gegenüber der Rechnung beim Anwenden weicht sie dadurch um höchstens eine Stufe ab, plus der
  Verkleinerung. Angewendet wird auf die aktive Ebene in voller Größe, bei aktiver Auswahl nur in
  der Auswahl.
- Nur RGB-Dokumente.
- Die Farben der Vorschau sind die Zahlenwerte der Datei, ohne Farbmanagement.
- Jede Vignette sitzt in der Mitte der Arbeitsfläche.
- Griffe auf dem Bild enden am Bildrand; liegt ein Griff außerhalb, hilft der Regler.
- Das Plug-in „Looksmaxxer" erscheint in PhotoCraft auch unter Filter ▸ Plug-ins. Sein Dialog dort
  zeigt nur die technischen Plätze (`Slot1`, `Slot1 A` …); gedacht ist es für dieses Fenster.
  Für die feste Kette mit eigenem Dialog gibt es weiter das Plug-in „Look-Kette (Prototyp)".
- Die App für macOS ist nur für Apple-Silicon-Macs gebaut (Intel siehe „Bauen").

## Ohne PhotoCraft ausprobieren

```sh
~/Documents/Looksmaxxer/Looksmaxxer.app/Contents/MacOS/looksmaxxer --demo BILD.png
```

Ein PNG steht dann für das Dokument; Apply tut nur so. Mit `--verbose` schreibt das Programm jede
Anfrage des Fensters ins Terminal.

## Bauen

Im Ordner `Look-Kette Quellcode`, mit Rust 1.88 oder neuer:

```sh
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown -p looksmaxxer
cargo build --release -p looksmaxxer-app
```

Das Programm liegt dann in `target/release/looksmaxxer` (Windows: `looksmaxxer.exe`); es enthält
Oberfläche, Schrift und `looksmaxxer.wasm`. Für macOS nach
`Looksmaxxer.app/Contents/MacOS/looksmaxxer` kopieren. Für Intel-Macs zusätzlich mit
`--target x86_64-apple-darwin` bauen und beide mit `lipo -create` zusammenfügen. Windows und Linux
baut man auf dem jeweiligen System.

## Was in der App steckt

Rechtsklick auf die App ▸ Paketinhalt zeigen ▸ `Contents`:

| Datei | Zweck |
|---|---|
| `MacOS/looksmaxxer` | das ganze Programm: Fenster, Oberfläche, Vorschau-Modul, Plug-in, Steuerkanal zu PhotoCraft |
| `Resources/looksmaxxer.icns` | das Symbol (vorerst das des Look-Fensters) |

## Sicherheit

- Die Oberfläche spricht nur mit dem Programm selbst (eigenes Schema `looksmaxxer:`); kein anderes
  Programm und keine Webseite erreicht sie.
- Der Steuerkanal zu PhotoCraft ist nur von diesem Rechner aus erreichbar (127.0.0.1).
- PhotoCraft erzeugt beim Start ein Token für den Steuerkanal. Es liegt im privaten Ordner, lesbar
  nur für dein Benutzerkonto: macOS `~/Library/Application Support/Looksmaxxer/`, Windows
  `%APPDATA%\Looksmaxxer\`, Linux `~/.local/state/looksmaxxer/`.
- PhotoCraft darf über den Steuerkanal nur in den Arbeitsordner von Looksmaxxer (`work` im privaten
  Ordner) schreiben.
