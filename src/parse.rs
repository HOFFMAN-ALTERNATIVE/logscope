//! Line parsers for JSON, logfmt and nginx/Apache combined logs.

use crate::record::{Record, Value};
use anyhow::{bail, Result};
use regex::Regex;
use std::sync::OnceLock;

/// Input format. `Auto` detects the format per line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Format {
    Auto,
    Json,
    Logfmt,
    Nginx,
}

/// Parse one line. In `Auto` mode unrecognised lines become a record with a
/// single `message` field, so plain-text lines are still searchable.
pub fn parse_line(line: &str, format: Format) -> Result<Record> {
    let line = line.trim_end_matches(['\r', '\n']);
    match format {
        Format::Json => parse_json(line),
        Format::Logfmt => Ok(parse_logfmt(line)),
        Format::Nginx => match parse_nginx(line) {
            Some(r) => Ok(r),
            None => bail!("not an nginx combined log line"),
        },
        Format::Auto => {
            let trimmed = line.trim_start();
            if trimmed.starts_with('{') {
                if let Ok(r) = parse_json(trimmed) {
                    return Ok(r);
                }
            }
            if let Some(r) = parse_nginx(line) {
                return Ok(r);
            }
            if looks_like_logfmt(line) {
                return Ok(parse_logfmt(line));
            }
            let mut r = Record::default();
            r.push("message", Value::Str(line.to_string()));
            Ok(r)
        }
    }
}

// ------------------------------------------------------------------- JSON

fn parse_json(line: &str) -> Result<Record> {
    let v: serde_json::Value = serde_json::from_str(line)?;
    let serde_json::Value::Object(map) = v else {
        bail!("JSON log line must be an object");
    };
    let mut r = Record::default();
    for (k, v) in map {
        flatten(&mut r, k, v);
    }
    Ok(r)
}

/// Nested objects become dotted keys: {"http":{"status":200}} -> http.status.
fn flatten(r: &mut Record, key: String, v: serde_json::Value) {
    use serde_json::Value as J;
    match v {
        J::Object(map) => {
            for (k, v) in map {
                flatten(r, format!("{key}.{k}"), v);
            }
        }
        J::String(s) => r.push(key, Value::Str(s)),
        J::Number(n) => r.push(key, n.as_f64().map_or(Value::Null, Value::Num)),
        J::Bool(b) => r.push(key, Value::Bool(b)),
        J::Null => r.push(key, Value::Null),
        J::Array(_) => r.push(key, Value::Str(v.to_string())),
    }
}

// ----------------------------------------------------------------- logfmt

fn looks_like_logfmt(line: &str) -> bool {
    // At least one `key=` token where key is a bare identifier.
    line.split_whitespace().any(|tok| {
        tok.split_once('=').is_some_and(|(k, _)| {
            !k.is_empty()
                && k.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '@'))
        })
    })
}

/// Parse `key=value key="quoted \"value\"" flag` pairs. A bare key is `true`.
pub fn parse_logfmt(line: &str) -> Record {
    let mut r = Record::default();
    let mut chars = line.char_indices().peekable();
    while let Some(&(start, c)) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        // key
        let mut end = start;
        while let Some(&(i, c)) = chars.peek() {
            if c == '=' || c.is_whitespace() {
                break;
            }
            end = i + c.len_utf8();
            chars.next();
        }
        let key = &line[start..end];
        if chars.peek().map(|&(_, c)| c) != Some('=') {
            if !key.is_empty() {
                r.push(key, Value::Bool(true));
            }
            continue;
        }
        chars.next(); // '='
        let value = if chars.peek().map(|&(_, c)| c) == Some('"') {
            chars.next();
            let mut s = String::new();
            while let Some((_, c)) = chars.next() {
                match c {
                    '\\' => {
                        if let Some((_, esc)) = chars.next() {
                            s.push(match esc {
                                'n' => '\n',
                                't' => '\t',
                                other => other,
                            });
                        }
                    }
                    '"' => break,
                    _ => s.push(c),
                }
            }
            Value::Str(s)
        } else {
            let vstart = chars.peek().map_or(line.len(), |&(i, _)| i);
            let mut vend = vstart;
            while let Some(&(i, c)) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                vend = i + c.len_utf8();
                chars.next();
            }
            Value::infer(&line[vstart..vend])
        };
        if !key.is_empty() {
            r.push(key, value);
        }
    }
    r
}

// ------------------------------------------------------------------ nginx

fn nginx_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // combined format, optionally followed by $request_time
        Regex::new(
            r#"^(?P<remote_addr>\S+) \S+ (?P<remote_user>\S+) \[(?P<time>[^\]]+)\] "(?P<request>[^"]*)" (?P<status>\d{3}) (?P<bytes>\d+|-)(?: "(?P<referer>[^"]*)" "(?P<user_agent>[^"]*)")?(?: (?P<request_time>\d+(?:\.\d+)?))?\s*$"#,
        )
        .expect("valid nginx regex")
    })
}

