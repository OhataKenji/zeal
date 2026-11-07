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

    fn wrap_target(target: proto::Target) -> Vec<u8> {
        proto::QueryResult {
            target: vec![target],
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

    fn parse_attributes(attributes: Vec<proto::Attribute>) -> Result<ParsedQuery, String> {
        let bytes = wrap_rule(proto::Rule {
            attribute: attributes,
            ..rule("//app:main")
        });
        parse_query(bytes.as_slice())
    }

    #[test]
    fn scalar_label_keeps_its_attribute_name() {
        let parsed = parse_attributes(vec![proto::Attribute {
            string_value: Some("//app:dep".into()),
            ..attr("deps", AttributeType::Label)
        }])
        .unwrap();
        assert_eq!(
            parsed.rules[0].label_attrs,
            [("deps".into(), "//app:dep".into())]
        );
    }

    #[test]
    fn valueless_scalar_label_produces_no_relationship() {
        let parsed = parse_attributes(vec![attr("dep", AttributeType::Label)]).unwrap();
        assert!(parsed.rules[0].label_attrs.is_empty());
    }

    #[test]
    fn label_list_collects_each_value_under_the_attribute_name() {
        let parsed = parse_attributes(vec![proto::Attribute {
            string_list_value: vec!["//app:a".into(), "//app:b".into()],
            ..attr("deps", AttributeType::LabelList)
        }])
        .unwrap();
        assert_eq!(
            parsed.rules[0].label_attrs,
            [
                ("deps".into(), "//app:a".into()),
                ("deps".into(), "//app:b".into()),
            ]
        );
    }

    #[test]
    fn label_dict_unary_collects_values_not_keys() {
        let parsed = parse_attributes(vec![proto::Attribute {
            label_dict_unary_value: vec![proto::LabelDictUnaryEntry {
                key: "//app:key".into(),
                value: "//app:value".into(),
            }],
            ..attr("deps", AttributeType::LabelDictUnary)
        }])
        .unwrap();
        assert_eq!(
            parsed.rules[0].label_attrs,
            [("deps".into(), "//app:value".into())]
        );
    }

    #[test]
    fn label_list_dict_flattens_entry_values() {
        let parsed = parse_attributes(vec![proto::Attribute {
            label_list_dict_value: vec![
                proto::LabelListDictEntry {
                    key: "//app:k1".into(),
                    value: vec!["//app:a".into(), "//app:b".into()],
                },
                proto::LabelListDictEntry {
                    key: "//app:k2".into(),
                    value: vec!["//app:c".into()],
                },
            ],
            ..attr("deps", AttributeType::LabelListDict)
        }])
        .unwrap();
        assert_eq!(
            parsed.rules[0].label_attrs,
            [
                ("deps".into(), "//app:a".into()),
                ("deps".into(), "//app:b".into()),
                ("deps".into(), "//app:c".into()),
            ]
        );
    }

    #[test]
    fn label_keyed_string_dict_collects_keys_not_values() {
        let parsed = parse_attributes(vec![proto::Attribute {
            label_keyed_string_dict_value: vec![
                proto::LabelKeyedStringDictEntry {
                    key: "//app:key1".into(),
                    value: "first".into(),
                },
                proto::LabelKeyedStringDictEntry {
                    key: "//app:key2".into(),
                    value: "second".into(),
                },
            ],
            ..attr("deps", AttributeType::LabelKeyedStringDict)
        }])
        .unwrap();
        assert_eq!(
            parsed.rules[0].label_attrs,
            [
                ("deps".into(), "//app:key1".into()),
                ("deps".into(), "//app:key2".into()),
            ]
        );
    }

    #[test]
    fn ignores_string_attributes() {
        let parsed = parse_attributes(vec![proto::Attribute {
            string_value: Some("//app:not_a_label".into()),
            ..attr("string_attr", AttributeType::String)
        }])
        .unwrap();
        assert!(parsed.rules[0].label_attrs.is_empty());
    }

    #[test]
    fn ignores_string_list_attributes() {
        let parsed = parse_attributes(vec![proto::Attribute {
            string_list_value: vec!["//app:also_not".into()],
            ..attr("string_list", AttributeType::StringList)
        }])
        .unwrap();
        assert!(parsed.rules[0].label_attrs.is_empty());
    }

    #[test]
    fn ignores_output_attributes() {
        let parsed = parse_attributes(vec![proto::Attribute {
            string_value: Some("//app:out1".into()),
            ..attr("output_attr", AttributeType::Output)
        }])
        .unwrap();
        assert!(parsed.rules[0].label_attrs.is_empty());
    }

    #[test]
    fn ignores_output_list_attributes() {
        let parsed = parse_attributes(vec![proto::Attribute {
            string_list_value: vec!["//app:out2".into()],
            ..attr("output_list", AttributeType::OutputList)
        }])
        .unwrap();
        assert!(parsed.rules[0].label_attrs.is_empty());
    }

    #[test]
    fn skips_nodep_attributes() {
        let parsed = parse_attributes(vec![proto::Attribute {
            nodep: Some(true),
            string_value: Some("//app:hidden".into()),
            ..attr("hidden", AttributeType::Label)
        }])
        .unwrap();
        assert!(parsed.rules[0].label_attrs.is_empty());
    }

    #[test]
    fn ignores_flattened_rule_inputs() {
        let bytes = wrap_rule(proto::Rule {
            rule_input: vec!["//app:input.txt".into()],
            ..rule("//app:main")
        });
        let parsed = parse_query(&bytes[..]).unwrap();
        assert!(parsed.rules[0].label_attrs.is_empty());
        assert!(parsed.rules[0].outputs.is_empty());
    }

    #[test]
    fn keeps_unicode_label_and_rule_class() {
        let bytes = wrap_rule(proto::Rule {
            rule_class: "cc_テスト".into(),
            ..rule("//app:é")
        });
        let parsed = parse_query(&bytes[..]).unwrap();
        assert_eq!(parsed.rules[0].label, "//app:é");
        assert_eq!(parsed.rules[0].rule_class, "cc_テスト");
    }

    #[test]
    fn parses_location_file_and_line() {
        let bytes = wrap_rule(proto::Rule {
            location: Some("/ws/app/BUILD:b:12:5".into()),
            ..rule("//app:main")
        });
        let parsed = parse_query(&bytes[..]).unwrap();
        assert_eq!(
            parsed.rules[0].location,
            Some(("/ws/app/BUILD:b".to_string(), 12))
        );
    }

    #[test]
    fn keeps_declared_rule_outputs() {
        let bytes = wrap_rule(proto::Rule {
            rule_output: vec!["//app:out.txt".into()],
            ..rule("//app:main")
        });
        let parsed = parse_query(&bytes[..]).unwrap();
        assert_eq!(parsed.rules[0].outputs, ["//app:out.txt"]);
    }

    #[test]
    fn collects_source_file_names() {
        let bytes = wrap_target(proto::Target {
            r#type: TargetType::SourceFile as i32,
            source_file: Some(proto::SourceFile {
                name: "//app:input.txt".into(),
                ..Default::default()
            }),
            ..Default::default()
        });
        let parsed = parse_query(&bytes[..]).unwrap();
        assert_eq!(parsed.source_files, ["//app:input.txt"]);
        assert!(parsed.rules.is_empty());
    }

    #[test]
    fn ignores_generated_file_targets() {
        let bytes = wrap_target(proto::Target {
            r#type: TargetType::GeneratedFile as i32,
            generated_file: Some(proto::GeneratedFile {
                name: "//app:gen.txt".into(),
                generating_rule: "//app:gen".into(),
                ..Default::default()
            }),
            ..Default::default()
        });
        let parsed = parse_query(&bytes[..]).unwrap();
        assert!(parsed.rules.is_empty());
        assert!(parsed.source_files.is_empty());
    }

    #[test]
    fn rejects_unknown_attribute_type() {
        assert!(
            parse_attributes(vec![proto::Attribute {
                r#type: 999,
                ..attr("deps", AttributeType::Label)
            }])
            .is_err()
        );
    }

    #[test]
    fn rejects_unflattened_selector_attributes() {
        let parsed = parse_attributes(vec![proto::Attribute {
            selector_list: Some(proto::attribute::SelectorList::default()),
            ..attr("select_attr", AttributeType::SelectorList)
        }]);
        assert!(parsed.is_err());
    }

    #[test]
    fn rejects_empty_rule_name() {
        let bytes = wrap_rule(proto::Rule {
            name: String::new(),
            ..rule("//app:main")
        });
        assert!(parse_query(&bytes[..]).is_err());
    }

    #[test]
    fn rejects_empty_rule_class() {
        let bytes = wrap_rule(proto::Rule {
            rule_class: String::new(),
            ..rule("//app:main")
        });
        assert!(parse_query(&bytes[..]).is_err());
    }

    #[test]
    fn rejects_source_file_without_name() {
        let bytes = wrap_target(proto::Target {
            r#type: TargetType::SourceFile as i32,
            source_file: Some(proto::SourceFile::default()),
            ..Default::default()
        });
        assert!(parse_query(&bytes[..]).is_err());
    }

    #[test]
    fn rejects_empty_output_label() {
        let bytes = wrap_rule(proto::Rule {
            rule_output: vec![String::new()],
            ..rule("//app:main")
        });
        assert!(parse_query(&bytes[..]).is_err());
    }

    #[test]
    fn rejects_empty_attribute_name() {
        assert!(
            parse_attributes(vec![proto::Attribute {
                string_value: Some("//app:x".into()),
                ..attr("", AttributeType::Label)
            }])
            .is_err()
        );
    }

    #[test]
    fn rejects_empty_attribute_label() {
        assert!(
            parse_attributes(vec![proto::Attribute {
                string_value: Some(String::new()),
                ..attr("deps", AttributeType::Label)
            }])
            .is_err()
        );
    }

    #[test]
    fn rejects_unknown_target_type() {
        let bytes = wrap_target(proto::Target {
            r#type: 999,
            ..Default::default()
        });
        assert!(parse_query(&bytes[..]).is_err());
    }

    #[test]
    fn rejects_target_payload_mismatched_with_type() {
        let bytes = wrap_target(proto::Target {
            r#type: TargetType::Rule as i32,
            source_file: Some(proto::SourceFile {
                name: "//app:f".into(),
                ..Default::default()
            }),
            ..Default::default()
        });
        assert!(parse_query(&bytes[..]).is_err());
    }

    #[test]
    fn rejects_target_without_payload() {
        let bytes = wrap_target(proto::Target {
            r#type: TargetType::Rule as i32,
            ..Default::default()
        });
        assert!(parse_query(&bytes[..]).is_err());
    }

    #[test]
    fn rejects_target_with_multiple_payloads() {
        let bytes = wrap_target(proto::Target {
            r#type: TargetType::Rule as i32,
            rule: Some(rule("//app:main")),
            source_file: Some(proto::SourceFile {
                name: "//app:f".into(),
                ..Default::default()
            }),
            ..Default::default()
        });
        assert!(parse_query(&bytes[..]).is_err());
    }

    #[test]
    fn rejects_malformed_bytes() {
        assert!(parse_query(&[0x0a, 0x80][..]).is_err());
    }

    #[test]
    fn rejects_truncated_output() {
        let bytes = wrap_rule(rule("//app:main"));
        assert!(parse_query(&bytes[..bytes.len() - 1][..]).is_err());
    }

    #[test]
    fn accepts_empty_query_result() {
        let bytes = proto::QueryResult::default().encode_to_vec();
        let parsed = parse_query(&bytes[..]).unwrap();
        assert!(parsed.rules.is_empty());
        assert!(parsed.source_files.is_empty());
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

    #[test]
    fn unknown_wire_fields_are_skipped() {
        let mut bytes = wrap_rule(rule("//app:main"));
        // Unknown field 100, varint 1, appended to the QueryResult message.
        bytes.extend_from_slice(&[0xa0, 0x06, 0x01]);
        assert!(parse_query(&bytes[..]).is_ok());
    }

    fn bazel_fixture() -> ParsedQuery {
        parse_query(&include_bytes!("../tests/fixtures/query-proto/query.pb")[..]).unwrap()
    }

    fn main_rule(query: &ParsedQuery) -> &RawRule {
        query
            .rules
            .iter()
            .find(|rule| rule.label == "//:main")
            .unwrap()
    }

    fn attribute_values<'a>(rule: &'a RawRule, name: &str) -> Vec<&'a str> {
        let mut values: Vec<&str> = rule
            .label_attrs
            .iter()
            .filter(|(attr, _)| attr == name)
            .map(|(_, value)| value.as_str())
            .collect();
        values.sort_unstable();
        values
    }

    #[test]
    fn bazel_fixture_scalar_label_keeps_its_attribute_name() {
        let query = bazel_fixture();
        assert_eq!(
            attribute_values(main_rule(&query), "plain_dep"),
            ["//:input.txt"]
        );
    }

    #[test]
    fn bazel_fixture_valueless_scalar_select_produces_no_values() {
        let query = bazel_fixture();
        assert!(attribute_values(main_rule(&query), "dep").is_empty());
    }

    #[test]
    fn bazel_fixture_list_select_flattens_both_branches() {
        let query = bazel_fixture();
        assert_eq!(
            attribute_values(main_rule(&query), "deps"),
            ["//:input.txt", "//:other.txt"]
        );
    }

    #[test]
    fn bazel_fixture_label_keyed_dict_collects_keys() {
        let query = bazel_fixture();
        assert_eq!(
            attribute_values(main_rule(&query), "mapping"),
            ["//:input.txt", "//:other.txt"]
        );
    }

    #[test]
    fn bazel_fixture_keeps_declared_output() {
        let query = bazel_fixture();
        assert_eq!(main_rule(&query).outputs, ["//:result.txt"]);
    }

    #[test]
    fn bazel_fixture_collects_source_files() {
        let query = bazel_fixture();
        let mut files = query.source_files.clone();
        files.sort();
        assert_eq!(files, ["//:input.txt", "//:other.txt"]);
    }

    #[test]
    fn bazel_fixture_projects_files_through_snapshot() {
        use crate::snapshot::{self, FileEntry, Status};
        let snapshot = snapshot::build(&bazel_fixture(), Status::Ok);
        assert!(snapshot.edges.is_empty());
        assert_eq!(
            snapshot.files,
            [
                FileEntry {
                    path: "input.txt".into(),
                    owner: "//:main".into(),
                    role: "deps".into(),
                },
                FileEntry {
                    path: "input.txt".into(),
                    owner: "//:main".into(),
                    role: "mapping".into(),
                },
                FileEntry {
                    path: "input.txt".into(),
                    owner: "//:main".into(),
                    role: "plain_dep".into(),
                },
                FileEntry {
                    path: "other.txt".into(),
                    owner: "//:main".into(),
                    role: "deps".into(),
                },
                FileEntry {
                    path: "other.txt".into(),
                    owner: "//:main".into(),
                    role: "mapping".into(),
                },
                FileEntry {
                    path: "result.txt".into(),
                    owner: "//:main".into(),
                    role: "out".into(),
                },
            ]
        );
    }
}
