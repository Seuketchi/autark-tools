//! `modules.alias`: which kernel module claims which device, by the device's modalias. Patterns use `*`, `?` and
//! `[...]`, as the kernel's own matching does.

use std::path::Path;

/// One line of `modules.alias`: `alias pci:v00008086d00005A84sv*sd*bc*sc*i* i915`.
#[derive(Debug, PartialEq, Eq)]
pub struct Alias {
    pub pattern: String,
    pub module: String,
}

pub fn parse(text: &str) -> Vec<Alias> {
    text.lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            (words.next()? == "alias").then_some(())?;
            Some(Alias { pattern: words.next()?.to_owned(), module: words.next()?.to_owned() })
        })
        .collect()
}

pub fn load(kernel: &str) -> Vec<Alias> {
    std::fs::read_to_string(Path::new("/lib/modules").join(kernel).join("modules.alias")).map(|text| parse(&text)).unwrap_or_default()
}

/// Whether `text` matches the glob `pattern`.
pub fn matches(pattern: &str, text: &str) -> bool {
    fn go(p: &[u8], t: &[u8]) -> bool {
        match p.split_first() {
            None => t.is_empty(),
            Some((b'*', rest)) => (0..=t.len()).any(|skip| go(rest, &t[skip..])),
            Some((b'?', rest)) => !t.is_empty() && go(rest, &t[1..]),
            Some((b'[', rest)) => {
                let Some(close) = rest.iter().position(|c| *c == b']') else { return !t.is_empty() && t[0] == b'[' && go(rest, &t[1..]) };
                let (set, after) = (&rest[..close], &rest[close + 1..]);
                let (negate, set) = match set.split_first() {
                    Some((b'!' | b'^', tail)) => (true, tail),
                    _ => (false, set),
                };
                let Some((c, tail)) = t.split_first() else { return false };
                let mut found = false;
                let mut i = 0;
                while i < set.len() {
                    if i + 2 < set.len() && set[i + 1] == b'-' {
                        found |= set[i] <= *c && *c <= set[i + 2];
                        i += 3;
                    } else {
                        found |= set[i] == *c;
                        i += 1;
                    }
                }
                found != negate && go(after, tail)
            }
            Some((c, rest)) => !t.is_empty() && t[0] == *c && go(rest, &t[1..]),
        }
    }
    go(pattern.as_bytes(), text.as_bytes())
}

/// The modules whose aliases match `modalias`, each once, in the order they're listed.
pub fn modules_for(aliases: &[Alias], modalias: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for alias in aliases {
        if matches(&alias.pattern, modalias) && !found.contains(&alias.module) {
            found.push(alias.module.clone());
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_match_as_the_kernel_does() {
        assert!(matches("pci:v00008086d0000A7A8sv*sd*bc*sc*i*", "pci:v00008086d0000A7A8sv00001462sd000013B9bc03sc00i00"));
        assert!(!matches("pci:v00008086d0000A7A9sv*", "pci:v00008086d0000A7A8sv00001462"));
        assert!(matches("usb:v1462p16??d*", "usb:v1462p1601d0300"));
        assert!(matches("hid:b0003g*v00001462p00001601", "hid:b0003g0001v00001462p00001601"));
        assert!(matches("x[0-9A-F]y", "xCy") && !matches("x[0-9A-F]y", "xZy"));
        assert!(matches("x[!0-9]y", "xay") && !matches("x[!0-9]y", "x5y"));
        assert!(matches("*", "") && matches("a*", "a") && !matches("a?", "a"));
    }

    #[test]
    fn aliases_are_read_and_looked_up() {
        let text = "# comment\nalias pci:v000010DEd000025A2sv*sd*bc03sc*i* nouveau\nalias pci:v000010DE* nouveau\nalias pci:v00008086d00005A84* i915\nalias usb:v1462* hid_generic\n";
        let aliases = parse(text);
        assert_eq!(aliases.len(), 4);
        assert_eq!(modules_for(&aliases, "pci:v000010DEd000025A2sv00001462sd000013B9bc03sc00i00"), ["nouveau"]);
        assert!(modules_for(&aliases, "pci:v00001234d00000001").is_empty());
    }
}
