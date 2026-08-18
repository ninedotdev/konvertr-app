//! Text kit: case conversions, line tools, diff, encoding helpers, markdown,
//! a regex tester and deterministic lorem ipsum. All pure functions.

use anyhow::{Result, anyhow};

// Splits on separators and camel boundaries, including acronym runs:
// "parseJSONData" -> ["parse", "JSON", "Data"].
fn split_words(input: &str) -> Vec<String> {
    let chars: Vec<char> = input.chars().collect();
    let mut words = Vec::new();
    let mut cur = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if !c.is_alphanumeric() {
            if !cur.is_empty() {
                words.push(std::mem::take(&mut cur));
            }
            continue;
        }
        if !cur.is_empty() {
            let prev = chars[i - 1];
            let next_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
            let boundary = (prev.is_lowercase() && c.is_uppercase())
                || (prev.is_uppercase() && c.is_uppercase() && next_lower);
            if boundary {
                words.push(std::mem::take(&mut cur));
            }
        }
        cur.push(c);
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words
}

fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase(),
        None => String::new(),
    }
}

pub fn to_camel(input: &str) -> String {
    let words = split_words(input);
    let mut out = String::new();
    for (i, word) in words.iter().enumerate() {
        if i == 0 {
            out.push_str(&word.to_lowercase());
        } else {
            out.push_str(&capitalize(word));
        }
    }
    out
}

pub fn to_pascal(input: &str) -> String {
    split_words(input).iter().map(|w| capitalize(w)).collect()
}

pub fn to_snake(input: &str) -> String {
    split_words(input)
        .iter()
        .map(|w| w.to_lowercase())
        .collect::<Vec<_>>()
        .join("_")
}

pub fn to_kebab(input: &str) -> String {
    split_words(input)
        .iter()
        .map(|w| w.to_lowercase())
        .collect::<Vec<_>>()
        .join("-")
}

pub fn to_constant(input: &str) -> String {
    split_words(input)
        .iter()
        .map(|w| w.to_uppercase())
        .collect::<Vec<_>>()
        .join("_")
}

pub fn to_title(input: &str) -> String {
    split_words(input)
        .iter()
        .map(|w| capitalize(w))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Lowercase URL slug: common accents ASCII-folded (Spanish matters here),
/// anything non-alphanumeric collapsed to single dashes, dashes trimmed.
pub fn slugify(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut prev_dash = true; // suppress a leading dash
    for c in input.to_lowercase().chars() {
        let folded: &str = match c {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => "a",
            'è' | 'é' | 'ê' | 'ë' => "e",
            'ì' | 'í' | 'î' | 'ï' => "i",
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' => "o",
            'ù' | 'ú' | 'û' | 'ü' => "u",
            'ñ' => "n",
            'ç' => "c",
            'ß' => "ss",
            _ => {
                if c.is_ascii_alphanumeric() {
                    out.push(c);
                    prev_dash = false;
                } else if !prev_dash {
                    out.push('-');
                    prev_dash = true;
                }
                continue;
            }
        };
        out.push_str(folded);
        prev_dash = false;
    }
    out.trim_end_matches('-').to_string()
}

// ---------------------------------------------------------------------------
// Line tools.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LineOp {
    Sort,
    SortDesc,
    Dedupe,
    Reverse,
    TrimEach,
    DropEmpty,
    NumberLines,
}

impl LineOp {
    pub const ALL: [LineOp; 7] = [
        LineOp::Sort,
        LineOp::SortDesc,
        LineOp::Dedupe,
        LineOp::Reverse,
        LineOp::TrimEach,
        LineOp::DropEmpty,
        LineOp::NumberLines,
    ];

    pub fn label(self) -> &'static str {
        match self {
            LineOp::Sort => "sort",
            LineOp::SortDesc => "sort desc",
            LineOp::Dedupe => "dedupe",
            LineOp::Reverse => "reverse",
            LineOp::TrimEach => "trim",
            LineOp::DropEmpty => "drop empty",
            LineOp::NumberLines => "number",
        }
    }
}

