//! Rendering of records and aggregation results.

use crate::record::{Record, Value};
use crate::stats::{Summary, TopN};
use std::io::{self, Write};

const RESET: &str = "\x1b[0m";
const DIM: &str = "\x1b[2m";
const BOLD: &str = "\x1b[1m";
const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const BLUE: &str = "\x1b[34m";
const MAGENTA: &str = "\x1b[35m";
const CYAN: &str = "\x1b[36m";

pub const TIME_KEYS: &[&str] = &["time", "ts", "timestamp", "@timestamp", "t"];
pub const LEVEL_KEYS: &[&str] = &["level", "lvl", "severity", "loglevel"];
pub const MSG_KEYS: &[&str] = &["msg", "message", "event"];

/// Output mode for matching records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum OutputMode {
    /// Human-friendly: time, level, message, then key=value pairs.
    Pretty,
    /// Normalised NDJSON (one object per line), for piping into other tools.
    Json,
    /// logfmt key=value pairs.
    Logfmt,
}

pub struct Printer {
    pub mode: OutputMode,
    pub color: bool,
    /// Restrict output to these fields (in this order).
    pub fields: Option<Vec<String>>,
}

impl Printer {
    pub fn print(&self, out: &mut impl Write, r: &Record) -> io::Result<()> {
        let projected;
        let r = match &self.fields {
            Some(fields) => {
                projected = Record {
                    fields: fields
                        .iter()
                        .filter_map(|f| r.get(f).map(|v| (f.clone(), v.clone())))
                        .collect(),
                };
                &projected
            }
            None => r,
        };
        match self.mode {
            OutputMode::Json => writeln!(out, "{}", r.to_json()),
            OutputMode::Logfmt => {
                let line = r
                    .fields
                    .iter()
                    .map(|(k, v)| format!("{k}={}", logfmt_value(v)))
                    .collect::<Vec<_>>()
                    .join(" ");
                writeln!(out, "{line}")
            }
            OutputMode::Pretty => self.pretty(out, r),
        }
    }

    fn paint(&self, color: &str, s: &str) -> String {
        if self.color {
            format!("{color}{s}{RESET}")
        } else {
            s.to_string()
        }
    }

    fn pretty(&self, out: &mut impl Write, r: &Record) -> io::Result<()> {
        let mut parts = Vec::new();
        let time = r.get_any(TIME_KEYS);
        let level = r.get_any(LEVEL_KEYS);
        let msg = r.get_any(MSG_KEYS);
        if let Some((_, t)) = time {
            parts.push(self.paint(DIM, &t.to_string()));
        }
        if let Some((_, l)) = level {
            let l = l.to_string();
            let c = level_color(&l);
            parts.push(self.paint(c, &format!("{:<5}", l.to_uppercase())));
        }
        if let Some(status) = r.get("status").and_then(Value::as_f64) {
            let c = match status as u16 {
                500..=599 => RED,
                400..=499 => YELLOW,
                300..=399 => CYAN,
                _ => GREEN,
            };
            parts.push(self.paint(c, &format!("{status}")));
        }
        if let Some((_, m)) = msg {
            parts.push(self.paint(BOLD, &m.to_string()));
        }
        let skip = [
            time.map(|t| t.0),
            level.map(|l| l.0),
            msg.map(|m| m.0),
            Some("status"),
        ];
        for (k, v) in &r.fields {
            if skip.contains(&Some(k.as_str())) {
                continue;
            }
            parts.push(format!("{}={}", self.paint(CYAN, k), logfmt_value(v)));
        }
        writeln!(out, "{}", parts.join(" "))
    }

    pub fn print_top(
        &self,
        out: &mut impl Write,
        field: &str,
        top: &TopN,
        n: usize,
    ) -> io::Result<()> {
        let total = top.total().max(1) as f64;
        let rows = top.top(n);
        if self.mode == OutputMode::Json {
            let arr: Vec<_> = rows
                .iter()
                .map(|(v, c)| serde_json::json!({ "value": v, "count": c, "percent": *c as f64 * 100.0 / total }))
                .collect();
            return writeln!(
                out,
                "{}",
                serde_json::json!({ "field": field, "total": top.total(), "top": arr })
            );
        }
        writeln!(
            out,
            "{}",
            self.paint(
                BOLD,
                &format!("top {} {field} (of {} records)", rows.len(), top.total())
            )
        )?;
        let width = rows
            .iter()
            .map(|(v, _)| v.chars().count())
            .max()
            .unwrap_or(0)
            .min(60);
        let max = rows.first().map_or(1, |r| r.1).max(1) as f64;
        for (v, c) in rows {
            let pct = c as f64 * 100.0 / total;
            let bar = "█".repeat(((c as f64 / max) * 30.0).round().max(1.0) as usize);
            writeln!(
                out,
                "{v:<width$}  {c:>8}  {pct:>5.1}%  {}",
                self.paint(MAGENTA, &bar)
            )?;
        }
        Ok(())
    }

