//! Ein sehr kleiner JSON-Leser für die Parameter-Objekte der Hosts, ohne Abhängigkeiten.
//!
//! Er zerlegt ein Objekt in `(Schlüssel, roher Wert)`-Paare und liest daraus Zahlen, Schalter,
//! einfache Zeichenketten und verschachtelte Objekte. Er prüft nicht, ob das JSON gültig ist:
//! Der Host erzeugt es selbst; Unverständliches wird übersprungen und der Aufrufer nimmt dann
//! seine Vorgabe.

/// Die Mitglieder des JSON-Objekts in `text`, in ihrer Reihenfolge. Leer, wenn `text` kein
/// Objekt ist.
pub fn members(text: &str) -> Vec<(&str, &str)> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = skip_ws(b, 0);
    if b.get(i) != Some(&b'{') {
        return out;
    }
    i += 1;
    loop {
        i = skip_ws(b, i);
        if b.get(i) != Some(&b'"') {
            break; // `}` oder etwas Unerwartetes
        }
        let Some(key_end) = string_end(b, i) else { break };
        let key = &text[i + 1..key_end - 1];
        i = skip_ws(b, key_end);
        if b.get(i) != Some(&b':') {
            break;
        }
        let start = skip_ws(b, i + 1);
        let Some(end) = value_end(b, start) else { break };
        out.push((key, text[start..end].trim_end()));
        i = skip_ws(b, end);
        if b.get(i) != Some(&b',') {
            break;
        }
        i += 1;
    }
    out
}

fn raw<'a>(members: &[(&'a str, &'a str)], key: &str) -> Option<&'a str> {
    members.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

/// Eine endliche Zahl.
pub fn number(members: &[(&str, &str)], key: &str) -> Option<f64> {
    raw(members, key)?.parse::<f64>().ok().filter(|v| v.is_finite())
}

pub fn boolean(members: &[(&str, &str)], key: &str) -> Option<bool> {
    match raw(members, key)? {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// Eine Zeichenkette ohne Escape-Sequenzen (mit ihnen: `None`).
pub fn string<'a>(members: &[(&'a str, &'a str)], key: &str) -> Option<&'a str> {
    let v = raw(members, key)?;
    let inner = v.strip_prefix('"')?.strip_suffix('"')?;
    (!inner.contains('\\')).then_some(inner)
}

/// Die Mitglieder eines verschachtelten Objekts.
pub fn object<'a>(members_of_parent: &[(&'a str, &'a str)], key: &str) -> Option<Vec<(&'a str, &'a str)>> {
    let v = raw(members_of_parent, key)?;
    v.starts_with('{').then(|| members(v))
}

fn skip_ws(b: &[u8], mut i: usize) -> usize {
    while b.get(i).is_some_and(|c| c.is_ascii_whitespace()) {
        i += 1;
    }
    i
}

/// Das Ende (exklusiv) der Zeichenkette, die bei `start` mit `"` beginnt.
fn string_end(b: &[u8], start: usize) -> Option<usize> {
    let mut i = start + 1;
    while let Some(&c) = b.get(i) {
        match c {
            b'\\' => i += 2,
            b'"' => return Some(i + 1),
            _ => i += 1,
        }
    }
    None
}

/// Das Ende (exklusiv) des Werts, der bei `start` beginnt.
fn value_end(b: &[u8], start: usize) -> Option<usize> {
    match b.get(start)? {
        b'"' => string_end(b, start),
        b'{' | b'[' => {
            let mut depth = 0usize;
            let mut i = start;
            while let Some(&c) = b.get(i) {
                match c {
                    b'"' => {
                        i = string_end(b, i)?;
                        continue;
                    }
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        depth = depth.checked_sub(1)?;
                        if depth == 0 {
                            return Some(i + 1);
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            None
        }
        _ => {
            let mut i = start;
            while b.get(i).is_some_and(|c| !matches!(c, b',' | b'}' | b']')) {
                i += 1;
            }
            (i > start).then_some(i)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_flat_and_nested_members() {
        let m = members(r#" { "a" : 1.5 , "on":true,"off" : false, "name":"soft", "n": -3e2, "_image": {"x": 7, "s": "a,b}", "list":[1,{"y":2}]}, "last": 9 } "#);
        assert_eq!(m.iter().map(|p| p.0).collect::<Vec<_>>(), ["a", "on", "off", "name", "n", "_image", "last"]);
        assert_eq!(number(&m, "a"), Some(1.5));
        assert_eq!(number(&m, "n"), Some(-300.0));
        assert_eq!(number(&m, "last"), Some(9.0));
        assert_eq!(boolean(&m, "on"), Some(true));
        assert_eq!(boolean(&m, "off"), Some(false));
        assert_eq!(string(&m, "name"), Some("soft"));
        let image = object(&m, "_image").expect("object");
        assert_eq!(number(&image, "x"), Some(7.0));
        assert_eq!(string(&image, "s"), Some("a,b}"));
        assert_eq!(raw(&image, "list"), Some(r#"[1,{"y":2}]"#));
        // Ein Schlüssel im verschachtelten Objekt ist oben nicht sichtbar.
        assert_eq!(number(&m, "x"), None);
    }

    #[test]
    fn wrong_types_and_missing_keys_are_none() {
        let m = members(r#"{"a":"1","b":1,"c":null,"d":true}"#);
        assert_eq!(number(&m, "a"), None);
        assert_eq!(boolean(&m, "b"), None);
        assert_eq!(number(&m, "c"), None);
        assert_eq!(string(&m, "d"), None);
        assert_eq!(number(&m, "zz"), None);
        assert!(object(&m, "b").is_none());
    }

    #[test]
    fn garbage_does_not_panic() {
        for bad in ["", "   ", "[1,2]", "{", r#"{"a"#, r#"{"a":"#, r#"{"a":{"b":1"#, r#"{"a":"x\"#, "{\"a\":1,,}", "}{", r#"{"a":]}"#, "{\"\u{e9}\":1}"] {
            let _ = members(bad);
        }
        assert_eq!(members("{}"), vec![]);
        assert_eq!(members(r#"{"é":2}"#), vec![("é", "2")]);
    }
}
