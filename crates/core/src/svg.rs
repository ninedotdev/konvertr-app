//! Conservative hand-rolled SVG optimizer: strips comments, metadata, editor
//! namespaces and data-name attrs, collapses whitespace, rounds coordinates to
//! 3 decimals. Content inside <text> is never touched.

use anyhow::{Context as _, Result};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct SvgOutcome {
    pub out_path: PathBuf,
    pub in_size: u64,
    pub out_size: u64,
}

/// Optimize `input` and write a sibling `<stem>-optimized.svg`. Never
/// overwrites (appends `-2`, `-3`, ... if needed).
pub fn optimize_file(input: &Path) -> Result<SvgOutcome> {
    let text =
        std::fs::read_to_string(input).with_context(|| format!("reading {}", input.display()))?;
    let optimized = optimize(&text);

    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("optimized");
    let sibling = |name: String| input.with_file_name(name);
    let mut out_path = sibling(format!("{stem}-optimized.svg"));
    let mut n = 2;
    while out_path.exists() {
        out_path = sibling(format!("{stem}-optimized-{n}.svg"));
        n += 1;
    }
    std::fs::write(&out_path, &optimized)
        .with_context(|| format!("writing {}", out_path.display()))?;
    Ok(SvgOutcome {
        out_path,
        in_size: text.len() as u64,
        out_size: optimized.len() as u64,
    })
}

enum Token<'a> {
    /// Character data between tags.
    Text(&'a str),
    /// `<!-- ... -->`
    Comment,
    /// `<![CDATA[...]]>`, `<!DOCTYPE ...>`, `<?xml ...?>` — passed through.
    Verbatim(&'a str),
    /// A full `<...>` element tag.
    Tag(&'a str),
}

fn lex(input: &str) -> Vec<Token<'_>> {
    let mut tokens = Vec::new();
    let bytes = input.as_bytes();
    let mut pos = 0;
    while pos < bytes.len() {
        match input[pos..].find('<') {
            None => {
                tokens.push(Token::Text(&input[pos..]));
                break;
            }
            Some(rel) => {
                if rel > 0 {
                    tokens.push(Token::Text(&input[pos..pos + rel]));
                }
                let start = pos + rel;
                let rest = &input[start..];
                let (token, len) = if rest.starts_with("<!--") {
                    let len = rest.find("-->").map(|i| i + 3).unwrap_or(rest.len());
                    (Token::Comment, len)
                } else if rest.starts_with("<![CDATA[") {
                    let len = rest.find("]]>").map(|i| i + 3).unwrap_or(rest.len());
                    (Token::Verbatim(&rest[..len]), len)
                } else if rest.starts_with("<!") || rest.starts_with("<?") {
                    let len = rest.find('>').map(|i| i + 1).unwrap_or(rest.len());
                    (Token::Verbatim(&rest[..len]), len)
                } else {
                    let len = tag_end(rest);
                    (Token::Tag(&rest[..len]), len)
                };
                tokens.push(token);
                pos = start + len;
            }
        }
    }
    tokens
}

/// Length of a tag starting at `<`, honoring quoted attribute values.
fn tag_end(s: &str) -> usize {
    let mut quote: Option<char> = None;
    for (i, c) in s.char_indices() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None => match c {
                '"' | '\'' => quote = Some(c),
                '>' => return i + 1,
                _ => {}
            },
        }
    }
    s.len()
}

/// Element name of a tag like `<name ...>` / `</name>` / `<name/>`.
fn tag_name(tag: &str) -> &str {
    let inner = tag.trim_start_matches('<').trim_start_matches('/');
    let end = inner
        .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
        .unwrap_or(inner.len());
    &inner[..end]
}

fn is_stripped_element(name: &str) -> bool {
    matches!(name, "metadata" | "title" | "desc")
        || name.starts_with("sodipodi:")
        || name.starts_with("inkscape:")
}

fn is_stripped_attr(name: &str) -> bool {
    name == "data-name"
        || name == "xmlns:sodipodi"
        || name == "xmlns:inkscape"
        || name.starts_with("sodipodi:")
        || name.starts_with("inkscape:")
}

fn is_numeric_attr(name: &str) -> bool {
    matches!(
        name,
        "d" | "points"
            | "x"
            | "y"
            | "x1"
            | "y1"
            | "x2"
            | "y2"
            | "cx"
            | "cy"
            | "r"
            | "rx"
            | "ry"
            | "dx"
            | "dy"
            | "width"
            | "height"
    )
}

