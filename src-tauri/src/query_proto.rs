//! Parse of `bazel query --output=proto` output.
//!
//! The query schema is Bazel's vendored `build.proto` (`blaze_query`
//! package). Label references are collected from label-typed attributes
//! only, keeping the attribute name each label arrived under. Flattened
//! `rule_input` entries lose the attribute name, so they are ignored.
//! Corrupt bytes are an error; a `--keep_going` partial result is intact
//! protobuf and parses normally.

use crate::protocol::query as proto;
use crate::protocol::query::attribute::Discriminator as AttributeType;
use crate::protocol::query::target::Discriminator as TargetType;
use prost::Message;
use std::io::Read;

/// One `Rule` message as decoded from the query stream.
#[derive(Debug, Default)]
pub struct RawRule {
    /// The rule's absolute label (the proto `name` field).
    pub label: String,
    /// Rule class (the proto `rule_class` field), e.g. `java_library`.
    pub rule_class: String,
    /// BUILD/bzl file path and 1-based line, from the proto `location`
    /// field's `<path>:<line>:<column>`.
    pub location: Option<(String, u32)>,
    /// (attribute name, label value) pairs from label-typed attributes.
    pub label_attrs: Vec<(String, String)>,
    /// `rule_output` values: declared outputs of the rule.
    pub outputs: Vec<String>,
}

#[derive(Debug, Default)]
pub struct ParsedQuery {
    pub rules: Vec<RawRule>,
    pub source_files: Vec<String>,
}

