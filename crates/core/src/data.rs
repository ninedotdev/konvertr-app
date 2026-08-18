//! Data converter: JSON / CSV / YAML / TOML in any direction.
//!
//! CSV rows map to an array of objects, dot-notation headers unflatten into
//! nested objects, and values get number/bool/null inference. TOML must be a
//! table at the root, so non-object roots are wrapped under a `rows` key.

use anyhow::{Context as _, Result, bail};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DataFormat {
    Json,
    Csv,
    Yaml,
    Toml,
}

impl DataFormat {
    pub const ALL: [DataFormat; 4] = [
        DataFormat::Json,
        DataFormat::Csv,
        DataFormat::Yaml,
        DataFormat::Toml,
    ];

    pub fn label(self) -> &'static str {
        match self {
            DataFormat::Json => "JSON",
            DataFormat::Csv => "CSV",
            DataFormat::Yaml => "YAML",
            DataFormat::Toml => "TOML",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            DataFormat::Json => "json",
            DataFormat::Csv => "csv",
            DataFormat::Yaml => "yaml",
            DataFormat::Toml => "toml",
        }
    }

    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext.to_ascii_lowercase().as_str() {
            "json" => Some(DataFormat::Json),
            "csv" => Some(DataFormat::Csv),
            "yaml" | "yml" => Some(DataFormat::Yaml),
            "toml" => Some(DataFormat::Toml),
            _ => None,
        }
    }
}

pub fn is_supported_input(path: &Path) -> bool {
    detect_format(path).is_some()
}

pub fn detect_format(path: &Path) -> Option<DataFormat> {
    path.extension()
        .and_then(|e| e.to_str())
        .and_then(DataFormat::from_extension)
}

pub fn convert(input: &str, from: DataFormat, to: DataFormat) -> Result<String> {
    if input.trim().is_empty() {
        bail!("input is empty");
    }
    if from == to {
        return Ok(input.to_string());
    }
    let value = parse(input, from)?;
    write(&value, to)
}

fn parse(input: &str, format: DataFormat) -> Result<Value> {
    match format {
        DataFormat::Json => serde_json::from_str(input).context("invalid JSON input"),
        DataFormat::Yaml => serde_yaml::from_str(input).context("invalid YAML input"),
        DataFormat::Csv => csv_to_value(input),
        DataFormat::Toml => {
            let value: toml::Value = toml::from_str(input).context("invalid TOML input")?;
            Ok(toml_to_json(value))
        }
    }
}

fn write(value: &Value, format: DataFormat) -> Result<String> {
    match format {
        DataFormat::Json => serde_json::to_string_pretty(value).context("writing JSON"),
        DataFormat::Yaml => serde_yaml::to_string(value).context("writing YAML"),
        DataFormat::Csv => value_to_csv(value),
        DataFormat::Toml => {
            // A TOML document must be a table at the root; wrap anything else
            // (e.g. CSV's array of rows) under a top-level `rows` key.
            let root = match value {
                Value::Object(_) => value.clone(),
                other => {
                    let mut map = Map::new();
                    map.insert("rows".to_string(), other.clone());
                    Value::Object(map)
                }
            };
            toml::to_string_pretty(&json_to_toml(&root)?).context("writing TOML")
        }
    }
}

fn toml_to_json(value: toml::Value) -> Value {
    match value {
        toml::Value::String(s) => Value::String(s),
        toml::Value::Integer(i) => Value::Number(i.into()),
        toml::Value::Float(f) => serde_json::Number::from_f64(f)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        toml::Value::Boolean(b) => Value::Bool(b),
        toml::Value::Datetime(d) => Value::String(d.to_string()),
        toml::Value::Array(items) => Value::Array(items.into_iter().map(toml_to_json).collect()),
        toml::Value::Table(table) => Value::Object(
            table
                .into_iter()
                .map(|(k, v)| (k, toml_to_json(v)))
                .collect(),
        ),
    }
}

fn json_to_toml(value: &Value) -> Result<toml::Value> {
    Ok(match value {
        // TOML has no null; map keys with null values are dropped by the
        // Object arm below, so a null here sits inside an array.
        Value::Null => bail!("TOML cannot represent null array elements"),
        Value::Bool(b) => toml::Value::Boolean(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                toml::Value::Integer(i)
            } else {
                toml::Value::Float(n.as_f64().context("number not representable in TOML")?)
            }
        }
        Value::String(s) => toml::Value::String(s.clone()),
        Value::Array(items) => {
            toml::Value::Array(items.iter().map(json_to_toml).collect::<Result<_>>()?)
        }
        Value::Object(map) => toml::Value::Table(
            map.iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| Ok((k.clone(), json_to_toml(v)?)))
                .collect::<Result<toml::Table>>()?,
        ),
    })
}

