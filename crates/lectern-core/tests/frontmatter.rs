use lectern_core::frontmatter::*;

fn entries(raw: &str) -> Vec<Property> {
    match parse_frontmatter(raw) {
        Frontmatter::Parsed { entries } => entries,
        Frontmatter::Invalid { error, .. } => panic!("expected parsed frontmatter, got: {error}"),
    }
}

fn value_of(entries: &[Property], key: &str) -> PropValue {
    entries
        .iter()
        .find(|p| p.key == key)
        .unwrap_or_else(|| panic!("no key {key}"))
        .value
        .clone()
}

#[test]
fn task_readme() {
    let f = parse_frontmatter(
        "status: active # in progress\nstarted: 2026-09-01\nprs: [6257, 6261]\nbranch: feat/x\n",
    );
    let Frontmatter::Parsed { entries } = f else {
        panic!()
    };
    assert_eq!(
        entries[0],
        Property {
            key: "status".into(),
            value: PropValue::Text("active".into())
        }
    );
    assert_eq!(entries[1].value, PropValue::Date("2026-09-01".into()));
    assert_eq!(
        entries[2].value,
        PropValue::List(vec![
            PropValue::Number("6257".into()),
            PropValue::Number("6261".into())
        ])
    );
    assert_eq!(entries[3].value, PropValue::Text("feat/x".into()));
}

#[test]
fn nested_map_flattened() {
    let f = parse_frontmatter(
        "name: x\nmetadata:\n  type: feedback\n  modified: 2026-01-01T10:00:00Z\n",
    );
    let Frontmatter::Parsed { entries } = f else {
        panic!()
    };
    assert!(entries
        .iter()
        .any(|p| p.key == "metadata.type" && p.value == PropValue::Text("feedback".into())));
    assert!(entries
        .iter()
        .any(|p| p.key == "metadata.modified" && matches!(p.value, PropValue::DateTime(_))));
}

#[test]
fn invalid_yaml_kept_raw() {
    assert!(matches!(
        parse_frontmatter("a: [unclosed\n"),
        Frontmatter::Invalid { .. }
    ));
}

#[test]
fn invalid_yaml_keeps_the_raw_text_and_an_error() {
    let Frontmatter::Invalid { raw, error } = parse_frontmatter("a: [unclosed\n") else {
        panic!()
    };
    assert_eq!(raw, "a: [unclosed\n");
    assert!(!error.is_empty());
}

#[test]
fn entries_keep_document_order() {
    let keys: Vec<String> = entries("zeta: 1\nalpha: 2\nmid: 3\n")
        .into_iter()
        .map(|p| p.key)
        .collect();
    assert_eq!(keys, ["zeta", "alpha", "mid"]);
}

#[test]
fn scalar_types() {
    let e = entries(concat!(
        "quoted: \"Quoted \\\"escaped\\\" text\"\n",
        "single: 'it''s'\n",
        "float: 1.50\n",
        "negative: -3\n",
        "yes_bool: true\n",
        "no_bool: false\n",
        "yes_word: yes\n",
        "quoted_number: \"42\"\n",
        "empty:\n",
        "tilde: ~\n",
        "spaced_datetime: 2026-01-01 10:00\n",
        "offset_datetime: 2026-01-01T10:00:00+05:30\n",
        "not_a_date: 2026-13-01\n",
        "block: |\n  line one\n  line two\n",
    ));
    assert_eq!(
        value_of(&e, "quoted"),
        PropValue::Text("Quoted \"escaped\" text".into())
    );
    assert_eq!(value_of(&e, "single"), PropValue::Text("it's".into()));
    assert_eq!(value_of(&e, "float"), PropValue::Number("1.50".into()));
    assert_eq!(value_of(&e, "negative"), PropValue::Number("-3".into()));
    assert_eq!(value_of(&e, "yes_bool"), PropValue::Bool(true));
    assert_eq!(value_of(&e, "no_bool"), PropValue::Bool(false));
    assert_eq!(value_of(&e, "yes_word"), PropValue::Text("yes".into()));
    assert_eq!(value_of(&e, "quoted_number"), PropValue::Text("42".into()));
    assert_eq!(value_of(&e, "empty"), PropValue::Text(String::new()));
    assert_eq!(value_of(&e, "tilde"), PropValue::Text(String::new()));
    assert_eq!(
        value_of(&e, "spaced_datetime"),
        PropValue::DateTime("2026-01-01 10:00".into())
    );
    assert_eq!(
        value_of(&e, "offset_datetime"),
        PropValue::DateTime("2026-01-01T10:00:00+05:30".into())
    );
    assert_eq!(
        value_of(&e, "not_a_date"),
        PropValue::Text("2026-13-01".into())
    );
    assert_eq!(
        value_of(&e, "block"),
        PropValue::Text("line one\nline two\n".into())
    );
}

