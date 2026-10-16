# logscope

[![ci](https://github.com/HOFFMAN-ALTERNATIVE/logscope/actions/workflows/ci.yml/badge.svg)](https://github.com/HOFFMAN-ALTERNATIVE/logscope/actions/workflows/ci.yml)
![Rust](https://img.shields.io/badge/Rust-2021-000000?logo=rust&logoColor=white)
![MSRV](https://img.shields.io/badge/MSRV-1.80-blue)
[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)](LICENSE)

**`grep` + `jq` + `awk` for logs, in one fast binary.** `logscope` tails, filters
and aggregates structured logs — **JSON, logfmt and nginx/Apache access logs, even
mixed in one stream** — with a real query language, top-N breakdowns and exact
latency percentiles, in colour.

```console
$ logscope access.log --stats request_time --by method
group     count         min         p50         p90         p95         p99         max        mean
GET      772000       0.003       0.047       0.171       0.262       0.450       0.475       0.075
POST     228000       0.005       0.046       0.182       0.248       0.317       0.395       0.074

$ logscope access.log --top path -n 5
top 5 path (of 1000000 records)
/api/users        310000   31.0%  ██████████████████████████████
/static/app.js    264000   26.4%  ██████████████████████████
/api/orders       194000   19.4%  ███████████████████
/healthz           92000    9.2%  █████████
/api/login         72000    7.2%  ███████
```

## Why

During an incident you want answers like *"which endpoints are throwing 5xx?"*
or *"what is p99 latency per service right now?"* — not a 40-character
`jq | sort | uniq -c | sort -rn` pipeline that breaks on the first logfmt line.
Heavy log platforms answer that, but only after ingestion, and not on the box
you are SSH'd into. `logscope` is a single static binary that understands the
formats services actually emit and answers those questions directly from files
or pipes.

## Features

- **Formats:** JSON (nested objects flattened to `http.status`), logfmt (quoted
  values, escapes, bare flags), nginx/Apache *combined* and *common* format with
  optional trailing `$request_time`. `--format auto` detects **per line**, so mixed
  streams (`kubectl logs`, docker compose) just work; plain text becomes `message`.
- **Query language:** `== != > >= < <=`, regex `~` / `!~`, `contains`, field
  existence, `and`/`or`/`not` (also `&& || !`), parentheses, quoted strings.
  Numeric comparison when both sides are numbers, otherwise lexicographic — so
  ISO-8601 timestamps compare correctly: `-w 'ts >= 2026-10-16T10:00:00Z'`.
- **Aggregations:** `--top FIELD` (count, share, bar chart) and
  `--stats FIELD [--by GROUP]` (count/min/p50/p90/p95/p99/max/mean, exact).
- **Follow mode:** `-f` tails one or many files like `tail -F`, survives
  truncation/rotation, flushes per record.
- **Output:** coloured pretty view (time · level · status · message · fields),
  NDJSON (`-o json`) for piping, logfmt, field projection (`--fields a,b`).
- **Unix citizen:** reads stdin, `| head` safe (no broken-pipe panics), grep-like
  exit codes (`0` match, `1` no match, `2` error), honours `NO_COLOR`.

## Architecture

```mermaid
flowchart LR
    subgraph Sources
        F1[file 1] --> T1[reader thread]
        F2[file N<br/>follow -f] --> T2[tail thread<br/>poll + truncate detect]
        SI[stdin] --> T3[reader thread]
    end
    T1 & T2 & T3 -- "bounded channel<br/>(backpressure)" --> P
    P[parse_line<br/>auto-detect per line] --> R[Record<br/>ordered flat fields]
    R --> Q{Query AST<br/>compiled once}
    Q -- no --> X[drop]
    Q -- yes --> M{mode}
    M -- print --> O[Printer<br/>pretty / json / logfmt]
    M -- --top --> TN[TopN<br/>hash counts]
    M -- --stats --> ST[Samples per group<br/>exact percentiles]
    TN & ST --> O
```

| Module | Responsibility |
|---|---|
| `input.rs` | One thread per source, bounded `sync_channel`, follow/rotation logic, lossy UTF-8 |
| `parse.rs` | JSON / logfmt / nginx parsers and per-line auto-detection |
| `query.rs` | Tokenizer + recursive-descent parser → AST; regexes compiled once |
| `stats.rs` | Top-N counter, exact percentiles (linear interpolation, NumPy-compatible) |
| `output.rs` | Rendering, colours, projection |

## Install

```bash
cargo install --git https://github.com/HOFFMAN-ALTERNATIVE/logscope
# or grab a prebuilt binary (Linux / macOS / Windows) from Releases
```

## Usage

```bash
# Errors only, from a JSON app log
logscope app.jsonl -w 'level == error'

# Live: 5xx or slow requests across several files
logscope -f /var/log/nginx/access.log -w 'status >= 500 or request_time > 1'

# Regex + boolean logic, emit NDJSON for further processing
logscope app.jsonl -w '(service == api or service == billing) and msg ~ "timeout|refused"' -o json

# Which clients hit /api/login the most?
logscope access.log -w 'path == /api/login' --top remote_addr -n 10

# Latency percentiles per queue from logfmt worker logs
logscope worker.logfmt --stats duration --by queue

# Just count, or project a few fields
logscope access.log -w 'status == 404' --count
logscope worker.logfmt --fields job_id,queue,duration -o logfmt

# Mixed formats in one pipe
docker compose logs --no-log-prefix | logscope -w 'level == error or status >= 500'
```

Try it on the bundled samples in [`examples/`](examples).

## Design decisions & trade-offs

- **Per-line format detection** costs a cheap first-byte check plus one anchored
  regex for non-JSON lines, and buys correctness on mixed streams, which are the
  norm in containers. `--format` pins it when you want strictness (`--strict`
  fails on unparsable lines instead of skipping them).
- **Exact percentiles over sketches.** Samples are stored and sorted once
  (8 bytes/value: 1M values ≈ 8 MB). For CLI-sized inputs exactness beats the
  error bounds of t-digest/HDR; a streaming sketch would be the next step for
  unbounded follow-mode aggregation — which is why aggregations and `-f` are
  mutually exclusive for now.
- **Flat, ordered records** (`Vec<(String, Value)>`) instead of a `HashMap`:
  log lines have ~5–20 fields, so linear lookup is faster than hashing and
  output keeps source order.
- **Thread per source + bounded channel** gives simple follow-mode semantics
  for many files and natural backpressure; parsing stays single-threaded so
  output order per file is preserved.
- **Polling, not inotify**, for `-f`: portable (Linux/macOS/Windows, network
  filesystems), 200 ms latency, no extra dependencies.
- **Small dependency surface:** `clap`, `regex`, `serde_json`, `anyhow`. Colours
  are hand-rolled ANSI.

## Performance

Release build, single thread, 8-vCPU Xeon VM, 1,000,000 lines:

| Workload | Input | Time | Throughput |
|---|---|---|---|
| nginx, filter `status >= 500` + count | 104 MB | 2.8 s | ~360k lines/s |
| nginx, `--stats request_time` | 104 MB | 2.8 s | ~360k lines/s |
| JSON, filter `level == error` + count | 1M lines | 2.0 s | ~490k lines/s |
| JSON, `--stats latency_ms --by service` | 1M lines | 2.3 s | ~440k lines/s |

CI re-runs the nginx benchmark on every push (`smoke` job). Parallel parsing
with order-preserving reassembly is the obvious next optimisation.

## Development

```bash
cargo test                                  # 18 unit + 15 CLI integration tests (assert_cmd)
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

CI: rustfmt + clippy `-D warnings` · tests on Linux and macOS · MSRV 1.80 check ·
release-build throughput smoke test. Tagging `v*` builds Linux/macOS/Windows
binaries and attaches them to a GitHub Release.

## License

MIT © Herman Hoffman