#[tracing::instrument(target = "zeal::operations", name = "query.proto_parse", skip_all)]
pub fn parse_query(mut input: impl Read) -> Result<ParsedQuery, String> {
    let mut bytes = Vec::new();
    input
        .read_to_end(&mut bytes)
        .map_err(|e| format!("failed to read query output: {e}"))?;
    let result = proto::QueryResult::decode(bytes.as_slice())
        .map_err(|e| format!("invalid query protobuf: {e}"))?;
    let mut parsed = ParsedQuery::default();
    for target in result.target {
        let kind = TargetType::try_from(target.r#type)
            .map_err(|_| "unknown query target type".to_string())?;
        let payloads = [
            target.rule.is_some(),
            target.source_file.is_some(),
            target.generated_file.is_some(),
            target.package_group.is_some(),
            target.environment_group.is_some(),
        ];
        let matching = match kind {
            TargetType::Rule => target.rule.is_some(),
            TargetType::SourceFile => target.source_file.is_some(),
            TargetType::GeneratedFile => target.generated_file.is_some(),
            TargetType::PackageGroup => target.package_group.is_some(),
            TargetType::EnvironmentGroup => target.environment_group.is_some(),
        };
        if !matching || payloads.into_iter().filter(|present| *present).count() != 1 {
            return Err("query target type does not match its payload".into());
        }
        match kind {
            TargetType::Rule => {
                if let Some(rule) = target.rule {
                    parsed.rules.push(parse_rule(rule)?);
                }
            }
            TargetType::SourceFile => {
                if let Some(source) = target.source_file {
                    if source.name.is_empty() {
                        return Err("query source file has no name".into());
                    }
                    parsed.source_files.push(source.name);
                }
            }
            _ => {}
        }
    }
    Ok(parsed)
}

fn parse_rule(rule: proto::Rule) -> Result<RawRule, String> {
    if rule.name.is_empty() || rule.rule_class.is_empty() {
        return Err("query rule has no name or class".into());
    }
    if rule.rule_output.iter().any(String::is_empty) {
        return Err("query rule has an empty output label".into());
    }
    let mut label_attrs = Vec::new();
    for attr in rule.attribute {
        append_labels(attr, &mut label_attrs)?;
    }
    Ok(RawRule {
        label: rule.name,
        rule_class: rule.rule_class,
        location: rule.location.as_deref().and_then(parse_location),
        label_attrs,
        outputs: rule.rule_output,
    })
}

fn append_labels(attr: proto::Attribute, labels: &mut Vec<(String, String)>) -> Result<(), String> {
    let kind = AttributeType::try_from(attr.r#type)
        .map_err(|_| format!("unknown query attribute type: {}", attr.name))?;
    if attr.nodep == Some(true) {
        return Ok(());
    }
    let values: Vec<String> = match kind {
        AttributeType::Label => attr.string_value.into_iter().collect(),
        AttributeType::LabelList => attr.string_list_value,
        AttributeType::LabelDictUnary => attr
            .label_dict_unary_value
            .into_iter()
            .map(|e| e.value)
            .collect(),
        AttributeType::LabelListDict => attr
            .label_list_dict_value
            .into_iter()
            .flat_map(|e| e.value)
            .collect(),
        AttributeType::LabelKeyedStringDict => attr
            .label_keyed_string_dict_value
            .into_iter()
            .map(|e| e.key)
            .collect(),
        AttributeType::SelectorList => {
            return Err(format!(
                "unsupported unflattened query attribute: {}",
                attr.name
            ));
        }
        _ => return Ok(()),
    };
    if attr.name.is_empty() || values.iter().any(String::is_empty) {
        return Err("query label attribute has an empty name or label".into());
    }
    labels.extend(values.into_iter().map(|value| (attr.name.clone(), value)));
    Ok(())
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
    use prost::Message;

    fn wrap_rule(rule: proto::Rule) -> Vec<u8> {
        proto::QueryResult {
            target: vec![proto::Target {
                r#type: TargetType::Rule as i32,
                rule: Some(rule),
                ..Default::default()
            }],
        }
        .encode_to_vec()
    }

    fn rule(name: &str) -> proto::Rule {
        proto::Rule {
            name: name.to_string(),
            rule_class: "cc_library".to_string(),
            ..Default::default()
        }
    }

    fn attr(name: &str, kind: AttributeType) -> proto::Attribute {
        proto::Attribute {
            name: name.to_string(),
            r#type: kind as i32,
            ..Default::default()
        }
    }

    #[test]
    fn collects_label_values_under_their_attribute_names() {
        let mut label = attr("label_attr", AttributeType::Label);
        label.string_value = Some("//app:one".into());
        let mut list = attr("list_attr", AttributeType::LabelList);
        list.string_list_value = vec!["//app:two".into(), "//app:three".into()];
        let mut dict_unary = attr("dict_unary", AttributeType::LabelDictUnary);
        dict_unary.label_dict_unary_value = vec![proto::LabelDictUnaryEntry {
            key: "not-a-label".into(),
            value: "//app:val".into(),
        }];
        let mut list_dict = attr("list_dict", AttributeType::LabelListDict);
        list_dict.label_list_dict_value = vec![
            proto::LabelListDictEntry {
                key: "//app:k1".into(),
                value: vec!["//app:a".into()],
            },
            proto::LabelListDictEntry {
                key: "//app:k2".into(),
                value: vec!["//app:b".into()],
            },
        ];
        let mut keyed = attr("keyed", AttributeType::LabelKeyedStringDict);
        keyed.label_keyed_string_dict_value = vec![proto::LabelKeyedStringDictEntry {
            key: "//app:key".into(),
            value: "not-a-label".into(),
        }];
        let mut string = attr("string_attr", AttributeType::String);
        string.string_value = Some("//app:not_a_label".into());
        let mut output = attr("output_attr", AttributeType::Output);
        output.string_value = Some("//app:out1".into());

        let mut r = rule("//app:main");
        r.rule_input = vec!["//app:flattened_input".into()];
        r.attribute = vec![label, list, dict_unary, list_dict, keyed, string, output];
        let parsed = parse_query(&wrap_rule(r)[..]).unwrap();
        assert_eq!(parsed.rules.len(), 1);
        assert_eq!(
            parsed.rules[0].label_attrs,
            [
                ("label_attr".into(), "//app:one".into()),
                ("list_attr".into(), "//app:two".into()),
                ("list_attr".into(), "//app:three".into()),
                ("dict_unary".into(), "//app:val".into()),
                ("list_dict".into(), "//app:a".into()),
                ("list_dict".into(), "//app:b".into()),
                ("keyed".into(), "//app:key".into()),
            ]
        );
    }

    #[test]
    fn skips_nodep_and_valueless_label_attributes() {
        let mut nodep = attr("nodep_attr", AttributeType::Label);
        nodep.nodep = Some(true);
        nodep.string_value = Some("//app:hidden".to_string());
        let bare = attr("select_scalar", AttributeType::Label);
        let mut r = rule("//app:main");
        r.attribute = vec![nodep, bare];
        let parsed = parse_query(&wrap_rule(r)[..]).unwrap();
        assert!(parsed.rules[0].label_attrs.is_empty());
    }

    #[test]
    fn keeps_rule_class_location_outputs_and_source_files() {
        let mut r = rule("//app:é");
        r.rule_class = "cc_テスト".to_string();
        r.location = Some("/ws/app/BUILD:b:12:5".to_string());
        r.rule_output = vec!["//app:out.txt".to_string()];
        let bytes = proto::QueryResult {
            target: vec![
                proto::Target {
                    r#type: TargetType::Rule as i32,
                    rule: Some(r),
                    ..Default::default()
                },
                proto::Target {
                    r#type: TargetType::SourceFile as i32,
                    source_file: Some(proto::SourceFile {
                        name: "//app:input.txt".to_string(),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            ],
        }
        .encode_to_vec();
        let parsed = parse_query(&bytes[..]).unwrap();
        assert_eq!(parsed.rules.len(), 1);
        let rule = &parsed.rules[0];
        assert_eq!(rule.label, "//app:é");
        assert_eq!(rule.rule_class, "cc_テスト");
        assert_eq!(rule.location, Some(("/ws/app/BUILD:b".to_string(), 12)));
        assert_eq!(rule.outputs, ["//app:out.txt"]);
        assert_eq!(parsed.source_files, ["//app:input.txt"]);
    }

    #[test]
    fn rejects_malformed_and_truncated_bytes() {
        assert!(parse_query(&[0x0a, 0x80][..]).is_err());
        let bytes = wrap_rule(rule("//app:main"));
        assert!(parse_query(&bytes[..bytes.len() - 1][..]).is_err());
    }

    #[test]
    fn rejects_unknown_and_inconsistent_target_types() {
        let unknown = proto::QueryResult {
            target: vec![proto::Target {
                r#type: 999,
                ..Default::default()
            }],
        }
        .encode_to_vec();
        assert!(parse_query(&unknown[..]).is_err());

        let mismatched = proto::QueryResult {
            target: vec![proto::Target {
                r#type: TargetType::Rule as i32,
                source_file: Some(proto::SourceFile {
                    name: "//app:f".to_string(),
                    ..Default::default()
                }),
                ..Default::default()
            }],
        }
        .encode_to_vec();
        assert!(parse_query(&mismatched[..]).is_err());

        let missing = proto::QueryResult {
            target: vec![proto::Target {
                r#type: TargetType::Rule as i32,
                ..Default::default()
            }],
        }
        .encode_to_vec();
        assert!(parse_query(&missing[..]).is_err());
    }

    #[test]
    fn rejects_empty_rule_fields_and_unknown_attribute_types() {
        assert!(parse_query(&wrap_rule(rule(""))[..]).is_err());

        let mut classless = rule("//app:main");
        classless.rule_class = String::new();
        assert!(parse_query(&wrap_rule(classless)[..]).is_err());

        let mut r = rule("//app:main");
        r.attribute = vec![proto::Attribute {
            r#type: 999,
            ..attr("bad", AttributeType::Label)
        }];
        assert!(parse_query(&wrap_rule(r)[..]).is_err());

        let mut empty_name = attr("", AttributeType::Label);
        empty_name.string_value = Some("//app:x".to_string());
        let mut r = rule("//app:main");
        r.attribute = vec![empty_name];
        assert!(parse_query(&wrap_rule(r)[..]).is_err());

        let mut empty_label = attr("empty_label", AttributeType::Label);
        empty_label.string_value = Some(String::new());
        let mut r = rule("//app:main");
        r.attribute = vec![empty_label];
        assert!(parse_query(&wrap_rule(r)[..]).is_err());

        let mut r = rule("//app:main");
        r.rule_output = vec![String::new()];
        assert!(parse_query(&wrap_rule(r)[..]).is_err());
    }

    #[test]
    fn rejects_selector_list_attributes() {
        let mut r = rule("//app:main");
        r.attribute = vec![attr("select_attr", AttributeType::SelectorList)];
        assert!(parse_query(&wrap_rule(r)[..]).is_err());
    }

    struct Failing;

    impl Read for Failing {
        fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe))
        }
    }

    #[test]
    fn read_errors_surface() {
        assert!(parse_query(Failing).is_err());
    }
}