#[test]
fn block_lists_and_deep_nesting() {
    let e = entries("tags:\n  - nvim\n  - motions\na:\n  b:\n    c: deep\n");
    assert_eq!(
        value_of(&e, "tags"),
        PropValue::List(vec![
            PropValue::Text("nvim".into()),
            PropValue::Text("motions".into())
        ])
    );
    assert_eq!(value_of(&e, "a.b.c"), PropValue::Text("deep".into()));
}

#[test]
fn empty_frontmatter_has_no_entries() {
    assert_eq!(entries(""), vec![]);
    assert_eq!(entries("# only a comment\n"), vec![]);
}

#[test]
fn a_scalar_document_is_invalid() {
    assert!(matches!(
        parse_frontmatter("just some words\n"),
        Frontmatter::Invalid { .. }
    ));
}

#[test]
fn serializes_with_kind_tags() {
    let json = serde_json::to_value(parse_frontmatter("status: done\nprs: [1]\n")).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "kind": "parsed",
            "entries": [
                { "key": "status", "value": { "kind": "text", "value": "done" } },
                { "key": "prs", "value": { "kind": "list", "value": [{ "kind": "number", "value": "1" }] } }
            ]
        })
    );
}

fn error_of(raw: &str) -> String {
    match parse_frontmatter(raw) {
        Frontmatter::Invalid { error, .. } => error,
        Frontmatter::Parsed { entries } => panic!("expected invalid frontmatter, got {entries:?}"),
    }
}

#[test]
fn an_alias_chain_is_rejected_quickly() {
    // Each level repeats the previous one nine times: expanded, six levels is 9^7 scalars.
    let mut yaml = String::from("l0: &l0 [lol, lol, lol, lol, lol, lol, lol, lol, lol]\n");
    for i in 1..=6 {
        let refs = vec![format!("*l{}", i - 1); 9].join(", ");
        yaml.push_str(&format!("l{i}: &l{i} [{refs}]\n"));
    }
    let start = std::time::Instant::now();
    let error = error_of(&yaml);
    assert!(
        start.elapsed() < std::time::Duration::from_secs(1),
        "took {:?}",
        start.elapsed()
    );
    assert_eq!(error, "YAML aliases aren't supported in frontmatter");
}

#[test]
fn any_alias_is_rejected() {
    assert_eq!(
        error_of("a: &x 1\nb: *x\n"),
        "YAML aliases aren't supported in frontmatter"
    );
}

#[test]
fn anchors_without_aliases_still_parse() {
    let e = entries("a: &x 1\nb: two\n");
    assert_eq!(value_of(&e, "a"), PropValue::Number("1".into()));
    assert_eq!(value_of(&e, "b"), PropValue::Text("two".into()));
}

#[test]
fn deep_nesting_is_rejected_without_overflowing() {
    let yaml = format!("a: {}{}\n", "[".repeat(100), "]".repeat(100));
    assert_eq!(error_of(&yaml), "frontmatter nested too deeply");
}

#[test]
fn nesting_up_to_the_limit_parses() {
    // The top-level map is depth 1, so 63 brackets reach the limit of 64 and 64 pass it.
    let at_limit = format!("a: {}{}\n", "[".repeat(63), "]".repeat(63));
    assert!(matches!(
        parse_frontmatter(&at_limit),
        Frontmatter::Parsed { .. }
    ));
    let over = format!("a: {}{}\n", "[".repeat(64), "]".repeat(64));
    assert_eq!(error_of(&over), "frontmatter nested too deeply");
}

#[test]
fn oversized_frontmatter_is_rejected() {
    let mut yaml = String::new();
    for i in 0.. {
        if yaml.len() >= 70 * 1024 {
            break;
        }
        yaml.push_str(&format!("key_{i}: value\n"));
    }
    let Frontmatter::Invalid { raw, error } = parse_frontmatter(&yaml) else {
        panic!()
    };
    assert_eq!(error, "frontmatter too large");
    assert_eq!(raw, yaml);
}
