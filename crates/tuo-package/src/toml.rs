//! A deliberately small TOML reader for the package manifest and lockfile.
//!
//! `tdg.toml` and `tdg.lock` are TOML, but the package format uses only a
//! narrow, fixed subset of the language: top-level key/values, `[table]`
//! headers, `[[array-of-tables]]` headers, double-quoted strings, non-negative
//! integers, arrays of strings, and inline tables (`{ path = "..." }`) as
//! dependency values. Rather than take on a full TOML parser dependency, this
//! module reads exactly that subset into a simple tree and reports a precise
//! [`TomlError`] on anything outside it.
//!
//! Serialization is handled directly by the manifest/lockfile writers (they
//! emit canonical text), so this module only *reads*.
//!
//! The subset is intentional and documented: a manifest that reaches for a
//! feature outside it gets a clear error naming the unsupported construct,
//! never a silent misparse.

use std::collections::BTreeMap;
use std::fmt;

/// A parsed TOML value, restricted to the subset the package format uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// A double-quoted string.
    String(String),
    /// A non-negative integer.
    Integer(u64),
    /// An array — of strings in every position the format uses.
    Array(Vec<Value>),
    /// An inline table, e.g. `{ path = "../util" }`.
    Table(BTreeMap<String, Value>),
}

impl Value {
    /// Borrow this value as a string, or `None` if it is another kind.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    /// Borrow this value as an integer, or `None` if it is another kind.
    #[must_use]
    pub fn as_integer(&self) -> Option<u64> {
        match self {
            Value::Integer(n) => Some(*n),
            _ => None,
        }
    }

    /// Borrow this value as an array, or `None` if it is another kind.
    #[must_use]
    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(items) => Some(items),
            _ => None,
        }
    }

    /// Borrow this value as an inline table, or `None` if it is another kind.
    #[must_use]
    pub fn as_table(&self) -> Option<&BTreeMap<String, Value>> {
        match self {
            Value::Table(map) => Some(map),
            _ => None,
        }
    }
}

/// A parsed TOML document: named tables, each a map of key to [`Value`], plus
/// arrays of tables (from `[[header]]`).
#[derive(Debug, Default, Clone)]
pub struct Document {
    /// Top-level key/values written before any table header.
    pub root: BTreeMap<String, Value>,
    /// Single tables (`[name]`), in insertion order-independent map form.
    pub tables: BTreeMap<String, BTreeMap<String, Value>>,
    /// Arrays of tables (`[[name]]`), preserving document order within each.
    pub array_tables: BTreeMap<String, Vec<BTreeMap<String, Value>>>,
}

impl Document {
    /// The single table named `name`, if present.
    #[must_use]
    pub fn table(&self, name: &str) -> Option<&BTreeMap<String, Value>> {
        self.tables.get(name)
    }

    /// The array of tables named `name` (`[[name]]`), or an empty slice.
    #[must_use]
    pub fn array_table(&self, name: &str) -> &[BTreeMap<String, Value>] {
        self.array_tables.get(name).map_or(&[], Vec::as_slice)
    }
}

/// A precise error from reading the manifest/lockfile TOML subset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TomlError {
    /// One-based line number where the error was detected.
    pub line: usize,
    /// A human-readable description of what went wrong.
    pub message: String,
}

impl fmt::Display for TomlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for TomlError {}

