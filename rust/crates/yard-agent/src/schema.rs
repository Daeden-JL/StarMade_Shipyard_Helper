//! The small part of JSON Schema the tools are described in, and checking a
//! model's arguments against it.
//!
//! Models, local ones above all, send numbers as strings, positions as
//! objects and lists as text. What can be read without guessing is put right
//! before it is checked; everything else comes back as a sentence the model
//! can act on.

use serde_json::{json, Map, Value};

/// An object with the given properties. They are listed to the model in
/// alphabetical order, the same on every request, which keeps the prompt
/// cache valid.
pub fn object(properties: Vec<(&str, Value)>, required: &[&str]) -> Value {
    let map: Map<String, Value> = properties.into_iter().map(|(name, schema)| (name.to_string(), schema)).collect();
    json!({ "type": "object", "properties": map, "required": required, "additionalProperties": false })
}

pub fn string(description: &str) -> Value {
    json!({ "type": "string", "description": description })
}

pub fn integer(description: &str) -> Value {
    json!({ "type": "integer", "description": description })
}

/// A whole number within bounds, both included.
pub fn integer_in(description: &str, min: i64, max: i64) -> Value {
    json!({ "type": "integer", "description": description, "minimum": min, "maximum": max })
}

pub fn number(description: &str) -> Value {
    json!({ "type": "number", "description": description })
}

pub fn boolean(description: &str) -> Value {
    json!({ "type": "boolean", "description": description })
}

/// A string that must be one of the listed words.
pub fn choice(description: &str, options: &[&str]) -> Value {
    json!({ "type": "string", "description": description, "enum": options })
}

/// A block position `[x, y, z]`.
pub fn position(description: &str) -> Value {
    json!({ "type": "array", "description": format!("{description} [x, y, z], whole numbers."), "items": { "type": "integer" }, "minItems": 3, "maxItems": 3 })
}

pub fn array(description: &str, items: Value, max_items: usize) -> Value {
    json!({ "type": "array", "description": description, "items": items, "maxItems": max_items })
}

/// Checks `value` against `schema`, first putting right what a model
/// commonly gets wrong in form only. The error names the argument.
pub fn validate(schema: &Value, value: &mut Value) -> Result<(), String> {
    check(schema, value, "")
}

fn check(schema: &Value, value: &mut Value, path: &str) -> Result<(), String> {
    let name = if path.is_empty() { "the arguments".to_string() } else { format!("\"{path}\"") };
    let kind = schema.get("type").and_then(Value::as_str).unwrap_or("");
    coerce(kind, schema, value);
    match kind {
        "object" => check_object(schema, value, path, &name),
        "array" => check_array(schema, value, path, &name),
        "string" => {
            let Some(text) = value.as_str() else { return Err(format!("{name} must be a string, not {}.", describe(value))) };
            match schema.get("enum").and_then(Value::as_array) {
                Some(options) if !options.iter().any(|o| o.as_str() == Some(text)) => {
                    // Case and stray spaces are not worth a round trip.
                    let fixed = options.iter().filter_map(Value::as_str).find(|o| o.eq_ignore_ascii_case(text.trim()));
                    match fixed {
                        Some(option) => {
                            *value = Value::String(option.to_string());
                            Ok(())
                        }
                        None => Err(format!("{name} must be one of {}, not \"{text}\".", list(options))),
                    }
                }
                _ => Ok(()),
            }
        }
        "integer" => {
            let Some(n) = value.as_i64() else { return Err(format!("{name} must be a whole number, not {}.", describe(value))) };
            in_range(schema, n as f64, &name)
        }
        "number" => {
            let Some(n) = value.as_f64() else { return Err(format!("{name} must be a number, not {}.", describe(value))) };
            in_range(schema, n, &name)
        }
        "boolean" if !value.is_boolean() => Err(format!("{name} must be true or false, not {}.", describe(value))),
        _ => Ok(()),
    }
}

fn check_object(schema: &Value, value: &mut Value, path: &str, name: &str) -> Result<(), String> {
    let Some(given) = value.as_object_mut() else { return Err(format!("{name} must be an object, not {}.", describe(value))) };
    let empty = Map::new();
    let properties = schema.get("properties").and_then(Value::as_object).unwrap_or(&empty);
    // A null stands for "not given".
    given.retain(|_, v| !v.is_null());
    if let Some(unknown) = given.keys().find(|k| !properties.contains_key(*k)) {
        let allowed: Vec<&str> = properties.keys().map(String::as_str).collect();
        return Err(format!("Unknown argument \"{}\"{}. Allowed: {}.", unknown, within(path), allowed.join(", ")));
    }
    for required in schema.get("required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
        if !given.contains_key(required) {
            return Err(format!("Missing argument \"{required}\"{}.", within(path)));
        }
    }
    for (key, child) in given.iter_mut() {
        let child_path = if path.is_empty() { key.clone() } else { format!("{path}.{key}") };
        check(&properties[key], child, &child_path)?;
    }
    Ok(())
}