#[derive(Clone, Debug)]
pub struct DataOutcome {
    pub out_path: PathBuf,
    pub in_size: u64,
    pub out_size: u64,
}

/// Read `input`, detect its format by extension, convert, and write the result
/// next to it. Never overwrites.
pub fn convert_file(input: &Path, to: DataFormat) -> Result<DataOutcome> {
    let from = detect_format(input)
        .with_context(|| format!("unsupported input format: {}", input.display()))?;
    let text =
        std::fs::read_to_string(input).with_context(|| format!("reading {}", input.display()))?;
    let out = convert(&text, from, to)?;
    let out_path = output_path(input, to.extension());
    std::fs::write(&out_path, &out).with_context(|| format!("writing {}", out_path.display()))?;
    Ok(DataOutcome {
        out_path,
        in_size: text.len() as u64,
        out_size: out.len() as u64,
    })
}

/// Sibling path for the converted file; never collides with the input or an
/// existing file (appends `-konverted`, then `-2`, `-3`, ...). Mirrors
/// `output_path` in the crate root, which is tied to image formats.
fn output_path(input: &Path, ext: &str) -> PathBuf {
    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("converted");
    let sibling = |name: String| input.with_file_name(name);

    let mut candidate = sibling(format!("{stem}.{ext}"));
    if candidate == input {
        candidate = sibling(format!("{stem}-konverted.{ext}"));
    }
    let base = candidate
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(stem)
        .to_string();
    let mut n = 2;
    while candidate.exists() {
        candidate = sibling(format!("{base}-{n}.{ext}"));
        n += 1;
    }
    candidate
}

// ---------------------------------------------------------------------------
// CSV <-> Value
// ---------------------------------------------------------------------------

fn csv_to_value(input: &str) -> Result<Value> {
    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .from_reader(input.as_bytes());
    let headers: Vec<String> = reader
        .headers()
        .context("invalid CSV input")?
        .iter()
        .map(|h| h.trim().to_string())
        .collect();
    if headers.iter().all(|h| h.is_empty()) {
        bail!("CSV has no data");
    }
    let has_dot_keys = headers.iter().any(|h| h.contains('.'));

    let mut rows = Vec::new();
    for record in reader.records() {
        let record = record.context("invalid CSV input")?;
        if record.iter().all(|f| f.trim().is_empty()) {
            continue;
        }
        let mut flat = Map::new();
        for (i, header) in headers.iter().enumerate() {
            let raw = record.get(i).unwrap_or("");
            flat.insert(header.clone(), infer_csv_value(raw));
        }
        let row = if has_dot_keys { unflatten(&flat) } else { flat };
        rows.push(Value::Object(row));
    }
    Ok(Value::Array(rows))
}

/// Type inference on a CSV field: number / bool / null, else string.
fn infer_csv_value(raw: &str) -> Value {
    let trimmed = raw.trim();
    match trimmed {
        "" => Value::String(String::new()),
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        "null" => Value::Null,
        _ => {
            if let Ok(n) = trimmed.parse::<i64>() {
                Value::Number(n.into())
            } else if let Some(n) = trimmed
                .parse::<f64>()
                .ok()
                .filter(|n| n.is_finite())
                .and_then(serde_json::Number::from_f64)
            {
                Value::Number(n)
            } else {
                Value::String(trimmed.to_string())
            }
        }
    }
}

fn value_to_csv(value: &Value) -> Result<String> {
    // Wrap a single object in an array, like the web version.
    let rows: Vec<&Value> = match value {
        Value::Array(items) => items.iter().collect(),
        other => vec![other],
    };
    if rows.is_empty() {
        return Ok(String::new());
    }

    let flat_rows: Vec<Map<String, Value>> = rows
        .iter()
        .map(|row| match row {
            Value::Object(map) => Ok(flatten(map, "")),
            _ => bail!("CSV conversion requires an array of objects"),
        })
        .collect::<Result<_>>()?;

    let mut headers: Vec<String> = Vec::new();
    for row in &flat_rows {
        for key in row.keys() {
            if !headers.iter().any(|h| h == key) {
                headers.push(key.clone());
            }
        }
    }

    let mut writer = csv::WriterBuilder::new().from_writer(Vec::new());
    writer.write_record(&headers).context("writing CSV")?;
    for row in &flat_rows {
        let record: Vec<String> = headers
            .iter()
            .map(|h| row.get(h).map(csv_field).unwrap_or_default())
            .collect();
        writer.write_record(&record).context("writing CSV")?;
    }
    let bytes = writer.into_inner().context("writing CSV")?;
    let mut out = String::from_utf8(bytes).context("writing CSV")?;
    // The csv crate terminates every record; the web version joins with \n.
    if out.ends_with('\n') {
        out.pop();
    }
    Ok(out)
}