/// Optimize an SVG document. Conservative: well-formed input stays well-formed,
/// character data inside <text> is preserved verbatim.
pub fn optimize(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    // (name, depth) of an element subtree being dropped entirely.
    let mut skipping: Option<(String, u32)> = None;
    let mut text_depth: u32 = 0;

    for token in lex(input) {
        if let Some((name, depth)) = &mut skipping {
            if let Token::Tag(tag) = token {
                let is_close = tag.starts_with("</");
                let self_closing = tag.ends_with("/>");
                if tag_name(tag) == name.as_str() {
                    if is_close {
                        *depth -= 1;
                        if *depth == 0 {
                            skipping = None;
                        }
                    } else if !self_closing {
                        *depth += 1;
                    }
                }
            }
            continue;
        }
        match token {
            Token::Comment => {}
            Token::Verbatim(v) => out.push_str(v),
            Token::Text(t) => {
                if text_depth > 0 {
                    out.push_str(t);
                } else if !t.trim().is_empty() {
                    // Mixed content outside <text>: collapse whitespace runs.
                    let mut last_ws = false;
                    for c in t.chars() {
                        if c.is_whitespace() {
                            if !last_ws {
                                out.push(' ');
                            }
                            last_ws = true;
                        } else {
                            out.push(c);
                            last_ws = false;
                        }
                    }
                }
            }
            Token::Tag(tag) => {
                let name = tag_name(tag);
                let is_close = tag.starts_with("</");
                let self_closing = tag.ends_with("/>");
                if !is_close && is_stripped_element(name) {
                    if !self_closing {
                        skipping = Some((name.to_string(), 1));
                    }
                    continue;
                }
                if name == "text" {
                    if is_close {
                        text_depth = text_depth.saturating_sub(1);
                    } else if !self_closing {
                        text_depth += 1;
                    }
                }
                if is_close {
                    out.push_str("</");
                    out.push_str(name);
                    out.push('>');
                } else {
                    out.push_str(&rebuild_tag(tag, name, self_closing));
                }
            }
        }
    }
    out.trim().to_string()
}

/// Re-emit an opening tag with stripped/rounded attributes and normalized
/// whitespace. Anything unparseable is copied through untouched.
fn rebuild_tag(tag: &str, name: &str, self_closing: bool) -> String {
    let inner = &tag[1 + name.len()..tag.len() - if self_closing { 2 } else { 1 }];
    let mut out = String::with_capacity(tag.len());
    out.push('<');
    out.push_str(name);

    let mut rest = inner.trim();
    while !rest.is_empty() {
        // Attribute name.
        let name_end = rest
            .find(|c: char| c.is_whitespace() || c == '=')
            .unwrap_or(rest.len());
        let attr_name = &rest[..name_end];
        rest = rest[name_end..].trim_start();
        // Optional ="value".
        let mut value: Option<&str> = None;
        let mut quote_char = '"';
        if let Some(after_eq) = rest.strip_prefix('=') {
            let after_eq = after_eq.trim_start();
            let mut chars = after_eq.chars();
            match chars.next() {
                Some(q @ ('"' | '\'')) => {
                    quote_char = q;
                    match after_eq[1..].find(q) {
                        Some(end) => {
                            value = Some(&after_eq[1..1 + end]);
                            rest = after_eq[1 + end + 1..].trim_start();
                        }
                        None => {
                            // Unterminated quote: bail out, emit original tag.
                            return tag.to_string();
                        }
                    }
                }
                _ => {
                    // Unquoted value: not valid XML, pass tag through.
                    return tag.to_string();
                }
            }
        }
        if attr_name.is_empty() {
            return tag.to_string();
        }
        if !is_stripped_attr(attr_name) {
            out.push(' ');
            out.push_str(attr_name);
            if let Some(v) = value {
                out.push('=');
                out.push(quote_char);
                if is_numeric_attr(attr_name) {
                    out.push_str(&round_numbers(v));
                } else {
                    out.push_str(v);
                }
                out.push(quote_char);
            }
        }
    }

    if self_closing {
        out.push('/');
    }
    out.push('>');
    out
}

/// Round every plain decimal number in `s` to 3 decimals. Numbers in
/// scientific notation are left untouched.
fn round_numbers(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        let starts_number = c.is_ascii_digit()
            || ((c == '-' || c == '+' || c == '.')
                && bytes
                    .get(i + 1)
                    .is_some_and(|&b| (b as char).is_ascii_digit() || b == b'.'));
        if !starts_number {
            out.push(c);
            i += 1;
            continue;
        }
        let start = i;
        if c == '-' || c == '+' {
            i += 1;
        }
        let mut seen_dot = false;
        while i < bytes.len() {
            let b = bytes[i] as char;
            if b.is_ascii_digit() {
                i += 1;
            } else if b == '.' && !seen_dot {
                seen_dot = true;
                i += 1;
            } else {
                break;
            }
        }
        let token = &s[start..i];
        // Scientific notation: keep the whole thing untouched.
        if bytes.get(i).is_some_and(|&b| b == b'e' || b == b'E') {
            let mut j = i + 1;
            if bytes.get(j).is_some_and(|&b| b == b'-' || b == b'+') {
                j += 1;
            }
            while j < bytes.len() && (bytes[j] as char).is_ascii_digit() {
                j += 1;
            }
            out.push_str(&s[start..j]);
            i = j;
            continue;
        }
        match token.parse::<f64>() {
            Ok(v) => out.push_str(&format_rounded(v)),
            Err(_) => out.push_str(token),
        }
    }
    out
}

