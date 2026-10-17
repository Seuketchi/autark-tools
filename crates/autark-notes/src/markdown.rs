//! Live preview: where each Markdown style applies, as character offsets into the note. Syntax
//! markers are hidden except on the line being edited, as in Obsidian.

#[derive(Clone, Debug, PartialEq)]
pub enum Style {
    Heading(u8),
    Bold,
    Italic,
    Code,
    Quote,
    /// `[[target]]`: the visible name.
    Link(String),
    Tag,
    /// Syntax shown dimmed, on the line being edited.
    Marker,
    /// Syntax hidden everywhere else.
    Hidden,
    Bullet,
    /// `[ ]` or `[x]` of a checklist item.
    Checkbox(bool),
    /// The text of a checked item.
    Done,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub style: Style,
}

/// Every styled range in `text`. `editing` is the line holding the cursor, whose markers stay
/// visible.
pub fn spans(text: &str, editing: Option<usize>) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut offset = 0;
    let mut fenced = false;
    for (number, line) in text.split('\n').enumerate() {
        let chars: Vec<char> = line.chars().collect();
        let marker = if editing == Some(number) { Style::Marker } else { Style::Hidden };
        let mut push = |start: usize, end: usize, style: Style| {
            if end > start {
                spans.push(Span { start: offset + start, end: offset + end, style });
            }
        };
        let trimmed = line.trim_start();
        let indent = chars.len() - trimmed.chars().count();
        if trimmed.starts_with("```") {
            fenced = !fenced;
            push(0, chars.len(), Style::Marker);
        } else if fenced {
            push(0, chars.len(), Style::Code);
        } else {
            let mut body = 0;
            let hashes = line.chars().take_while(|&c| c == '#').count();
            if (1..=6).contains(&hashes) && chars.get(hashes) == Some(&' ') {
                push(0, chars.len(), Style::Heading(hashes.min(3) as u8));
                push(0, hashes + 1, marker.clone());
                body = hashes + 1;
            } else if let Some(rest) = trimmed.strip_prefix("- [") {
                let mark = rest.chars().next();
                if matches!(mark, Some(' ' | 'x' | 'X')) && rest.chars().nth(1) == Some(']') {
                    let checked = mark != Some(' ');
                    push(indent, indent + 2, marker.clone());
                    push(indent + 2, indent + 5, Style::Checkbox(checked));
                    if checked {
                        push(indent + 5, chars.len(), Style::Done);
                    }
                    body = indent + 5;
                } else {
                    push(indent, indent + 1, Style::Bullet);
                    body = indent + 1;
                }
            } else if let Some(rest) = trimmed.strip_prefix(['-', '*', '+']) {
                if rest.starts_with(' ') {
                    push(indent, indent + 1, Style::Bullet);
                    body = indent + 1;
                }
            } else if trimmed.starts_with('>') {
                push(0, chars.len(), Style::Quote);
                push(indent, indent + 1, marker.clone());
                body = indent + 1;
            }
            inline(&chars, body, &marker, &mut push);
        }
        offset += chars.len() + 1;
    }
    spans
}

fn inline(chars: &[char], from: usize, marker: &Style, push: &mut impl FnMut(usize, usize, Style)) {
    let find = |start: usize, pattern: &[char]| (start..chars.len()).find(|&i| chars[i..].starts_with(pattern));
    let mut i = from;
    while i < chars.len() {
        let c = chars[i];
        let starts_word = i == 0 || chars[i - 1].is_whitespace();
        if c == '`' && let Some(end) = find(i + 1, &['`']) {
            push(i, i + 1, marker.clone());
            push(i + 1, end, Style::Code);
            push(end, end + 1, marker.clone());
            i = end + 1;
        } else if chars[i..].starts_with(&['[', '[']) && let Some(end) = find(i + 2, &[']', ']']) {
            let inner: String = chars[i + 2..end].iter().collect();
            let target = inner[..inner.find(['#', '|']).unwrap_or(inner.len())].trim().to_owned();
            // `[[target|alias]]` shows only the alias.
            let shown = match inner.find('|') {
                Some(bar) => i + 2 + inner[..bar].chars().count() + 1,
                None => i + 2,
            };
            push(i, shown, marker.clone());
            push(shown, end, Style::Link(target));
            push(end, end + 2, marker.clone());
            i = end + 2;
        } else if chars[i..].starts_with(&['*', '*']) && let Some(end) = find(i + 2, &['*', '*']).filter(|&end| end > i + 2) {
            push(i, i + 2, marker.clone());
            push(i + 2, end, Style::Bold);
            push(end, end + 2, marker.clone());
            i = end + 2;
        } else if (c == '*' || c == '_') && starts_word && let Some(end) = find(i + 1, &[c]).filter(|&end| end > i + 1 && !chars[end - 1].is_whitespace()) {
            push(i, i + 1, marker.clone());
            push(i + 1, end, Style::Italic);
            push(end, end + 1, marker.clone());
            i = end + 1;
        } else if c == '#' && starts_word {
            let len = chars[i + 1..].iter().take_while(|&&c| crate::vault::is_tag_char(c)).count();
            if len > 0 && !chars[i + 1..i + 1 + len].iter().all(char::is_ascii_digit) {
                push(i, i + 1 + len, Style::Tag);
            }
            i += 1 + len;
        } else {
            i += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn styled(text: &str, editing: Option<usize>) -> Vec<(String, Style)> {
        let chars: Vec<char> = text.chars().collect();
        spans(text, editing).into_iter().map(|span| (chars[span.start..span.end].iter().collect(), span.style)).collect()
    }

    #[test]
    fn heading_markers_hide_off_the_editing_line() {
        assert_eq!(styled("## Next", None), [("## Next".into(), Style::Heading(2)), ("## ".into(), Style::Hidden)]);
        assert_eq!(styled("## Next", Some(0))[1], ("## ".into(), Style::Marker));
    }

    #[test]
    fn links_bold_code_tags() {
        let found = styled("See [[kith|Kith]], **now** `x` #autark *a*", None);
        assert!(found.contains(&("Kith".into(), Style::Link("kith".into()))));
        assert!(found.contains(&("[[kith|".into(), Style::Hidden)));
        assert!(found.contains(&("now".into(), Style::Bold)));
        assert!(found.contains(&("x".into(), Style::Code)));
        assert!(found.contains(&("#autark".into(), Style::Tag)));
        assert!(found.contains(&("a".into(), Style::Italic)));
    }

    #[test]
    fn checklists() {
        let found = styled("- [x] Spec it\n- [ ] Build it", None);
        assert!(found.contains(&("[x]".into(), Style::Checkbox(true))));
        assert!(found.contains(&(" Spec it".into(), Style::Done)));
        assert!(found.contains(&("[ ]".into(), Style::Checkbox(false))));
        assert_eq!(spans("- [x] a\n- [ ] b", None).iter().find(|s| s.style == Style::Checkbox(false)).unwrap().start, 10);
    }

    #[test]
    fn fenced_code_is_left_alone() {
        let found = styled("```\n**not bold** [[no]]\n```", None);
        assert_eq!(found[1], ("**not bold** [[no]]".into(), Style::Code));
        assert_eq!(found.len(), 3);
    }
}
