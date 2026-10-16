//! Normalised log record: an ordered list of flat `key -> Value` fields.

use std::fmt;

/// A scalar field value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Str(String),
    Num(f64),
    Bool(bool),
    Null,
}

impl Value {
    /// Infer a typed value from untyped text (logfmt / nginx fields).
    pub fn infer(s: &str) -> Value {
        match s {
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            "" | "-" => Value::Null,
            _ => match s.parse::<f64>() {
                Ok(n) if n.is_finite() => Value::Num(n),
                _ => Value::Str(s.to_string()),
            },
        }
    }

    /// Numeric view used by comparisons and statistics.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Num(n) => Some(*n),
            Value::Str(s) => s.trim().parse().ok(),
            Value::Bool(_) | Value::Null => None,
        }
    }

    /// JSON representation, used by `--output json`.
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Value::Str(s) => serde_json::Value::String(s.clone()),
            Value::Num(n) if n.fract() == 0.0 && n.abs() < 1e15 => {
                serde_json::Value::Number((*n as i64).into())
            }
            Value::Num(n) => serde_json::Number::from_f64(*n)
                .map(serde_json::Value::Number)
                .unwrap_or(serde_json::Value::Null),
            Value::Bool(b) => serde_json::Value::Bool(*b),
            Value::Null => serde_json::Value::Null,
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Str(s) => f.write_str(s),
            Value::Num(n) => {
                if n.fract() == 0.0 && n.abs() < 1e15 {
                    write!(f, "{}", *n as i64)
                } else {
                    write!(f, "{n}")
                }
            }
            Value::Bool(b) => write!(f, "{b}"),
            Value::Null => f.write_str("null"),
        }
    }
}

/// A parsed log line. Field order follows the source line.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Record {
    pub fields: Vec<(String, Value)>,
}

impl Record {
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.fields.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn push(&mut self, key: impl Into<String>, value: Value) {
        self.fields.push((key.into(), value));
    }

    /// First present field among `keys` (used for level/message/time aliases).
    pub fn get_any(&self, keys: &[&str]) -> Option<(&str, &Value)> {
        keys.iter()
            .find_map(|k| self.fields.iter().find(|(name, _)| name == k))
            .map(|(k, v)| (k.as_str(), v))
    }

    pub fn to_json(&self) -> serde_json::Value {
        let map = self
            .fields
            .iter()
            .map(|(k, v)| (k.clone(), v.to_json()))
            .collect::<serde_json::Map<_, _>>();
        serde_json::Value::Object(map)
    }
}