/// Parse an nginx/Apache "combined" (or "common") access-log line.
pub fn parse_nginx(line: &str) -> Option<Record> {
    let caps = nginx_re().captures(line)?;
    let mut r = Record::default();
    r.push("time", Value::Str(caps["time"].to_string()));
    r.push("remote_addr", Value::Str(caps["remote_addr"].to_string()));
    r.push("remote_user", Value::infer(&caps["remote_user"]));
    let request = &caps["request"];
    let mut parts = request.splitn(3, ' ');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(m), Some(p), proto) => {
            r.push("method", Value::Str(m.to_string()));
            r.push("path", Value::Str(p.to_string()));
            if let Some(proto) = proto {
                r.push("protocol", Value::Str(proto.to_string()));
            }
        }
        _ => r.push("request", Value::Str(request.to_string())),
    }
    r.push("status", Value::infer(&caps["status"]));
    r.push("bytes", Value::infer(&caps["bytes"]));
    for name in ["referer", "user_agent"] {
        if let Some(m) = caps.name(name) {
            r.push(name, Value::infer(m.as_str()));
        }
    }
    if let Some(m) = caps.name("request_time") {
        r.push("request_time", Value::infer(m.as_str()));
    }
    Some(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_flattens_and_types() {
        let r = parse_line(
            r#"{"level":"info","http":{"status":200,"ok":true},"tags":[1,2],"x":null}"#,
            Format::Auto,
        )
        .unwrap();
        assert_eq!(r.get("level"), Some(&Value::Str("info".into())));
        assert_eq!(r.get("http.status"), Some(&Value::Num(200.0)));
        assert_eq!(r.get("http.ok"), Some(&Value::Bool(true)));
        assert_eq!(r.get("tags"), Some(&Value::Str("[1,2]".into())));
        assert_eq!(r.get("x"), Some(&Value::Null));
    }

    #[test]
    fn json_rejects_non_object() {
        assert!(parse_line("[1,2]", Format::Json).is_err());
        assert!(parse_line("not json", Format::Json).is_err());
    }

    #[test]
    fn logfmt_quotes_escapes_and_flags() {
        let r = parse_logfmt(r#"level=warn msg="disk \"almost\" full" pct=91.5 retry dur=12ms"#);
        assert_eq!(r.get("level"), Some(&Value::Str("warn".into())));
        assert_eq!(
            r.get("msg"),
            Some(&Value::Str("disk \"almost\" full".into()))
        );
        assert_eq!(r.get("pct"), Some(&Value::Num(91.5)));
        assert_eq!(r.get("retry"), Some(&Value::Bool(true)));
        assert_eq!(r.get("dur"), Some(&Value::Str("12ms".into())));
    }

    #[test]
    fn logfmt_unicode_and_empty() {
        let r = parse_logfmt("user=jörg note=\"ünïcode ✓\" empty=");
        assert_eq!(r.get("user"), Some(&Value::Str("jörg".into())));
        assert_eq!(r.get("note"), Some(&Value::Str("ünïcode ✓".into())));
        assert_eq!(r.get("empty"), Some(&Value::Null));
    }

    #[test]
    fn nginx_combined_with_request_time() {
        let line = r#"192.0.2.1 - - [10/Oct/2026:13:55:36 +0000] "GET /api/users?id=7 HTTP/1.1" 200 512 "-" "curl/8.5.0" 0.123"#;
        let r = parse_line(line, Format::Auto).unwrap();
        assert_eq!(r.get("method"), Some(&Value::Str("GET".into())));
        assert_eq!(r.get("path"), Some(&Value::Str("/api/users?id=7".into())));
        assert_eq!(r.get("status"), Some(&Value::Num(200.0)));
        assert_eq!(r.get("referer"), Some(&Value::Null));
        assert_eq!(r.get("user_agent"), Some(&Value::Str("curl/8.5.0".into())));
        assert_eq!(r.get("request_time"), Some(&Value::Num(0.123)));
    }

    #[test]
    fn nginx_common_format() {
        let r = parse_nginx(
            r#"198.51.100.4 - alice [10/Oct/2026:13:55:36 +0000] "POST /login HTTP/2.0" 401 -"#,
        )
        .unwrap();
        assert_eq!(r.get("remote_user"), Some(&Value::Str("alice".into())));
        assert_eq!(r.get("bytes"), Some(&Value::Null));
        assert!(r.get("user_agent").is_none());
    }

    #[test]
    fn plain_text_falls_back_to_message() {
        let r = parse_line("just some text", Format::Auto).unwrap();
        assert_eq!(r.get("message"), Some(&Value::Str("just some text".into())));
        assert!(parse_line("just some text", Format::Nginx).is_err());
    }
}
