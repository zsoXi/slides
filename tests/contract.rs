//! Machine integration contract, including recursive form-schema validation.
use serde_json::{Value, json};
use std::io::Write;
use std::process::{Command, Stdio};
type Result = std::result::Result<(), Box<dyn std::error::Error>>;

fn walk(node: &Value, path: &str) {
    let allowed = [
        "type",
        "enum",
        "const",
        "default",
        "title",
        "description",
        "properties",
        "required",
        "items",
        "additionalProperties",
        "format",
        "pattern",
        "minLength",
        "maxLength",
        "minimum",
        "maximum",
        "minItems",
        "maxItems",
        "x-file",
        "x-accept",
        "x-formats",
    ];
    if let Some(object) = node.as_object() {
        for key in object.keys() {
            assert!(
                allowed.contains(&key.as_str()),
                "unsupported keyword {path}/{key}"
            );
        }
        if let Some(properties) = node["properties"].as_object() {
            for (name, property) in properties {
                assert!(
                    property["title"].as_str().is_some_and(|s| !s.is_empty()),
                    "title {path}/{name}"
                );
                assert!(
                    property["description"]
                        .as_str()
                        .is_some_and(|s| !s.is_empty()),
                    "description {path}/{name}"
                );
                if ["input", "output", "template", "model", "image"].contains(&name.as_str()) {
                    let direction = if name == "output" { "output" } else { "input" };
                    assert_eq!(
                        property["x-file"], direction,
                        "path annotation {path}/{name}"
                    );
                    assert!(
                        property[if direction == "output" {
                            "x-formats"
                        } else {
                            "x-accept"
                        }]
                        .as_array()
                        .is_some_and(|a| !a.is_empty())
                    );
                }
                walk(property, &format!("{path}/{name}"));
            }
        }
        if let Some(items) = node.get("items") {
            walk(items, &format!("{path}/items"));
        }
    }
}
#[test]
fn schema_is_form_friendly_and_paths_are_annotated_recursively() -> Result {
    let schema = slides::schema();
    let variants = schema["oneOf"].as_array().ok_or("operation variants")?;
    assert_eq!(variants.len(), 5);
    for variant in variants {
        assert!(variant["title"].is_string());
        assert!(variant["description"].is_string());
        walk(variant, "op");
    }
    let cli = Command::new(env!("CARGO_BIN_EXE_slides"))
        .arg("--schema")
        .output()?;
    assert!(cli.status.success());
    assert_eq!(serde_json::from_slice::<Value>(&cli.stdout)?, schema);
    Ok(())
}
#[test]
fn docs_messages_match_every_catalogue_template_and_parameter() -> Result {
    let out = Command::new(env!("CARGO_BIN_EXE_slides"))
        .args(["docs", "--messages"])
        .output()?;
    assert!(out.status.success());
    let doc: Value = serde_json::from_slice(&out.stdout)?;
    assert_eq!(doc, slides::messages::catalogue());
    let top = Command::new(env!("CARGO_BIN_EXE_slides"))
        .arg("--messages")
        .output()?;
    assert!(top.status.success());
    assert_eq!(serde_json::from_slice::<Value>(&top.stdout)?, doc);
    assert_eq!(doc["tool"], slides::NAME);
    assert_eq!(doc["version"], env!("CARGO_PKG_VERSION"));
    let entries = doc["messages"].as_array().ok_or("entries")?;
    assert_eq!(entries.len(), slides::MessageCode::ALL.len());
    let mut codes = std::collections::BTreeSet::new();
    for (code, entry) in slides::MessageCode::ALL.iter().zip(entries) {
        assert!(codes.insert(entry["code"].as_str().ok_or("code")?));
        assert_eq!(entry["code"], code.as_str());
        assert!(
            ["summary", "warning", "error", "finding", "help"]
                .contains(&entry["kind"].as_str().ok_or("kind")?)
        );
        let params: Vec<_> = entry["params"]
            .as_array()
            .ok_or("params")?
            .iter()
            .map(|p| Ok((p.as_str().ok_or("parameter")?.to_owned(), "example".into())))
            .collect::<std::result::Result<_, Box<dyn std::error::Error>>>()?;
        let rendered = slides::messages::render(*code, &params, "en");
        assert!(!rendered.contains('{'), "unfilled {code:?}");
        let mut template = entry["template"].as_str().ok_or("template")?.to_owned();
        for (name, value) in params {
            template = template.replace(&format!("{{{name}}}"), &value);
        }
        assert_eq!(rendered, template);
    }
    Ok(())
}
#[test]
fn validation_uses_pointer_fields_and_enum_arrays_and_flat_envelopes() -> Result {
    let dir = tempfile::tempdir()?;
    for (request, field, allowed) in [
        (json!({"op":"read"}), "/input", None),
        (json!({"op":"read","input":"missing.pptx"}), "/input", None),
        (
            json!({"op":"create","output":"unsupported.txt","slides":[{"title":"A"}]}),
            "/output",
            None,
        ),
        (
            json!({"op":"create","output":"../escape.pptx","slides":[{"title":"A"}]}),
            "/output",
            None,
        ),
        (
            json!({"op":"unknown"}),
            "/op",
            Some(json!(["create", "read", "edit", "convert", "check"])),
        ),
        (
            json!({"op":"read","input":"x.pptx","representation":"unknown"}),
            "/representation",
            Some(json!(["text", "data", "both"])),
        ),
        (
            json!({"op":"create","output":"x.pptx","theme":"unknown","slides":[{"title":"A"}]}),
            "/theme",
            Some(json!(["light", "dark", "corporate", "creative"])),
        ),
        (
            json!({"op":"create","output":"x.pptx","slides":[{"title":"A"},{"chart":{"type":"wrong","labels":["A"],"series":[[1]]}}]}),
            "/slides/1/chart/type",
            Some(json!(["bar", "column", "line", "pie", "area", "scatter"])),
        ),
        (
            json!({"op":"create","output":"x.pptx","slides":[{"chart":{"type":"bar","labels":[],"series":[{"name":"A","values":[],"a~/b":0}]}}]}),
            "/slides/0/chart/series/0/a~0~1b",
            None,
        ),
        (
            json!({"op":"create","output":"x.pptx","markdown":"## Chart\n```chart\n{\"type\":\"wrong\",\"labels\":[\"A\"],\"series\":[[1]]}\n```"}),
            "/markdown",
            Some(json!(["bar", "column", "line", "pie", "area", "scatter"])),
        ),
        (
            json!({"op":"create","output":"x.pptx","slides":[{"title":"A"},{"image":"missing.png"}]}),
            "/slides/1/image",
            None,
        ),
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_slides"))
            .current_dir(dir.path())
            .arg(request.to_string())
            .output()?;
        assert_eq!(out.status.code(), Some(1));
        assert!(out.stderr.is_empty());
        let value: Value = serde_json::from_slice(&out.stdout)?;
        assert_eq!(value.as_object().ok_or("error envelope")?.len(), 5);
        assert_eq!(value["ok"], false);
        assert_eq!(value["warnings"], json!([]));
        assert_eq!(value["summary_params"]["field"], field, "{value}");
        if let Some(allowed) = allowed {
            assert_eq!(value["summary_params"]["allowed"], allowed);
        }
    }
    Ok(())
}
#[test]
fn long_unknown_property_names_keep_a_bounded_valid_parent_pointer() -> Result {
    let mut slide = json!({});
    slide["~/".repeat(1000)] = json!(true);
    let request = json!({"op":"create","output":"x.pptx","slides":[slide]});
    let error = slides::parse_input(&request.to_string())
        .err()
        .ok_or("unknown field accepted")?;
    assert_eq!(error.params()["field"], "/slides/0");
    assert_eq!(error.params()["pointer"], "/slides/0");
    assert!(serde_json::to_vec(&error)?.len() < 2048);
    Ok(())
}

#[test]
fn stdin_and_argument_produce_identical_success_and_error_json() -> Result {
    let dir = tempfile::tempdir()?;
    for request in [
        r#"{"op":"create","output":"x.pptx","slides":[{"title":"Review 2027"}]}"#,
        "garbage",
        r#"{"op":"read"}"#,
    ] {
        let arg = Command::new(env!("CARGO_BIN_EXE_slides"))
            .current_dir(dir.path())
            .arg(request)
            .output()?;
        let mut child = Command::new(env!("CARGO_BIN_EXE_slides"))
            .current_dir(dir.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
        child
            .stdin
            .take()
            .ok_or("stdin")?
            .write_all(request.as_bytes())?;
        let stdin = child.wait_with_output()?;
        assert_eq!(arg.status.code(), stdin.status.code());
        assert_eq!(arg.stdout, stdin.stdout);
        assert!(
            serde_json::from_slice::<Value>(&arg.stdout)?
                .get("warnings")
                .is_some()
        );
    }
    Ok(())
}
