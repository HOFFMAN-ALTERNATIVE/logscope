//! logscope: tail, filter and aggregate structured logs.

pub mod input;
pub mod output;
pub mod parse;
pub mod query;
pub mod record;
pub mod stats;

use anyhow::{bail, Result};
use clap::Parser;
use output::{OutputMode, Printer};
use parse::{parse_line, Format};
use query::Query;
use stats::{Samples, TopN};
use std::collections::HashMap;
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;

/// Fast CLI to tail, filter and aggregate structured logs (JSON, logfmt, nginx).
#[derive(Debug, Parser)]
#[command(name = "logscope", version, about, long_about = None, after_help = EXAMPLES)]
pub struct Cli {
    /// Log files to read ("-" or none = stdin).
    pub files: Vec<PathBuf>,

    /// Keep reading appended data (like `tail -F`); handles truncation.
    #[arg(short, long)]
    pub follow: bool,

    /// Input format; `auto` detects per line.
    #[arg(long, value_enum, default_value_t = Format::Auto)]
    pub format: Format,

    /// Filter expression, e.g. 'status >= 500 and path ~ "^/api"'. Repeat to AND.
    #[arg(short = 'w', long = "where", value_name = "EXPR")]
    pub filters: Vec<String>,

    /// Only print these fields (comma-separated).
    #[arg(long, value_delimiter = ',', value_name = "FIELDS")]
    pub fields: Option<Vec<String>>,

    /// Aggregate: most frequent values of FIELD.
    #[arg(long, value_name = "FIELD", conflicts_with_all = ["stats", "count", "follow"])]
    pub top: Option<String>,

    /// Aggregate: count/min/p50/p90/p95/p99/max/mean of numeric FIELD.
    #[arg(long, value_name = "FIELD", conflicts_with_all = ["count", "follow"])]
    pub stats: Option<String>,

    /// Group --stats by FIELD.
    #[arg(long, value_name = "FIELD", requires = "stats")]
    pub by: Option<String>,

    /// Number of rows for --top / --by, or max records to print.
    #[arg(short = 'n', long, value_name = "N")]
    pub limit: Option<usize>,

    /// Print only the number of matching records.
    #[arg(short, long, conflicts_with = "follow")]
    pub count: bool,

    /// Output format for records and aggregations.
    #[arg(short, long, value_enum, default_value_t = OutputMode::Pretty)]
    pub output: OutputMode,

    /// Colorize output.
    #[arg(long, value_enum, default_value_t = ColorChoice::Auto)]
    pub color: ColorChoice,

    /// Fail on lines that do not parse in the given --format (default: skip them).
    #[arg(long)]
    pub strict: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum ColorChoice {
    Auto,
    Always,
    Never,
}

const EXAMPLES: &str = "\
Examples:
  logscope app.log -w 'level == error'
  logscope -f app.log -w 'status >= 500 or latency_ms > 1000'
  logscope access.log --top path -n 5
  logscope access.log --stats request_time --by method
  kubectl logs deploy/api | logscope -w 'msg ~ \"timeout|refused\"' -o json";

/// Run the CLI; returns the process exit code.
pub fn run(cli: Cli) -> Result<i32> {
    let queries = cli
        .filters
        .iter()
        .map(|f| Query::parse(f))
        .collect::<Result<Vec<_>>>()?;
    let color = match cli.color {
        ColorChoice::Always => true,
        ColorChoice::Never => false,
        ColorChoice::Auto => io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none(),
    };
    let printer = Printer {
        mode: cli.output,
        color,
        fields: cli.fields.clone(),
    };
    let rx = input::spawn_sources(&cli.files, cli.follow)?;

    let stdout = io::stdout();
    let mut out = io::BufWriter::new(stdout.lock());
    let mut matched: usize = 0;
    let mut skipped: usize = 0;
    let mut top = TopN::default();
    let mut stats: HashMap<String, Samples> = HashMap::new();
    let aggregate = cli.top.is_some() || cli.stats.is_some() || cli.count;

    for line in rx {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let rec = match parse_line(&line, cli.format) {
            Ok(r) => r,
            Err(e) if cli.strict => bail!("parse error: {e}: {}", line.trim_end()),
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        if !queries.iter().all(|q| q.matches(&rec)) {
            continue;
        }
        matched += 1;
        if let Some(field) = &cli.top {
            if let Some(v) = rec.get(field) {
                top.add(&v.to_string());
            }
        } else if let Some(field) = &cli.stats {
            if let Some(x) = rec.get(field).and_then(record::Value::as_f64) {
                let group = match &cli.by {
                    Some(by) => rec
                        .get(by)
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "(none)".into()),
                    None => String::new(),
                };
                stats.entry(group).or_default().add(x);
            }
        } else if !aggregate {
            if let Err(e) = printer.print(&mut out, &rec) {
                return broken_pipe_ok(e);
            }
            if cli.follow {
                // Interactive: flush every record.
                if let Err(e) = out.flush() {
                    return broken_pipe_ok(e);
                }
            }
            if cli.limit.is_some_and(|n| matched >= n) {
                break;
            }
        }
    }

    let res = (|| -> io::Result<()> {
        if let Some(field) = &cli.top {
            printer.print_top(&mut out, field, &top, cli.limit.unwrap_or(10))?;
        } else if let Some(field) = &cli.stats {
            let mut rows: Vec<_> = stats
                .into_iter()
                .filter_map(|(g, mut s)| s.summary().map(|sum| (g, sum)))
                .collect();
            // Largest groups first, then by name for determinism.
            rows.sort_by(|a, b| b.1.count.cmp(&a.1.count).then_with(|| a.0.cmp(&b.0)));
            if let Some(n) = cli.limit {
                rows.truncate(n);
            }
            if rows.is_empty() {
                eprintln!("logscope: no numeric values for field {field:?}");
            } else {
                printer.print_stats(&mut out, field, &rows)?;
            }
        } else if cli.count {
            writeln!(out, "{matched}")?;
        }
        out.flush()
    })();
    if let Err(e) = res {
        return broken_pipe_ok(e);
    }
    if skipped > 0 {
        eprintln!(
            "logscope: skipped {skipped} line(s) not in {:?} format",
            cli.format
        );
    }
    // grep-like exit status: 1 when nothing matched.
    Ok(if matched == 0 { 1 } else { 0 })
}

/// `logscope ... | head` closes the pipe early; that is success, not an error.
fn broken_pipe_ok(e: io::Error) -> Result<i32> {
    if e.kind() == io::ErrorKind::BrokenPipe {
        Ok(0)
    } else {
        Err(e.into())
    }
}
