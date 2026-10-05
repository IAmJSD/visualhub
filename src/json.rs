//! Reading GitHub's JSON without a struct per endpoint.
//!
//! The client talks to a few hundred endpoints and only ever reads a handful
//! of fields from each, so responses stay as `serde_json::Value` and are read
//! through dotted paths: `issue.s("user.login")`, `run.i("run_number")`.
//! Missing fields read as empty, zero or false rather than failing, which is
//! what a UI wants -- a blank cell, not a crash.

use serde_json::Value;

static NULL: Value = Value::Null;

pub trait Json {
    /// The value at a dotted path (`"head.repo.full_name"`), or null.
    fn at(&self, path: &str) -> &Value;
    /// A string field; numbers and booleans are written out, null is empty.
    fn s(&self, path: &str) -> String;
    fn i(&self, path: &str) -> i64;
    fn b(&self, path: &str) -> bool;
    /// An array field's items, or none.
    fn list(&self, path: &str) -> &[Value];
    /// Whether the path holds something other than null.
    fn has(&self, path: &str) -> bool;
}

impl Json for Value {
    fn at(&self, path: &str) -> &Value {
        if path.is_empty() {
            return self;
        }
        let mut here = self;
        for part in path.split('.') {
            here = match here {
                Value::Object(map) => map.get(part).unwrap_or(&NULL),
                Value::Array(items) => part
                    .parse::<usize>()
                    .ok()
                    .and_then(|i| items.get(i))
                    .unwrap_or(&NULL),
                _ => &NULL,
            };
        }
        here
    }

    fn s(&self, path: &str) -> String {
        match self.at(path) {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            _ => String::new(),
        }
    }

    fn i(&self, path: &str) -> i64 {
        match self.at(path) {
            Value::Number(n) => n
                .as_i64()
                .unwrap_or_else(|| n.as_f64().unwrap_or(0.0) as i64),
            Value::String(s) => s.parse().unwrap_or(0),
            _ => 0,
        }
    }

    fn b(&self, path: &str) -> bool {
        match self.at(path) {
            Value::Bool(b) => *b,
            _ => false,
        }
    }

    fn list(&self, path: &str) -> &[Value] {
        match self.at(path) {
            Value::Array(items) => items,
            _ => &[],
        }
    }

    fn has(&self, path: &str) -> bool {
        !self.at(path).is_null()
    }
}

/// The items of a list response: the array itself, or the array under one of
/// the wrapper keys GitHub uses (`items` for search, `workflow_runs`, ...).
pub fn items<'a>(value: &'a Value, key: Option<&str>) -> &'a [Value] {
    match key {
        Some(key) => value.list(key),
        None => match value {
            Value::Array(items) => items,
            Value::Object(map) => map
                .values()
                .find_map(|v| v.as_array().map(|a| a.as_slice()))
                .unwrap_or(&[]),
            _ => &[],
        },
    }
}

/// Up to `n` characters of `s`, with an ellipsis if it was cut.
pub fn clip(s: &str, n: usize) -> String {
    let mut out: String = s.chars().take(n).collect();
    if s.chars().nth(n).is_some() {
        out.push('…');
    }
    out
}

/// The first line of a commit message or body.
pub fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").to_string()
}

/// "1.2k" style counts for stars and forks.
pub fn count(n: i64) -> String {
    match n {
        n if n >= 1_000_000 => format!("{:.1}m", n as f64 / 1_000_000.0),
        n if n >= 10_000 => format!("{}k", n / 1000),
        n if n >= 1_000 => format!("{:.1}k", n as f64 / 1000.0),
        n => n.to_string(),
    }
}

/// Bytes as "12.3 KB".
pub fn bytes(n: i64) -> String {
    let n = n as f64;
    if n >= 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1} GB", n / (1024.0 * 1024.0 * 1024.0))
    } else if n >= 1024.0 * 1024.0 {
        format!("{:.1} MB", n / (1024.0 * 1024.0))
    } else if n >= 1024.0 {
        format!("{:.1} KB", n / 1024.0)
    } else {
        format!("{n} B")
    }
}

/// Percent-encode a path segment or query value.
pub fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Percent-encode a file path, keeping its slashes.
pub fn enc_path(s: &str) -> String {
    s.split('/').map(enc).collect::<Vec<_>>().join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn dotted_paths_reach_into_objects_and_arrays() {
        let v = json!({"user": {"login": "octo"}, "labels": [{"name": "bug"}], "n": 3});
        assert_eq!(v.s("user.login"), "octo");
        assert_eq!(v.s("labels.0.name"), "bug");
        assert_eq!(v.i("n"), 3);
        assert_eq!(v.s("missing.deep"), "");
        assert!(!v.b("n"));
    }

    #[test]
    fn counts_and_encoding() {
        assert_eq!(count(999), "999");
        assert_eq!(count(1500), "1.5k");
        assert_eq!(count(25_000), "25k");
        assert_eq!(enc("a b/c"), "a%20b%2Fc");
        assert_eq!(enc_path("src/a b.rs"), "src/a%20b.rs");
    }
}