    pub fn print_stats(
        &self,
        out: &mut impl Write,
        field: &str,
        rows: &[(String, Summary)],
    ) -> io::Result<()> {
        if self.mode == OutputMode::Json {
            for (group, s) in rows {
                let mut obj = serde_json::json!({
                    "field": field, "count": s.count, "min": s.min, "max": s.max, "mean": s.mean,
                    "p50": s.p50, "p90": s.p90, "p95": s.p95, "p99": s.p99,
                });
                if !group.is_empty() {
                    obj["group"] = serde_json::Value::String(group.clone());
                }
                writeln!(out, "{obj}")?;
            }
            return Ok(());
        }
        let gw = rows
            .iter()
            .map(|(g, _)| g.chars().count())
            .max()
            .unwrap_or(0)
            .clamp(5, 40);
        writeln!(
            out,
            "{}",
            self.paint(
                BOLD,
                &format!(
                    "{:<gw$}  {:>8}  {:>10}  {:>10}  {:>10}  {:>10}  {:>10}  {:>10}  {:>10}",
                    "group", "count", "min", "p50", "p90", "p95", "p99", "max", "mean"
                )
            )
        )?;
        for (group, s) in rows {
            let g = if group.is_empty() {
                field
            } else {
                group.as_str()
            };
            writeln!(
                out,
                "{g:<gw$}  {:>8}  {:>10}  {:>10}  {:>10}  {:>10}  {:>10}  {:>10}  {:>10}",
                s.count,
                num(s.min),
                num(s.p50),
                num(s.p90),
                num(s.p95),
                self.paint(YELLOW, &format!("{:>10}", num(s.p99))),
                num(s.max),
                num(s.mean)
            )?;
        }
        Ok(())
    }
}

fn num(x: f64) -> String {
    if x.fract() == 0.0 && x.abs() < 1e12 {
        format!("{}", x as i64)
    } else {
        format!("{x:.3}")
    }
}

fn level_color(level: &str) -> &'static str {
    match level.to_ascii_lowercase().as_str() {
        "error" | "err" | "fatal" | "crit" | "critical" | "panic" => RED,
        "warn" | "warning" => YELLOW,
        "info" | "notice" => GREEN,
        "debug" | "trace" => BLUE,
        _ => DIM,
    }
}

fn logfmt_value(v: &Value) -> String {
    let s = v.to_string();
    if s.is_empty() || s.contains(|c: char| c.is_whitespace() || c == '"' || c == '=') {
        format!("{:?}", s)
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_logfmt;

    fn render(p: &Printer, line: &str) -> String {
        let mut buf = Vec::new();
        p.print(&mut buf, &parse_logfmt(line)).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn pretty_orders_known_fields_first() {
        let p = Printer {
            mode: OutputMode::Pretty,
            color: false,
            fields: None,
        };
        let s = render(
            &p,
            r#"user=7 msg="hi there" level=warn ts=2026-01-01T00:00:00Z"#,
        );
        assert_eq!(s, "2026-01-01T00:00:00Z WARN  hi there user=7\n");
    }

    #[test]
    fn color_and_projection() {
        let p = Printer {
            mode: OutputMode::Logfmt,
            color: false,
            fields: Some(vec!["b".into(), "a".into()]),
        };
        assert_eq!(render(&p, "a=1 b=\"x y\" c=3"), "b=\"x y\" a=1\n");
        let p = Printer {
            mode: OutputMode::Pretty,
            color: true,
            fields: None,
        };
        assert!(render(&p, "level=error msg=boom").contains("\x1b[31mERROR"));
    }

    #[test]
    fn json_output() {
        let p = Printer {
            mode: OutputMode::Json,
            color: false,
            fields: None,
        };
        assert_eq!(render(&p, "a=1 b=x"), "{\"a\":1,\"b\":\"x\"}\n");
    }
}