fn format_rounded(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".to_string()
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_comments_and_metadata() {
        let input = concat!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\">\n",
            "  <!-- a comment -->\n",
            "  <title>My Icon</title>\n",
            "  <desc>Long description</desc>\n",
            "  <metadata><rdf:RDF>stuff</rdf:RDF></metadata>\n",
            "  <rect width=\"10\" height=\"10\"/>\n",
            "</svg>"
        );
        let out = optimize(input);
        assert!(!out.contains("comment"));
        assert!(!out.contains("<title>"));
        assert!(!out.contains("<desc>"));
        assert!(!out.contains("<metadata>"));
        assert!(out.contains("<rect width=\"10\" height=\"10\"/>"));
    }

    #[test]
    fn strips_editor_namespaces_and_data_name() {
        let input = concat!(
            "<svg xmlns:inkscape=\"http://www.inkscape.org/ns\" ",
            "xmlns:sodipodi=\"http://sodipodi.sf.net/DTD\" ",
            "inkscape:version=\"1.2\" data-name=\"Layer 1\">",
            "<sodipodi:namedview inkscape:zoom=\"2\"/>",
            "<g sodipodi:role=\"main\" fill=\"red\"/>",
            "</svg>"
        );
        let out = optimize(input);
        assert!(!out.contains("inkscape"));
        assert!(!out.contains("sodipodi"));
        assert!(!out.contains("data-name"));
        assert!(out.contains("<g fill=\"red\"/>"));
    }

    #[test]
    fn rounds_path_and_coordinate_numbers() {
        let input = r#"<svg><path d="M 1.234567 2.99999 L 10.00001 -0.000004"/><circle cx="5.55555" cy="3" r="1.5"/></svg>"#;
        let out = optimize(input);
        assert!(out.contains(r#"d="M 1.235 3 L 10 0""#), "{out}");
        assert!(out.contains(r#"cx="5.556""#), "{out}");
        assert!(out.contains(r#"cy="3""#), "{out}");
        assert!(out.contains(r#"r="1.5""#), "{out}");
    }

    #[test]
    fn text_content_is_untouched() {
        let input = "<svg><text x=\"1.23456\">  Hello   world 3.14159  </text></svg>";
        let out = optimize(input);
        assert!(out.contains(">  Hello   world 3.14159  <"), "{out}");
        // The <text> element's own coordinate attr is still rounded.
        assert!(out.contains("x=\"1.235\""), "{out}");
    }

    #[test]
    fn collapses_whitespace_and_saves_bytes() {
        let input = concat!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\">\n\n",
            "    <!-- big comment explaining nothing at all -->\n",
            "    <g>\n        <rect x=\"1.000000\" width=\"5\" height=\"5\"/>\n    </g>\n",
            "</svg>\n"
        );
        let out = optimize(input);
        assert!(out.len() < input.len());
        assert!(out.starts_with("<svg"));
        assert!(out.ends_with("</svg>"));
        assert!(out.contains("x=\"1\""));
        assert!(!out.contains('\n'));
    }

    #[test]
    fn style_and_fill_values_untouched() {
        let input = r##"<svg><rect fill="#1.5abc" style="stroke-width:1.23456"/></svg>"##;
        let out = optimize(input);
        assert!(out.contains("stroke-width:1.23456"));
        assert!(out.contains("#1.5abc"));
    }

    #[test]
    fn optimize_file_writes_sibling() {
        let dir = std::env::temp_dir().join(format!(
            "konvrt-svg-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("icon.svg");
        std::fs::write(&input, "<svg>\n  <!-- x -->\n  <rect/>\n</svg>").unwrap();
        let outcome = optimize_file(&input).unwrap();
        assert_eq!(outcome.out_path, dir.join("icon-optimized.svg"));
        assert!(outcome.out_size < outcome.in_size);
        let second = optimize_file(&input).unwrap();
        assert_eq!(second.out_path, dir.join("icon-optimized-2.svg"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
