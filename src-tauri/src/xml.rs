//! Streaming parse of `bazel query --output=xml` output.
//!
//! The relevant shape is:
//!
//! ```xml
//! <query version="2">
//!   <rule class="java_library" location="/ws/app/lib/BUILD:3:8" name="//app/lib:parser">
//!     <list name="deps">
//!       <label value="//app/lib:parser_proto"/>
//!     </list>
//!     <rule-input name="//app/lib:Parser.java"/>
//!     <rule-output name="//app/lib:libparser.jar"/>
//!   </rule>
//! </query>
//! ```
//!
//! Label references are collected only from attribute elements (children of
//! `<rule>` carrying the attribute name, e.g. `<list name="deps">`). The
//! flattened `<rule-input>` elements mix in implicit dependencies and lose the
//! attribute name, so they are ignored.
//!
//! Each nesting level is one loop (`parse` → `read_body` → `read_children`),
//! so there is no cross-event state to keep consistent. A query cut short
//! (`--keep_going` partial output, or a killed process) yields the rules that
//! were read — a rule truncated mid-body keeps the attributes that arrived.

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use std::io::BufRead;

/// One `<rule>` element as read from the XML stream.
#[derive(Debug, Default)]
pub struct RawRule {
    /// The rule's absolute label (the XML `name` attribute).
    pub label: String,
    /// Rule class (the XML `class` attribute), e.g. `java_library`.
    pub rule_class: String,
    /// BUILD/bzl file path and 1-based line, from the XML `location` attribute.
    pub location: Option<(String, u32)>,
    /// (attribute name, label value) pairs from label-typed attribute
    /// elements: direct `<label name="x" value=".."/>` and `<label value>` at
    /// any depth inside named attribute elements such as `<list name="x">`.
    pub label_attrs: Vec<(String, String)>,
    /// `<rule-output name="...">` values: declared outputs of the rule.
    pub outputs: Vec<String>,
}

pub fn parse_rules<R: BufRead>(input: R) -> Vec<RawRule> {
    let mut reader = Reader::from_reader(input);
    let mut rules = Vec::new();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) if e.name().as_ref() == b"rule" => {
                let mut rule = raw_rule(&reader, &e);
                read_body(&mut reader, &mut rule);
                rules.push(rule);
            }
            Ok(Event::Empty(e)) if e.name().as_ref() == b"rule" => {
                rules.push(raw_rule(&reader, &e));
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    rules
}

/// `<rule ...>` was opened: read its attributes into a fresh RawRule.
fn raw_rule(reader: &Reader<impl BufRead>, e: &BytesStart) -> RawRule {
    RawRule {
        label: attr_value(reader, e, "name").unwrap_or_default(),
        rule_class: attr_value(reader, e, "class").unwrap_or_default(),
        location: attr_value(reader, e, "location").and_then(|l| parse_location(&l)),
        ..Default::default()
    }
}

/// Consume a `<rule>`'s children up to `</rule>` (or a truncated EOF).
fn read_body(reader: &mut Reader<impl BufRead>, rule: &mut RawRule) {
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => match e.name().as_ref() {
                b"rule-output" => push_output(reader, &e, rule),
                tag => read_children(reader, tag, attr_value(reader, &e, "name"), rule),
            },
            Ok(Event::Empty(e)) => match e.name().as_ref() {
                b"rule-output" => push_output(reader, &e, rule),
                b"label" => {
                    if let (Some(n), Some(v)) = (
                        attr_value(reader, &e, "name"),
                        attr_value(reader, &e, "value"),
                    ) {
                        rule.label_attrs.push((n, v));
                    }
                }
                _ => {}
            },
            Ok(Event::End(e)) if e.name().as_ref() == b"rule" => break,
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
}

/// Consume an attribute element's children up to its end tag. Every
/// `<label value="..">` inside becomes an (attr name, value) pair. Elements we
/// do not care about (rule-input, scalar values) are consumed and ignored.
fn read_children(
    reader: &mut Reader<impl BufRead>,
    tag: &[u8],
    name: Option<String>,
    rule: &mut RawRule,
) {
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(e)) | Ok(Event::Start(e)) if e.name().as_ref() == b"label" => {
                if let (Some(n), Some(v)) = (name.as_deref(), attr_value(reader, &e, "value"))
                    && !n.is_empty()
                {
                    rule.label_attrs.push((n.to_string(), v));
                }
            }
            Ok(Event::End(e)) if e.name().as_ref() == tag => break,
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
}

fn push_output(reader: &Reader<impl BufRead>, e: &BytesStart, rule: &mut RawRule) {
    if let Some(v) = attr_value(reader, e, "name") {
        rule.outputs.push(v);
    }
}

fn attr_value(reader: &Reader<impl BufRead>, e: &BytesStart, key: &'static str) -> Option<String> {
    e.try_get_attribute(key)
        .ok()
        .flatten()
        .and_then(|a| a.decode_and_unescape_value(reader.decoder()).ok())
        .map(|c| c.into_owned())
}

/// `location` is `path:line:column`; keep file and line.
fn parse_location(raw: &str) -> Option<(String, u32)> {
    let (head, _col) = raw.rsplit_once(':')?;
    let (file, line) = head.rsplit_once(':')?;
    Some((file.to_string(), line.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};
    use std::io::Cursor;

    fn fixture_rules() -> Vec<RawRule> {
        let xml = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/query.xml"
        ))
        .unwrap();
        parse_rules(Cursor::new(xml))
    }

    #[test]
    fn parses_all_rules() {
        let rules = fixture_rules();
        let labels: HashSet<&str> = rules.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "//app/lib:parser",
                "//app/lib:parser_proto",
                "//app/lib:parser_test",
                "//app",
                "//app:grammars",
                "//tools:launcher",
                "//tools:cc",
            ]
            .into_iter()
            .collect()
        );
    }

    #[test]
    fn parses_class_location_and_outputs() {
        let rules = fixture_rules();
        let by_label: HashMap<&str, &RawRule> =
            rules.iter().map(|r| (r.label.as_str(), r)).collect();

        let parser = by_label["//app/lib:parser"];
        assert_eq!(parser.rule_class, "java_library");
        assert_eq!(
            parser.location,
            Some(("/repo/app/lib/BUILD".to_string(), 3))
        );
        assert_eq!(parser.outputs, ["//app/lib:libparser.jar"]);

        // Macro-generated rule: location lands inside a .bzl file.
        let launcher = by_label["//tools:launcher"];
        assert_eq!(
            launcher.location,
            Some(("/repo/tools/build_defs.bzl".to_string(), 22))
        );

        // A rule that declares no outputs has none.
        assert!(by_label["//app/lib:parser_test"].outputs.is_empty());
    }

    #[test]
    fn truncated_xml_yields_completed_rules() {
        let xml = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/query.xml"
        ))
        .unwrap();
        // Cut mid-rule: everything after the first </rule> is lost.
        let cut = xml.find("</rule>").unwrap() + "</rule>".len();
        let rules = parse_rules(Cursor::new(&xml[..cut]));
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].label, "//app/lib:parser");

        // Cut mid-body: the rule survives with the attributes that arrived.
        let mid = xml.find("<list name=\"deps\">").unwrap();
        let rules = parse_rules(Cursor::new(&xml[..mid]));
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].label, "//app/lib:parser");
        assert!(!rules[0].label_attrs.iter().any(|(a, _)| a == "deps"));
    }
}
