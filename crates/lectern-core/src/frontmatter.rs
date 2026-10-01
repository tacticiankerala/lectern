//! YAML frontmatter, parsed into ordered, typed properties for the properties strip.
//!
//! Uses `saphyr` (a maintained YAML 1.2 parser) with scalar resolution turned off, so each value
//! keeps the exact text the author wrote (`1.50` stays `1.50`) and quoted values stay text.

use std::borrow::Cow;

use saphyr::{Mapping, Scalar, ScalarStyle, Tag, Yaml, YamlLoader};
use saphyr_parser::{Event, Parser, ScanError};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// One frontmatter value. YAML has no date type, so dates are recognised from the text of
/// string values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", content = "value", rename_all = "camelCase")]
#[ts(export)]
pub enum PropValue {
    Text(String),
    /// `YYYY-MM-DD`.
    Date(String),
    /// A date followed by a time, with optional seconds, fraction and zone.
    DateTime(String),
    /// The number exactly as written.
    Number(String),
    Bool(bool),
    List(Vec<PropValue>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Property {
    /// The key; nested maps are flattened to dotted keys such as `metadata.type`.
    pub key: String,
    pub value: PropValue,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum Frontmatter {
    /// Entries in document order.
    Parsed { entries: Vec<Property> },
    /// The YAML did not parse; the UI shows `raw` with `error` as a warning.
    Invalid { raw: String, error: String },
}

/// Larger frontmatter is not parsed.
const MAX_BYTES: usize = 64 * 1024;
/// The deepest nesting of sequences and maps accepted. This bounds the recursion when flattening.
const MAX_DEPTH: usize = 64;

const NOT_A_MAPPING: &str = "frontmatter must be `key: value` lines";
const TOO_LARGE: &str = "frontmatter too large";
const TOO_DEEP: &str = "frontmatter nested too deeply";
const HAS_ALIAS: &str = "YAML aliases aren't supported in frontmatter";

/// Parses the YAML between the `---` lines of a note.
pub fn parse_frontmatter(raw: &str) -> Frontmatter {
    let invalid = |error: String| Frontmatter::Invalid {
        raw: raw.to_owned(),
        error,
    };
    if let Err(error) = check_limits(raw) {
        return invalid(error);
    }
    let doc = match load_first_document(raw) {
        Ok(doc) => doc,
        Err(e) => return invalid(e.to_string()),
    };
    let mut entries = Vec::new();
    match doc.as_ref().map(untag) {
        Some(Yaml::Mapping(map)) => flatten_into(&mut entries, None, map),
        // Nothing but comments, or a document that is null or empty.
        None | Some(Yaml::BadValue) => {}
        Some(node) if value_of(node) == PropValue::Text(String::new()) => {}
        Some(_) => return invalid(NOT_A_MAPPING.to_owned()),
    }
    Frontmatter::Parsed { entries }
}

/// Rejects, before loading, YAML that is too large, nested too deeply or uses aliases. The loader
/// expands every alias in place, so a few hundred bytes of chained aliases can take gigabytes.
fn check_limits(raw: &str) -> Result<(), String> {
    if raw.len() > MAX_BYTES {
        return Err(TOO_LARGE.to_owned());
    }
    let mut depth = 0usize;
    for event in Parser::new_from_str(raw) {
        let (event, _) = event.map_err(|e| e.to_string())?;
        match event {
            Event::Alias(_) => return Err(HAS_ALIAS.to_owned()),
            Event::SequenceStart(..) | Event::MappingStart(..) => {
                depth += 1;
                if depth > MAX_DEPTH {
                    return Err(TOO_DEEP.to_owned());
                }
            }
            Event::SequenceEnd | Event::MappingEnd => depth = depth.saturating_sub(1),
            // Only the first document is loaded.
            Event::DocumentEnd => break,
            _ => {}
        }
    }
    Ok(())
}

/// Loads the first YAML document, keeping every scalar as written (`Yaml::Representation`).
fn load_first_document(raw: &str) -> Result<Option<Yaml<'_>>, ScanError> {
    let mut loader = YamlLoader::<Yaml>::default();
    loader.early_parse(false);
    Parser::new_from_str(raw).load(&mut loader, false)?;
    Ok(loader.into_documents().into_iter().next())
}

fn untag<'a, 'y>(node: &'a Yaml<'y>) -> &'a Yaml<'y> {
    match node {
        Yaml::Tagged(_, inner) => untag(inner),
        other => other,
    }
}

/// Appends one property per leaf, joining nested map keys with dots.
fn flatten_into(out: &mut Vec<Property>, prefix: Option<&str>, map: &Mapping) {
    for (key, value) in map {
        let key = match prefix {
            Some(prefix) => format!("{prefix}.{}", flow_text(key)),
            None => flow_text(key),
        };
        match untag(value) {
            Yaml::Mapping(inner) if !inner.is_empty() => flatten_into(out, Some(&key), inner),
            leaf => out.push(Property {
                key,
                value: value_of(leaf),
            }),
        }
    }
}

fn value_of(node: &Yaml) -> PropValue {
    match node {
        Yaml::Representation(text, style, tag) => scalar_value(text, *style, tag.as_ref()),
        Yaml::Value(scalar) => resolved_value(scalar, &scalar_text(scalar)),
        Yaml::Sequence(items) => PropValue::List(items.iter().map(value_of).collect()),
        // Only reached for a map inside a list (or an empty map): shown in flow style.
        Yaml::Mapping(_) => PropValue::Text(flow_text(node)),
        Yaml::Tagged(_, inner) => value_of(inner),
        Yaml::Alias(_) | Yaml::BadValue => PropValue::Text(String::new()),
    }
}

/// Resolves a scalar with the YAML 1.2 core schema: quoted values are always strings.
fn scalar_value(text: &str, style: ScalarStyle, tag: Option<&Cow<'_, Tag>>) -> PropValue {
    match Scalar::parse_from_cow_and_metadata(Cow::Borrowed(text), style, tag) {
        Some(scalar) => resolved_value(&scalar, text),
        // A core-schema tag the text does not fit, such as `!!int abc`.
        None => PropValue::Text(text.to_owned()),
    }
}

fn resolved_value(scalar: &Scalar, written: &str) -> PropValue {
    match scalar {
        Scalar::Null => PropValue::Text(String::new()),
        Scalar::Boolean(b) => PropValue::Bool(*b),
        Scalar::Integer(_) | Scalar::FloatingPoint(_) => PropValue::Number(written.to_owned()),
        Scalar::String(s) if is_date(s) => PropValue::Date(s.to_string()),
        Scalar::String(s) if is_datetime(s) => PropValue::DateTime(s.to_string()),
        Scalar::String(s) => PropValue::Text(s.to_string()),
    }
}

fn scalar_text(scalar: &Scalar) -> String {
    match scalar {
        Scalar::Null => String::new(),
        Scalar::Boolean(b) => b.to_string(),
        Scalar::Integer(i) => i.to_string(),
        Scalar::FloatingPoint(f) => f.to_string(),
        Scalar::String(s) => s.to_string(),
    }
}

/// A node as compact flow-style text, for keys and for maps that cannot be flattened.
fn flow_text(node: &Yaml) -> String {
    match node {
        Yaml::Representation(text, ..) => text.to_string(),
        Yaml::Value(scalar) => scalar_text(scalar),
        Yaml::Sequence(items) => {
            format!(
                "[{}]",
                items.iter().map(flow_text).collect::<Vec<_>>().join(", ")
            )
        }
        Yaml::Mapping(map) => {
            let pairs: Vec<String> = map
                .iter()
                .map(|(k, v)| format!("{}: {}", flow_text(k), flow_text(v)))
                .collect();
            format!("{{{}}}", pairs.join(", "))
        }
        Yaml::Tagged(_, inner) => flow_text(inner),
        Yaml::Alias(_) | Yaml::BadValue => String::new(),
    }
}

/// `YYYY-MM-DD` with a plausible month and day.
fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && digits(&b[0..4])
        && b[4] == b'-'
        && in_range(&b[5..7], 1, 12)
        && b[7] == b'-'
        && in_range(&b[8..10], 1, 31)
}

/// A date, then `T`, `t` or a space, then `HH:MM`, optional `:SS` and `.fraction`, then an
/// optional zone: `Z`, `±HH`, `±HHMM` or `±HH:MM`.
fn is_datetime(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() < 16 || !s.is_char_boundary(10) || !is_date(&s[..10]) {
        return false;
    }
    if !matches!(b[10], b'T' | b't' | b' ') || !in_range(&b[11..13], 0, 23) || b[13] != b':' {
        return false;
    }
    if !in_range(&b[14..16], 0, 59) {
        return false;
    }
    let mut rest = &b[16..];
    if let [b':', s1, s2, tail @ ..] = rest {
        if !in_range(&[*s1, *s2], 0, 60) {
            return false;
        }
        rest = tail;
        if let [b'.', tail @ ..] = rest {
            let n = tail.iter().take_while(|c| c.is_ascii_digit()).count();
            if n == 0 {
                return false;
            }
            rest = &tail[n..];
        }
    }
    match rest {
        [] | [b'Z' | b'z'] => true,
        [b'+' | b'-', zone @ ..] => match zone {
            [h1, h2] => in_range(&[*h1, *h2], 0, 23),
            [h1, h2, m1, m2] | [h1, h2, b':', m1, m2] => {
                in_range(&[*h1, *h2], 0, 23) && in_range(&[*m1, *m2], 0, 59)
            }
            _ => false,
        },
        _ => false,
    }
}

fn digits(b: &[u8]) -> bool {
    b.iter().all(u8::is_ascii_digit)
}

/// Two ASCII digits whose value is within `lo..=hi`.
fn in_range(b: &[u8], lo: u8, hi: u8) -> bool {
    digits(b) && b.len() == 2 && (lo..=hi).contains(&((b[0] - b'0') * 10 + (b[1] - b'0')))
}