/// Parse `input` as a TOML document restricted to the package-format subset.
///
/// # Errors
///
/// Returns a [`TomlError`] (with a line number) on any syntax outside the
/// subset: an unterminated string, a malformed header, a value kind the format
/// does not use, or a duplicate `[table]` header.
pub fn parse(input: &str) -> Result<Document, TomlError> {
    let mut doc = Document::default();
    // Where subsequent `key = value` lines land: the root, a named table, or
    // the last-pushed entry of an array-of-tables.
    enum Cursor {
        Root,
        Table(String),
        ArrayTable(String),
    }
    let mut cursor = Cursor::Root;

    for (index, raw) in input.lines().enumerate() {
        let line_no = index + 1;
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }

        if let Some(rest) = line.strip_prefix("[[") {
            let name = rest
                .strip_suffix("]]")
                .ok_or_else(|| err(line_no, "unterminated `[[array-of-tables]]` header"))?
                .trim()
                .to_string();
            check_key(&name, line_no)?;
            doc.array_tables
                .entry(name.clone())
                .or_default()
                .push(BTreeMap::new());
            cursor = Cursor::ArrayTable(name);
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            let name = rest
                .strip_suffix(']')
                .ok_or_else(|| err(line_no, "unterminated `[table]` header"))?
                .trim()
                .to_string();
            check_key(&name, line_no)?;
            if doc.tables.contains_key(&name) {
                return Err(err(line_no, format!("duplicate table `[{name}]`")));
            }
            doc.tables.insert(name.clone(), BTreeMap::new());
            cursor = Cursor::Table(name);
            continue;
        }

        // A `key = value` line.
        let (key, value) = parse_pair(line, line_no)?;
        let dest = match &cursor {
            Cursor::Root => &mut doc.root,
            Cursor::Table(name) => doc
                .tables
                .get_mut(name)
                .expect("table was inserted when its header was seen"),
            Cursor::ArrayTable(name) => doc
                .array_tables
                .get_mut(name)
                .and_then(|entries| entries.last_mut())
                .expect("an array-table entry was pushed when its header was seen"),
        };
        if dest.insert(key.clone(), value).is_some() {
            return Err(err(line_no, format!("duplicate key `{key}`")));
        }
    }

    Ok(doc)
}

/// Remove a trailing `# comment`, respecting a `#` inside a quoted string.
fn strip_comment(line: &str) -> &str {
    let mut in_string = false;
    for (i, ch) in line.char_indices() {
        match ch {
            '"' => in_string = !in_string,
            '#' if !in_string => return &line[..i],
            _ => {}
        }
    }
    line
}

/// Split and parse a `key = value` line.
fn parse_pair(line: &str, line_no: usize) -> Result<(String, Value), TomlError> {
    let eq = line
        .find('=')
        .ok_or_else(|| err(line_no, "expected a `key = value` assignment"))?;
    let key = line[..eq].trim().to_string();
    check_key(&key, line_no)?;
    let value = parse_value(line[eq + 1..].trim(), line_no)?;
    Ok((key, value))
}

/// A bare key must be a non-empty run of `[A-Za-z0-9_-]`. Dotted keys are not
/// part of the subset.
fn check_key(key: &str, line_no: usize) -> Result<(), TomlError> {
    if key.is_empty() {
        return Err(err(line_no, "empty key"));
    }
    if !key
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(err(
            line_no,
            format!("key `{key}` uses characters outside the supported subset (A-Za-z0-9_-)"),
        ));
    }
    Ok(())
}

/// Parse a value: string, integer, array of strings, or inline table.
fn parse_value(text: &str, line_no: usize) -> Result<Value, TomlError> {
    if let Some(rest) = text.strip_prefix('"') {
        let close = rest
            .find('"')
            .ok_or_else(|| err(line_no, "unterminated string literal"))?;
        if rest[close + 1..].trim().is_empty() {
            return Ok(Value::String(rest[..close].to_string()));
        }
        return Err(err(line_no, "trailing characters after string value"));
    }
    if text.starts_with('[') {
        return parse_array(text, line_no);
    }
    if text.starts_with('{') {
        return parse_inline_table(text, line_no);
    }
    // An integer is the only bare value the subset accepts.
    text.parse::<u64>().map(Value::Integer).map_err(|_| {
        err(
            line_no,
            format!("`{text}` is not a supported value (expected a string, non-negative integer, array, or inline table)"),
        )
    })
}

/// Parse `[ "a", "b" ]` — an array of strings.
fn parse_array(text: &str, line_no: usize) -> Result<Value, TomlError> {
    let inner = text
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .ok_or_else(|| err(line_no, "unterminated array"))?
        .trim();
    if inner.is_empty() {
        return Ok(Value::Array(Vec::new()));
    }
    let mut items = Vec::new();
    for element in split_top_level(inner, line_no)? {
        items.push(parse_value(element.trim(), line_no)?);
    }
    Ok(Value::Array(items))
}