fn check_array(schema: &Value, value: &mut Value, path: &str, name: &str) -> Result<(), String> {
    let Some(items) = value.as_array_mut() else { return Err(format!("{name} must be a list, not {}.", describe(value))) };
    let count = items.len() as u64;
    let (min, max) = (schema.get("minItems").and_then(Value::as_u64), schema.get("maxItems").and_then(Value::as_u64));
    if min.is_some() && min == max && Some(count) != min {
        return Err(format!("{name} must have exactly {} values, not {count}.", min.unwrap_or(0)));
    }
    if min.is_some_and(|m| count < m) {
        return Err(format!("{name} needs at least {} values, not {count}.", min.unwrap_or(0)));
    }
    if max.is_some_and(|m| count > m) {
        return Err(format!("{name} may have at most {} values, not {count}. Split the work over several calls.", max.unwrap_or(0)));
    }
    if let Some(item_schema) = schema.get("items") {
        for (i, item) in items.iter_mut().enumerate() {
            check(item_schema, item, &format!("{path}[{i}]"))?;
        }
    }
    Ok(())
}

fn in_range(schema: &Value, n: f64, name: &str) -> Result<(), String> {
    let (min, max) = (schema.get("minimum").and_then(Value::as_f64), schema.get("maximum").and_then(Value::as_f64));
    match (min, max) {
        (Some(min), _) if n < min => Err(format!("{name} must be at least {min}, not {n}.")),
        (_, Some(max)) if n > max => Err(format!("{name} must be at most {max}, not {n}.")),
        _ => Ok(()),
    }
}

/// Rewrites a value that is right in content and wrong in form.
fn coerce(kind: &str, schema: &Value, value: &mut Value) {
    let fixed = match (kind, &*value) {
        ("integer", Value::String(s)) => s.trim().parse::<i64>().ok().map(Value::from),
        // 3.0 for 3, as some models write every number.
        ("integer", Value::Number(n)) if !n.is_i64() => n.as_f64().filter(|f| f.fract() == 0.0 && f.abs() < 1e15).map(|f| Value::from(f as i64)),
        ("number", Value::String(s)) => s.trim().parse::<f64>().ok().map(Value::from),
        ("boolean", Value::String(s)) => match s.trim().to_ascii_lowercase().as_str() {
            "true" | "yes" => Some(Value::Bool(true)),
            "false" | "no" => Some(Value::Bool(false)),
            _ => None,
        },
        ("string", Value::Number(n)) => Some(Value::String(n.to_string())),
        // A list or object written out as text.
        ("array" | "object", Value::String(s)) => {
            let text = s.trim();
            let parsed = serde_json::from_str::<Value>(text).ok().filter(|v| v.is_array() || v.is_object());
            parsed.or_else(|| numbers_in(text))
        }
        // {"x": 1, "y": 2, "z": 3} for a position.
        ("array", Value::Object(map)) if is_position(schema) => match (map.get("x"), map.get("y"), map.get("z")) {
            (Some(x), Some(y), Some(z)) if map.len() == 3 => Some(Value::Array(vec![x.clone(), y.clone(), z.clone()])),
            _ => None,
        },
        _ => None,
    };
    if let Some(fixed) = fixed {
        *value = fixed;
    }
}

fn is_position(schema: &Value) -> bool {
    schema.get("minItems").and_then(Value::as_u64) == Some(3) && schema.get("maxItems").and_then(Value::as_u64) == Some(3)
}

/// "1, 2, 3" or "(1 2 3)" as a list of numbers.
fn numbers_in(text: &str) -> Option<Value> {
    let inner = text.trim_matches(|c| "()[] ".contains(c));
    let parts: Vec<&str> = inner.split(|c: char| c == ',' || c.is_whitespace()).filter(|p| !p.is_empty()).collect();
    let numbers: Option<Vec<Value>> = parts.iter().map(|p| p.parse::<i64>().ok().map(Value::from)).collect();
    numbers.filter(|n| !n.is_empty()).map(Value::Array)
}

fn within(path: &str) -> String {
    if path.is_empty() { String::new() } else { format!(" in \"{path}\"") }
}

