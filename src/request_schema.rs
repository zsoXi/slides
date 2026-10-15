//! Form-oriented projection of the derived request schema. Legacy unions stay parser-only.

use serde_json::{Value, json};

pub(crate) fn form_schema() -> Value {
    let derived = serde_json::to_value(schemars::schema_for!(crate::Input)).unwrap_or(Value::Null);
    let mut result = expand(&derived, &derived);
    if let Some(variants) = result["oneOf"].as_array_mut() {
        for variant in variants {
            let op = variant["properties"]["op"]["const"]
                .as_str()
                .unwrap_or("")
                .to_owned();
            variant["title"] = json!(op);
            let properties = &mut variant["properties"];
            annotate_path(
                properties,
                "input",
                "input",
                match op.as_str() {
                    "convert" => &["pptx", "md", "markdown"],
                    "read" => &["pptx", "potx"],
                    _ => &["pptx"],
                },
            );
            annotate_path(
                properties,
                "output",
                "output",
                match op.as_str() {
                    "create" => &["pptx", "pdf"],
                    "convert" => &["pdf", "md", "pptx"],
                    _ => &["pptx"],
                },
            );
            annotate_path(properties, "template", "input", &["pptx", "potx"]);
            annotate_path(properties, "model", "input", &["xlsx"]);
            if let Some(theme) = properties.get_mut("theme") {
                theme["enum"] = json!(["light", "dark", "corporate", "creative"]);
            }
        }
    }
    result
}

fn annotate_path(properties: &mut Value, name: &str, direction: &str, formats: &[&str]) {
    if let Some(property) = properties.get_mut(name) {
        property["x-file"] = json!(direction);
        property[if direction == "input" {
            "x-accept"
        } else {
            "x-formats"
        }] = json!(formats);
    }
}

fn expand(node: &Value, root: &Value) -> Value {
    if let Some(reference) = node["$ref"].as_str() {
        let mut expanded = reference
            .strip_prefix('#')
            .and_then(|r| root.pointer(r))
            .map(|node| expand(node, root))
            .unwrap_or(Value::Null);
        for key in ["title", "description", "default"] {
            if let Some(value) = node.get(key) {
                expanded[key] = value.clone();
            }
        }
        return expanded;
    }
    // Optional objects and legacy named-or-bare series have a single canonical form.
    if let Some(variants) = node["anyOf"]
        .as_array()
        .or_else(|| node["oneOf"].as_array())
        && node.pointer("/oneOf/0/properties/op").is_none()
    {
        let selected = variants.iter().find(|v| v["type"] != "null");
        let mut expanded = selected.map(|v| expand(v, root)).unwrap_or(Value::Null);
        for key in ["title", "description"] {
            if let Some(value) = node.get(key) {
                expanded[key] = value.clone();
            }
        }
        return expanded;
    }
    let Some(object) = node.as_object() else {
        return node.clone();
    };
    let mut result = serde_json::Map::new();
    for (key, value) in object {
        match key.as_str() {
            "$schema" | "$defs" => {}
            "properties" => {
                let mut properties = serde_json::Map::new();
                for (name, value) in value.as_object().into_iter().flatten() {
                    let mut property = expand(value, root);
                    if property.get("description").is_none() {
                        property["description"] = json!(if name == "op" {
                            "Operation to perform."
                        } else {
                            name
                        });
                    }
                    property["title"] = json!(
                        property["description"]
                            .as_str()
                            .unwrap_or(name)
                            .lines()
                            .next()
                            .unwrap_or(name)
                            .trim_end_matches('.')
                    );
                    if name == "type"
                        && object
                            .get("properties")
                            .is_some_and(|p| p.get("series").is_some())
                    {
                        property["enum"] =
                            json!(["bar", "column", "line", "pie", "area", "scatter"]);
                    }
                    if name == "image" {
                        property["x-file"] = json!("input");
                        property["x-accept"] = json!(["png", "jpg", "jpeg"]);
                    }
                    properties.insert(name.clone(), property);
                }
                result.insert(key.clone(), Value::Object(properties));
            }
            "items" => {
                result.insert(key.clone(), expand(value, root));
            }
            "oneOf" => {
                result.insert(
                    key.clone(),
                    Value::Array(
                        value
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(|v| expand(v, root))
                            .collect(),
                    ),
                );
            }
            "type" if value.is_array() => {
                if let Some(kind) = value
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|v| **v != "null")
                {
                    result.insert(key.clone(), kind.clone());
                }
            }
            "default" if value.is_null() => {}
            _ => {
                result.insert(key.clone(), value.clone());
            }
        }
    }
    Value::Object(result)
}