pub fn apply_lines(input: &str, op: LineOp) -> String {
    let mut lines: Vec<String> = input.lines().map(|l| l.to_string()).collect();
    match op {
        LineOp::Sort => lines.sort(),
        LineOp::SortDesc => {
            lines.sort();
            lines.reverse();
        }
        LineOp::Dedupe => {
            let mut seen = std::collections::HashSet::new();
            lines.retain(|l| seen.insert(l.clone()));
        }
        LineOp::Reverse => lines.reverse(),
        LineOp::TrimEach => {
            for line in &mut lines {
                *line = line.trim().to_string();
            }
        }
        LineOp::DropEmpty => lines.retain(|l| !l.trim().is_empty()),
        LineOp::NumberLines => {
            for (i, line) in lines.iter_mut().enumerate() {
                *line = format!("{}. {line}", i + 1);
            }
        }
    }
    lines.join("\n")
}

// ---------------------------------------------------------------------------
// Diff (line-based, via the `similar` crate).
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DiffKind {
    Equal,
    Insert,
    Delete,
}

#[derive(Clone, Debug)]
pub struct DiffLine {
    pub kind: DiffKind,
    pub text: String,
}

pub fn diff_lines(a: &str, b: &str) -> Vec<DiffLine> {
    similar::TextDiff::from_lines(a, b)
        .iter_all_changes()
        .map(|change| DiffLine {
            kind: match change.tag() {
                similar::ChangeTag::Equal => DiffKind::Equal,
                similar::ChangeTag::Insert => DiffKind::Insert,
                similar::ChangeTag::Delete => DiffKind::Delete,
            },
            text: change.value().trim_end_matches('\n').to_string(),
        })
        .collect()
}

/// (lines added, lines removed)
pub fn diff_stats(lines: &[DiffLine]) -> (usize, usize) {
    let added = lines.iter().filter(|l| l.kind == DiffKind::Insert).count();
    let removed = lines.iter().filter(|l| l.kind == DiffKind::Delete).count();
    (added, removed)
}

// ---------------------------------------------------------------------------
// Encoding helpers.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LineEnding {
    Lf,
    Crlf,
}

pub fn convert_line_endings(input: &str, ending: LineEnding) -> String {
    let normalized = input.replace("\r\n", "\n").replace('\r', "\n");
    match ending {
        LineEnding::Lf => normalized,
        LineEnding::Crlf => normalized.replace('\n', "\r\n"),
    }
}

pub fn strip_bom(input: &str) -> &str {
    input.strip_prefix('\u{feff}').unwrap_or(input)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TextStats {
    pub chars: usize,
    pub chars_no_spaces: usize,
    pub words: usize,
    pub lines: usize,
    /// At 200 words per minute, rounded up.
    pub reading_secs: usize,
}

pub fn count_stats(input: &str) -> TextStats {
    let words = input.split_whitespace().count();
    TextStats {
        chars: input.chars().count(),
        chars_no_spaces: input.chars().filter(|c| !c.is_whitespace()).count(),
        words,
        lines: input.lines().count(),
        reading_secs: (words * 60).div_ceil(200),
    }
}

// ---------------------------------------------------------------------------
// Markdown (tables, strikethrough, footnotes enabled).
// ---------------------------------------------------------------------------

pub fn markdown_to_html(input: &str) -> String {
    let options = pulldown_cmark::Options::ENABLE_TABLES
        | pulldown_cmark::Options::ENABLE_STRIKETHROUGH
        | pulldown_cmark::Options::ENABLE_FOOTNOTES;
    let parser = pulldown_cmark::Parser::new_ext(input, options);
    let mut html = String::with_capacity(input.len() * 2);
    pulldown_cmark::html::push_html(&mut html, parser);
    html
}

// ---------------------------------------------------------------------------
// Regex tester.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct RegexMatch {
    pub start: usize,
    pub end: usize,
    pub text: String,
    /// Capture groups 1.. (None when the group did not participate).
    pub groups: Vec<Option<String>>,
}