fn csv_field(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        // Arrays stay leaf values when flattening; store them as JSON text.
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

/// Flatten nested objects into dot-notation keys. Arrays are leaf values.
fn flatten(map: &Map<String, Value>, prefix: &str) -> Map<String, Value> {
    let mut result = Map::new();
    for (key, value) in map {
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        match value {
            Value::Object(inner) => result.extend(flatten(inner, &path)),
            other => {
                result.insert(path, other.clone());
            }
        }
    }
    result
}

/// Rebuild nested objects from dot-notation keys.
fn unflatten(flat: &Map<String, Value>) -> Map<String, Value> {
    let mut result = Map::new();
    for (key, value) in flat {
        let mut parts = key.split('.').peekable();
        let mut current = &mut result;
        while let Some(part) = parts.next() {
            if parts.peek().is_none() {
                current.insert(part.to_string(), value.clone());
            } else {
                let entry = current
                    .entry(part.to_string())
                    .or_insert_with(|| Value::Object(Map::new()));
                if !entry.is_object() {
                    *entry = Value::Object(Map::new());
                }
                current = entry.as_object_mut().unwrap();
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_to_csv_flattens_nested_keys() {
        let json = r#"[
            {"name": "ada", "meta": {"age": 36, "tags": "math"}},
            {"name": "alan", "meta": {"age": 41, "tags": "code"}}
        ]"#;
        let csv = convert(json, DataFormat::Json, DataFormat::Csv).unwrap();
        // serde_json's preserve_order feature keeps source key order.
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines[0], "name,meta.age,meta.tags");
        assert_eq!(lines[1], "ada,36,math");
        assert_eq!(lines[2], "alan,41,code");
    }

    #[test]
    fn json_single_object_wraps_in_one_row() {
        let csv = convert(r#"{"a": 1, "b": "x"}"#, DataFormat::Json, DataFormat::Csv).unwrap();
        assert_eq!(csv, "a,b\n1,x");
    }

    #[test]
    fn csv_to_json_infers_types_and_unflattens() {
        let csv = "name,meta.age,active,note\nada,36,true,null\nalan,41.5,false,hi";
        let json = convert(csv, DataFormat::Csv, DataFormat::Json).unwrap();
        let value: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value[0]["name"], "ada");
        assert_eq!(value[0]["meta"]["age"], 36);
        assert_eq!(value[0]["active"], Value::Bool(true));
        assert_eq!(value[0]["note"], Value::Null);
        assert_eq!(value[1]["meta"]["age"], 41.5);
        assert_eq!(value[1]["note"], "hi");
    }

    #[test]
    fn csv_quoted_commas_and_escaped_quotes() {
        let csv = "name,quote\nada,\"hello, world\"\nalan,\"say \"\"hi\"\"\"";
        let json = convert(csv, DataFormat::Csv, DataFormat::Json).unwrap();
        let value: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value[0]["quote"], "hello, world");
        assert_eq!(value[1]["quote"], "say \"hi\"");

        // And back out: fields with commas/quotes get quoted again.
        let back = convert(&json, DataFormat::Json, DataFormat::Csv).unwrap();
        assert!(back.contains("\"hello, world\""));
        assert!(back.contains("\"say \"\"hi\"\"\""));
    }

    #[test]
    fn yaml_round_trips_through_json() {
        let yaml = "name: ada\nage: 36\nlangs:\n  - rust\n  - lisp\n";
        let json = convert(yaml, DataFormat::Yaml, DataFormat::Json).unwrap();
        let value: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["name"], "ada");
        assert_eq!(value["langs"][1], "lisp");

        let yaml2 = convert(&json, DataFormat::Json, DataFormat::Yaml).unwrap();
        let json2 = convert(&yaml2, DataFormat::Yaml, DataFormat::Json).unwrap();
        assert_eq!(json, json2);
    }

    #[test]
    fn csv_to_yaml_and_back() {
        let csv = "a,b\n1,two";
        let yaml = convert(csv, DataFormat::Csv, DataFormat::Yaml).unwrap();
        let back = convert(&yaml, DataFormat::Yaml, DataFormat::Csv).unwrap();
        assert_eq!(back, "a,b\n1,two");
    }

    #[test]
    fn same_format_returns_input_unchanged() {
        let input = "{ \"weird\":   1 }";
        assert_eq!(
            convert(input, DataFormat::Json, DataFormat::Json).unwrap(),
            input
        );
    }

    #[test]
    fn empty_and_invalid_inputs_error() {
        assert!(convert("  \n ", DataFormat::Json, DataFormat::Csv).is_err());
        assert!(convert("not json", DataFormat::Json, DataFormat::Yaml).is_err());
        assert!(
            convert("[1, 2, 3]", DataFormat::Json, DataFormat::Csv).is_err(),
            "scalars cannot become CSV rows"
        );
    }

    #[test]
    fn json_to_toml_and_back() {
        let json = r#"{"name": "konvrt", "version": 1, "deps": {"gpui": "git", "serde": "1"}, "tags": ["rust", "gpui"]}"#;
        let toml_out = convert(json, DataFormat::Json, DataFormat::Toml).unwrap();
        assert!(toml_out.contains("name = \"konvrt\""));
        assert!(toml_out.contains("[deps]"));
        let back = convert(&toml_out, DataFormat::Toml, DataFormat::Json).unwrap();
        let value: Value = serde_json::from_str(&back).unwrap();
        assert_eq!(value["deps"]["gpui"], "git");
        assert_eq!(value["version"], 1);
        assert_eq!(value["tags"][1], "gpui");
    }

    #[test]
    fn csv_to_toml_wraps_rows_at_root() {
        // TOML documents must be a table at the root, so the CSV row array
        // lands under a top-level `rows` key (as arrays of tables).
        let csv = "name,age\nada,36\nalan,41";
        let toml_out = convert(csv, DataFormat::Csv, DataFormat::Toml).unwrap();
        assert!(toml_out.contains("[[rows]]"), "got:\n{toml_out}");
        let back = convert(&toml_out, DataFormat::Toml, DataFormat::Json).unwrap();
        let value: Value = serde_json::from_str(&back).unwrap();
        assert_eq!(value["rows"][0]["name"], "ada");
        assert_eq!(value["rows"][1]["age"], 41);
    }

    #[test]
    fn toml_to_yaml_and_csv_directions() {
        let toml_in = "title = \"test\"\n\n[owner]\nname = \"ada\"\n";
        let yaml = convert(toml_in, DataFormat::Toml, DataFormat::Yaml).unwrap();
        assert!(yaml.contains("title: test"));
        // TOML root table goes through the same flatten path as a JSON object.
        // (The toml crate's table iterates alphabetically, so `owner` first.)
        let csv = convert(toml_in, DataFormat::Toml, DataFormat::Csv).unwrap();
        assert_eq!(csv, "owner.name,title\nada,test");
    }

    #[test]
    fn toml_drops_null_map_values_and_stringifies_datetimes() {
        let json = r#"{"a": 1, "gone": null}"#;
        let toml_out = convert(json, DataFormat::Json, DataFormat::Toml).unwrap();
        assert!(toml_out.contains("a = 1"));
        assert!(!toml_out.contains("gone"));
        // Null inside an array cannot be represented.
        assert!(convert(r#"{"xs": [1, null]}"#, DataFormat::Json, DataFormat::Toml).is_err());

        let back = convert(
            "date = 2024-02-29T12:00:00Z\n",
            DataFormat::Toml,
            DataFormat::Json,
        )
        .unwrap();
        let value: Value = serde_json::from_str(&back).unwrap();
        assert_eq!(value["date"], "2024-02-29T12:00:00Z");
    }

    #[test]
    fn detects_formats_from_extension() {
        assert_eq!(DataFormat::from_extension("yml"), Some(DataFormat::Yaml));
        assert_eq!(DataFormat::from_extension("toml"), Some(DataFormat::Toml));
        assert_eq!(DataFormat::from_extension("JSON"), Some(DataFormat::Json));
        assert_eq!(DataFormat::from_extension("txt"), None);
        assert!(is_supported_input(Path::new("a/b.csv")));
        assert!(!is_supported_input(Path::new("a/b.png")));
    }
}