fn list(options: &[Value]) -> String {
    options.iter().filter_map(Value::as_str).map(|o| format!("\"{o}\"")).collect::<Vec<_>>().join(", ")
}

/// A value in a few words, for error messages.
fn describe(value: &Value) -> String {
    let text = value.to_string();
    let short: String = text.chars().take(60).collect();
    if short.len() < text.len() { format!("{short}...") } else { short }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fill_schema() -> Value {
        object(
            vec![
                ("from", position("One corner.")),
                ("block", string("Block name.")),
                ("mode", choice("How to fill.", &["solid", "hollow"])),
                ("thickness", integer_in("Wall thickness.", 1, 8)),
                ("hollow", boolean("Shell only.")),
                ("radius", number("Radius.")),
                ("cells", array("Cells.", object(vec![("pos", position("Where."))], &["pos"]), 2)),
            ],
            &["from", "block"],
        )
    }

    fn checked(mut value: Value) -> Result<Value, String> {
        validate(&fill_schema(), &mut value).map(|()| value)
    }

    #[test]
    fn well_formed_arguments_pass_unchanged() {
        let args = json!({"from": [1, -2, 3], "block": "Grey Hull", "mode": "hollow", "thickness": 2, "hollow": true, "radius": 2.5});
        assert_eq!(checked(args.clone()).unwrap(), args);
    }

    #[test]
    fn missing_unknown_and_mistyped_arguments_are_named() {
        assert_eq!(checked(json!({"from": [0, 0, 0]})).unwrap_err(), "Missing argument \"block\".");
        let unknown = checked(json!({"from": [0, 0, 0], "block": "x", "colour": "red"})).unwrap_err();
        assert!(unknown.starts_with("Unknown argument \"colour\". Allowed: "), "{unknown}");
        assert!(unknown.contains("block, cells, from"), "{unknown}");
        assert_eq!(checked(json!({"from": [0, 0], "block": "x"})).unwrap_err(), "\"from\" must have exactly 3 values, not 2.");
        assert_eq!(checked(json!({"from": [0, "a", 0], "block": "x"})).unwrap_err(), "\"from[1]\" must be a whole number, not \"a\".");
        assert_eq!(checked(json!({"from": [0, 0, 0], "block": "x", "mode": "striped"})).unwrap_err(), "\"mode\" must be one of \"solid\", \"hollow\", not \"striped\".");
        assert_eq!(checked(json!({"from": [0, 0, 0], "block": "x", "thickness": 9})).unwrap_err(), "\"thickness\" must be at most 8, not 9.");
        assert_eq!(checked(json!({"from": [0, 0, 0], "block": "x", "hollow": "maybe"})).unwrap_err(), "\"hollow\" must be true or false, not \"maybe\".");
        assert_eq!(checked(json!("fill it")).unwrap_err(), "the arguments must be an object, not \"fill it\".");
    }

    #[test]
    fn nested_lists_are_checked_and_capped() {
        let nested = checked(json!({"from": [0, 0, 0], "block": "x", "cells": [{"pos": [1, 2, 3]}, {}]})).unwrap_err();
        assert_eq!(nested, "Missing argument \"pos\" in \"cells[1]\".");
        let many = checked(json!({"from": [0, 0, 0], "block": "x", "cells": [{"pos": [1, 2, 3]}, {"pos": [1, 2, 3]}, {"pos": [1, 2, 3]}]})).unwrap_err();
        assert!(many.starts_with("\"cells\" may have at most 2 values, not 3."), "{many}");
    }

    #[test]
    fn values_right_in_content_and_wrong_in_form_are_put_right() {
        let fixed = checked(json!({
            "from": {"x": 1, "y": 2, "z": 3}, "block": 598, "mode": "Hollow ", "thickness": "3", "hollow": "true", "radius": "2.5", "cells": null
        }))
        .unwrap();
        assert_eq!(fixed, json!({"from": [1, 2, 3], "block": "598", "mode": "hollow", "thickness": 3, "hollow": true, "radius": 2.5}));
        assert_eq!(checked(json!({"from": "[4, 5, 6]", "block": "x"})).unwrap()["from"], json!([4, 5, 6]));
        assert_eq!(checked(json!({"from": "(4, -5, 6)", "block": "x"})).unwrap()["from"], json!([4, -5, 6]));
        assert_eq!(checked(json!({"from": [1.0, 2.0, 3.0], "block": "x"})).unwrap()["from"], json!([1, 2, 3]));
        assert!(checked(json!({"from": [1.5, 2, 3], "block": "x"})).is_err());
    }
}