/// Flags: any of "i" (case-insensitive), "m" (multi-line ^$), "s" (dot
/// matches newline). Compile errors come back as `Err` with the regex
/// crate's message.
pub fn regex_test(pattern: &str, flags: &str, haystack: &str) -> Result<Vec<RegexMatch>> {
    let re = regex::RegexBuilder::new(pattern)
        .case_insensitive(flags.contains('i'))
        .multi_line(flags.contains('m'))
        .dot_matches_new_line(flags.contains('s'))
        .build()
        .map_err(|e| anyhow!("{e}"))?;
    Ok(re
        .captures_iter(haystack)
        .map(|caps| {
            let whole = caps.get(0).expect("group 0 always participates");
            RegexMatch {
                start: whole.start(),
                end: whole.end(),
                text: whole.as_str().to_string(),
                groups: (1..caps.len())
                    .map(|g| caps.get(g).map(|m| m.as_str().to_string()))
                    .collect(),
            }
        })
        .collect())
}

// ---------------------------------------------------------------------------
// Lorem ipsum — deterministic (fixed word list cycled, no rng).
// ---------------------------------------------------------------------------

const LOREM_WORDS: [&str; 30] = [
    "lorem",
    "ipsum",
    "dolor",
    "sit",
    "amet",
    "consectetur",
    "adipiscing",
    "elit",
    "sed",
    "do",
    "eiusmod",
    "tempor",
    "incididunt",
    "ut",
    "labore",
    "et",
    "dolore",
    "magna",
    "aliqua",
    "enim",
    "ad",
    "minim",
    "veniam",
    "quis",
    "nostrud",
    "exercitation",
    "ullamco",
    "laboris",
    "nisi",
    "aliquip",
];

