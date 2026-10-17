use assert_cmd::Command;
use predicates::prelude::*;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command as StdCommand, Stdio};
use std::sync::mpsc;
use std::time::Duration;

fn logscope() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_logscope"));
    c.arg("--color=never");
    c
}

const MIXED: &str = r#"{"ts":"2026-10-16T10:00:00Z","level":"info","msg":"started","port":8080}
level=error msg="db timeout" latency_ms=1200 service=api
192.0.2.1 - - [16/Oct/2026:10:00:01 +0000] "GET /api/users HTTP/1.1" 500 12 "-" "curl/8" 0.250
plain text line
{"level":"error","msg":"disk full","http":{"status":507}}
"#;

#[test]
fn autodetects_mixed_formats_and_filters() {
    logscope()
        .args(["-w", "level == error"])
        .write_stdin(MIXED)
        .assert()
        .success()
        .stdout(predicate::str::contains("db timeout"))
        .stdout(predicate::str::contains("disk full"))
        .stdout(predicate::str::contains("started").not());
}

#[test]
fn nested_json_and_nginx_fields_are_queryable() {
    logscope()
        .args(["-w", "http.status >= 500 or status >= 500", "-o", "json"])
        .write_stdin(MIXED)
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""http.status":507"#))
        .stdout(predicate::str::contains(r#""path":"/api/users""#));
}

#[test]
fn repeated_where_is_and_and_count() {
    logscope()
        .args(["-w", "level=error", "-w", "latency_ms > 1000", "--count"])
        .write_stdin(MIXED)
        .assert()
        .success()
        .stdout("1\n");
}

#[test]
fn no_match_exits_1_like_grep() {
    logscope()
        .args(["-w", "level == fatal"])
        .write_stdin(MIXED)
        .assert()
        .code(1)
        .stdout("");
}

#[test]
fn bad_query_exits_2() {
    logscope()
        .args(["-w", "status >="])
        .write_stdin(MIXED)
        .assert()
        .code(2)
        .stderr(predicate::str::contains("expected a value"));
}

#[test]
fn missing_file_is_a_clear_error() {
    logscope()
        .arg("/definitely/not/here.log")
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "cannot open /definitely/not/here.log",
        ));
}

#[test]
fn top_n_on_example_access_log() {
    logscope()
        .args([
            "examples/access.log",
            "--top",
            "path",
            "-n",
            "3",
            "-o",
            "json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""field":"path""#))
        .stdout(predicate::str::contains(r#""total":500"#));
}

#[test]
fn top_text_output_is_sorted() {
    let input = "a=x\na=y\na=x\na=z\na=x\na=y\n";
    let out = logscope()
        .args(["--top", "a"])
        .write_stdin(input)
        .assert()
        .success();
    let s = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    let rows: Vec<&str> = s
        .lines()
        .skip(1)
        .map(|l| l.split_whitespace().next().unwrap())
        .collect();
    assert_eq!(rows, ["x", "y", "z"]);
}

#[test]
fn stats_percentiles_exact() {
    let mut input = String::new();
    for i in 1..=100 {
        let grp = if i <= 50 { "a" } else { "b" };
        input.push_str(&format!(
            "lat={i} grp={grp}
"
        ));
    }
    logscope()
        .args(["--stats", "lat", "-o", "json"])
        .write_stdin(input.clone())
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""count":100"#))
        .stdout(predicate::str::contains(r#""p50":50.5"#))
        .stdout(predicate::str::contains(r#""max":100.0"#));
    logscope()
        .args(["--stats", "lat", "--by", "grp", "-o", "json"])
        .write_stdin(input)
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""group":"a""#))
        .stdout(predicate::str::contains(r#""group":"b""#));
}

#[test]
fn stats_by_method_on_nginx_example() {
    logscope()
        .args([
            "examples/access.log",
            "--stats",
            "request_time",
            "--by",
            "method",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("p99"))
        .stdout(predicate::str::contains("GET"));
}

#[test]
fn fields_projection_and_limit() {
    logscope()
        .args([
            "examples/worker.logfmt",
            "--fields",
            "job_id,queue",
            "-o",
            "logfmt",
            "-n",
            "2",
        ])
        .assert()
        .success()
        .stdout(
            predicate::str::is_match("^job_id=1000 queue=\\w+\njob_id=1001 queue=\\w+\n$").unwrap(),
        );
}

#[test]
fn strict_mode_rejects_unparseable_lines() {
    logscope()
        .args(["--format", "json", "--strict"])
        .write_stdin("{\"a\":1}\nnot json\n")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("parse error"));
    logscope()
        .args(["--format", "json"])
        .write_stdin("{\"a\":1}\nnot json\n")
        .assert()
        .success()
        .stderr(predicate::str::contains("skipped 1 line"));
}

#[test]
fn color_always_emits_ansi() {
    Command::new(env!("CARGO_BIN_EXE_logscope"))
        .args(["--color=always"])
        .write_stdin("level=error msg=boom\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("\x1b[31m"));
}

#[test]
fn aggregation_conflicts_with_follow() {
    logscope()
        .args(["-f", "--top", "a", "x.log"])
        .assert()
        .code(2);
}

#[test]
fn follow_mode_picks_up_appended_lines() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("live.log");
    std::fs::write(&path, "level=info msg=first\n").unwrap();

    let mut child = StdCommand::new(env!("CARGO_BIN_EXE_logscope"))
        .args(["--color=never", "-f", "-w", "level=error"])
        .arg(&path)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if tx.send(line.unwrap()).is_err() {
                break;
            }
        }
    });

    std::thread::sleep(Duration::from_millis(300));
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    writeln!(f, "level=info msg=ignored").unwrap();
    writeln!(f, "level=error msg=appended").unwrap();
    f.flush().unwrap();

    let got = rx.recv_timeout(Duration::from_secs(5));
    child.kill().unwrap();
    let _ = child.wait();
    let line = got.expect("follow mode should emit the appended error line");
    assert!(line.contains("appended"), "got {line:?}");
}

#[test]
fn overlong_line_is_truncated_not_buffered() {
    // 3 MiB without a newline, then a normal record.
    let mut input = "x".repeat(3 << 20);
    input.push('\n');
    input.push_str("level=error msg=after\n");
    logscope()
        .args(["-w", "level == error", "--count"])
        .write_stdin(input)
        .assert()
        .success()
        .stdout("1\n");
}