/// Parse `{ key = "value", key2 = "value2" }` — an inline table.
fn parse_inline_table(text: &str, line_no: usize) -> Result<Value, TomlError> {
    let inner = text
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .ok_or_else(|| err(line_no, "unterminated inline table"))?
        .trim();
    let mut map = BTreeMap::new();
    if inner.is_empty() {
        return Ok(Value::Table(map));
    }
    for entry in split_top_level(inner, line_no)? {
        let (key, value) = parse_pair(entry.trim(), line_no)?;
        if map.insert(key.clone(), value).is_some() {
            return Err(err(
                line_no,
                format!("duplicate key `{key}` in inline table"),
            ));
        }
    }
    Ok(Value::Table(map))
}

/// Split a comma-separated list, ignoring commas inside strings, arrays, or
/// inline tables. Enough for the one-level nesting the subset allows.
fn split_top_level(text: &str, line_no: usize) -> Result<Vec<String>, TomlError> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut in_string = false;
    let mut start = 0usize;
    for (i, ch) in text.char_indices() {
        match ch {
            '"' => in_string = !in_string,
            '[' | '{' if !in_string => depth += 1,
            ']' | '}' if !in_string => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| err(line_no, "unbalanced brackets"))?;
            }
            ',' if !in_string && depth == 0 => {
                parts.push(text[start..i].to_string());
                start = i + 1;
            }
            _ => {}
        }
    }
    let tail = text[start..].trim();
    if !tail.is_empty() {
        parts.push(tail.to_string());
    }
    Ok(parts)
}

/// Build a [`TomlError`] at `line`.
fn err(line: usize, message: impl Into<String>) -> TomlError {
    TomlError {
        line,
        message: message.into(),
    }
}

/// Escape a string for emission inside a double-quoted TOML value. The package
/// format's strings (names, versions, paths, hex checksums) never contain
/// control characters, but a `"` or `\` is escaped defensively so a
/// round-trip is exact.
#[must_use]
pub fn escape_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{Value, parse};

    #[test]
    fn parses_a_manifest_shape() {
        let doc = parse(
            "\
[package]
name = \"app\"
version = \"0.1.0\"
edition = \"2024\"

[modules]
root = \"src\"

[dependencies]
util = { path = \"../util\" }
",
        )
        .expect("valid manifest");

        let package = doc.table("package").expect("has [package]");
        assert_eq!(package["name"], Value::String("app".into()));
        assert_eq!(package["edition"], Value::String("2024".into()));

        let deps = doc.table("dependencies").expect("has [dependencies]");
        let util = deps["util"].as_table().expect("inline table");
        assert_eq!(util["path"], Value::String("../util".into()));
    }

    #[test]
    fn parses_a_lockfile_shape() {
        let doc = parse(
            "\
version = 1

[[package]]
name = \"app\"
version = \"0.1.0\"
source = \"root\"
checksum = \"abcd\"
dependencies = [\"util\"]

[[package]]
name = \"util\"
version = \"0.2.0\"
source = \"path+../util\"
checksum = \"ef01\"
dependencies = []
",
        )
        .expect("valid lockfile");

        assert_eq!(doc.root["version"], Value::Integer(1));
        let packages = doc.array_table("package");
        assert_eq!(packages.len(), 2);
        assert_eq!(packages[0]["name"], Value::String("app".into()));
        assert_eq!(
            packages[0]["dependencies"],
            Value::Array(vec![Value::String("util".into())])
        );
        assert_eq!(packages[1]["dependencies"], Value::Array(vec![]));
    }

    #[test]
    fn rejects_unterminated_string() {
        let e = parse("name = \"oops").expect_err("unterminated");
        assert!(e.message.contains("unterminated string"), "{e}");
    }

    #[test]
    fn rejects_duplicate_table() {
        let e = parse("[a]\n[a]\n").expect_err("duplicate");
        assert!(e.message.contains("duplicate table"), "{e}");
        assert_eq!(e.line, 2);
    }

    #[test]
    fn rejects_value_outside_subset() {
        let e = parse("x = 1.5").expect_err("float unsupported");
        assert!(e.message.contains("not a supported value"), "{e}");
    }

    #[test]
    fn comments_and_blank_lines_are_ignored() {
        let doc = parse("# header\n\nx = 1 # trailing\n").expect("valid");
        assert_eq!(doc.root["x"], Value::Integer(1));
    }

    #[test]
    fn a_hash_inside_a_string_is_not_a_comment() {
        let doc = parse("checksum = \"ab#cd\"\n").expect("valid");
        assert_eq!(doc.root["checksum"], Value::String("ab#cd".into()));
    }
}