pub fn lorem(paragraphs: usize, words_per: usize) -> String {
    let mut word_ix = 0;
    let mut out = Vec::with_capacity(paragraphs);
    for _ in 0..paragraphs {
        let mut para = String::new();
        for w in 0..words_per {
            let word = LOREM_WORDS[word_ix % LOREM_WORDS.len()];
            word_ix += 1;
            if w == 0 {
                para.push_str(&capitalize(word));
            } else {
                para.push(' ');
                para.push_str(word);
            }
        }
        if !para.is_empty() {
            para.push('.');
        }
        out.push(para);
    }
    out.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_conversions_split_words_correctly() {
        for input in ["helloWorld", "hello_world", "Hello World", "hello-world"] {
            assert_eq!(to_snake(input), "hello_world", "for {input:?}");
            assert_eq!(to_camel(input), "helloWorld", "for {input:?}");
            assert_eq!(to_pascal(input), "HelloWorld", "for {input:?}");
            assert_eq!(to_kebab(input), "hello-world", "for {input:?}");
            assert_eq!(to_constant(input), "HELLO_WORLD", "for {input:?}");
            assert_eq!(to_title(input), "Hello World", "for {input:?}");
        }
    }

    #[test]
    fn case_conversions_handle_acronyms() {
        assert_eq!(to_snake("parseJSONData"), "parse_json_data");
        assert_eq!(to_camel("parse_JSON_data"), "parseJsonData");
        assert_eq!(to_pascal("XMLHttpRequest"), "XmlHttpRequest");
        assert_eq!(to_kebab("innerHTML"), "inner-html");
    }

    #[test]
    fn slugify_folds_accents_and_collapses() {
        assert_eq!(slugify("¡El Niño & la Señora!"), "el-nino-la-senora");
        assert_eq!(slugify("  Café con Leche  "), "cafe-con-leche");
        assert_eq!(slugify("--a---b--"), "a-b");
        assert_eq!(slugify("über straße"), "uber-strasse");
    }

    #[test]
    fn line_ops() {
        let input = "b\n\n  a  \nb\nc";
        assert_eq!(apply_lines(input, LineOp::Sort), "\n  a  \nb\nb\nc");
        assert_eq!(apply_lines(input, LineOp::SortDesc), "c\nb\nb\n  a  \n");
        assert_eq!(apply_lines(input, LineOp::Dedupe), "b\n\n  a  \nc");
        assert_eq!(apply_lines(input, LineOp::Reverse), "c\nb\n  a  \n\nb");
        assert_eq!(apply_lines(input, LineOp::TrimEach), "b\n\na\nb\nc");
        assert_eq!(apply_lines(input, LineOp::DropEmpty), "b\n  a  \nb\nc");
        assert_eq!(apply_lines("a\nb", LineOp::NumberLines), "1. a\n2. b");
    }

    #[test]
    fn diff_reports_changes_and_stats() {
        let lines = diff_lines("a\nb\nc\n", "a\nx\nc\nd\n");
        let kinds: Vec<DiffKind> = lines.iter().map(|l| l.kind).collect();
        assert_eq!(
            kinds,
            [
                DiffKind::Equal,
                DiffKind::Delete,
                DiffKind::Insert,
                DiffKind::Equal,
                DiffKind::Insert
            ]
        );
        assert_eq!(diff_stats(&lines), (2, 1));
        assert_eq!(lines[1].text, "b");
        assert_eq!(lines[2].text, "x");
    }

    #[test]
    fn encodings_and_stats() {
        assert_eq!(
            convert_line_endings("a\r\nb\rc\n", LineEnding::Lf),
            "a\nb\nc\n"
        );
        assert_eq!(convert_line_endings("a\nb", LineEnding::Crlf), "a\r\nb");
        assert_eq!(strip_bom("\u{feff}hi"), "hi");
        assert_eq!(strip_bom("hi"), "hi");

        let stats = count_stats("hola mundo\ncruel  mundo");
        assert_eq!(stats.words, 4);
        assert_eq!(stats.lines, 2);
        assert_eq!(stats.chars, 23);
        assert_eq!(stats.chars_no_spaces, 19);
        assert_eq!(stats.reading_secs, 2); // ceil(4 * 60 / 200)
        assert_eq!(count_stats("").words, 0);
    }

    #[test]
    fn markdown_renders_tables_and_strikethrough() {
        let html = markdown_to_html("# Hi\n\n~~gone~~\n\n| a | b |\n|---|---|\n| 1 | 2 |\n");
        assert!(html.contains("<h1>Hi</h1>"));
        assert!(html.contains("<del>gone</del>"));
        assert!(html.contains("<table>"));
    }

    #[test]
    fn regex_matches_flags_and_groups() {
        let matches = regex_test(r"(\w+)@(\w+)\.com", "", "a@b.com c@d.com").unwrap();
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].text, "a@b.com");
        assert_eq!(matches[0].start, 0);
        assert_eq!(matches[1].groups, vec![Some("c".into()), Some("d".into())]);

        assert_eq!(regex_test("HOLA", "", "hola").unwrap().len(), 0);
        assert_eq!(regex_test("HOLA", "i", "hola").unwrap().len(), 1);
        assert_eq!(regex_test("^b$", "m", "a\nb").unwrap().len(), 1);
        assert_eq!(regex_test("a.b", "s", "a\nb").unwrap().len(), 1);
        assert_eq!(
            regex_test("(a)|(b)", "", "b").unwrap()[0].groups,
            vec![None, Some("b".into())]
        );
        assert!(regex_test("(", "", "x").is_err());
    }

    #[test]
    fn lorem_is_deterministic() {
        let a = lorem(2, 5);
        assert_eq!(a, lorem(2, 5));
        assert_eq!(
            a,
            "Lorem ipsum dolor sit amet.\n\nConsectetur adipiscing elit sed do."
        );
        assert_eq!(lorem(0, 10), "");
    }
}
