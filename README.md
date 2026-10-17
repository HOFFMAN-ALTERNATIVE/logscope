# logscope

[![ci](https://github.com/HOFFMAN-ALTERNATIVE/logscope/actions/workflows/ci.yml/badge.svg)](https://github.com/HOFFMAN-ALTERNATIVE/logscope/actions/workflows/ci.yml)
![MSRV](https://img.shields.io/badge/MSRV-1.85-blue)
[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)](LICENSE)

One binary that filters and aggregates JSON, logfmt and nginx access logs, mixed in
the same stream. It answers "which endpoints return 5xx" or "p99 latency per service"
on the box you are SSH'd into, without a `jq | sort | uniq -c` pipeline and without
shipping logs anywhere first. On 1M nginx lines it computes exact latency percentiles
in 2.5 s on one core.

```console
$ logscope access.log --stats request_time --by method
group     count         min         p50         p90         p95         p99         max        mean
GET      772000       0.003       0.047       0.171       0.262       0.450       0.475       0.075
POST     228000       0.005       0.046       0.182       0.248       0.317       0.395       0.074

$ logscope access.log --top path -n 3
top 3 path (of 1000000 records)
/api/users        310000   31.0%  ██████████████████████████████
/static/app.js    264000   26.4%  ██████████████████████████
/api/orders       194000   19.4%  ███████████████████
```

## What it does

- Parses JSON (nested objects flattened to `http.status`), logfmt, and nginx/Apache
  combined or common format with an optional trailing `$request_time`.
- Detects the format per line, so `kubectl logs` or `docker compose logs` output with
  mixed formats works without flags. Plain text lines become `message`.
- Filters with a small query language: `== != > >= < <=`, regex `~` and `!~`,
  `contains`, field existence, `and`/`or`/`not`, parentheses. Comparison is numeric
  when both sides are numbers and lexicographic otherwise, which is what makes
  `ts >= 2026-10-16T10:00:00Z` work on ISO-8601 timestamps.
- Aggregates with `--top FIELD` and `--stats FIELD [--by GROUP]` (count, min, p50,
  p90, p95, p99, max, mean; exact, NumPy-compatible interpolation).
- Follows files with `-f` like `tail -F`, including truncation and rotation.
- Behaves like a Unix tool: reads stdin, survives `| head`, exits `0` on match, `1` on
  no match, `2` on error, honours `NO_COLOR`.

## How it works

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
| `input.rs` | One thread per source, bounded `sync_channel`, follow/rotation, 1 MiB line cap, lossy UTF-8 |
| `parse.rs` | JSON, logfmt and nginx parsers; per-line detection |
| `query.rs` | Tokenizer and recursive-descent parser to an AST; regexes compiled once |
| `stats.rs` | Top-N counter, exact percentiles |
| `output.rs` | Rendering, colours, projection, control-character escaping |

## Install and run

```bash
cargo install --git https://github.com/HOFFMAN-ALTERNATIVE/logscope
# or take a prebuilt Linux/macOS/Windows binary from Releases
```

```bash
logscope app.jsonl -w 'level == error'
logscope -f /var/log/nginx/access.log -w 'status >= 500 or request_time > 1'
logscope app.jsonl -w '(service == api or service == billing) and msg ~ "timeout|refused"' -o json
logscope access.log -w 'path == /api/login' --top remote_addr -n 10
logscope worker.logfmt --stats duration --by queue
logscope access.log -w 'status == 404' --count
docker compose logs --no-log-prefix | logscope -w 'level == error or status >= 500'
```

Sample inputs live in [`examples/`](examples).

## Performance

Measured 2026-10-17, release build, 8-vCPU Xeon VM, best of 3 runs, 1,000,000 lines
built by repeating the files in `examples/`:

| Workload | Input | Time | Throughput |
|---|---|---|---|
| nginx, `-w 'status >= 500' --count` | 100 MB | 2.49 s | ~400k lines/s |
| nginx, `--stats request_time` | 100 MB | 2.50 s | ~400k lines/s |
| JSON, `-w 'level == error' --count` | 1M lines | 1.74 s | ~575k lines/s |
| JSON, `--stats latency_ms --by service` | 1M lines | 1.72 s | ~580k lines/s |

Parsing is single-threaded. The CI `smoke` job re-runs the nginx case on every push so
regressions show up in the job log.

## Design decisions

- [ADR 0001](docs/adr/0001-per-line-format-detection.md): detect the format per line,
  because mixed streams are the normal case in containers. `--format` pins it,
  `--strict` fails on unparsable lines.
- [ADR 0002](docs/adr/0002-exact-percentiles.md): store samples and sort once instead of
  using t-digest. 1M values cost about 8 MB, and the numbers match NumPy exactly.
- [ADR 0003](docs/adr/0003-polling-follow-mode.md): poll every 200 ms for `-f` instead
  of inotify. Works on macOS, Windows and network filesystems with no extra crates.
- Records are an ordered `Vec<(String, Value)>`, not a `HashMap`. Log lines have
  5-20 fields, linear lookup beats hashing at that size, and output keeps source order.
- Four runtime dependencies: `clap`, `regex`, `serde_json`, `anyhow`. ANSI colours
  are hand-written.

## Limits

- `--top`/`--stats` cannot be combined with `-f`. Exact percentiles need all samples;
  a streaming sketch would be required for unbounded input.
- Lines longer than 1 MiB are truncated and the rest of the line is dropped.
- Single-threaded parsing. Parallel parsing with order-preserving reassembly is the
  next optimisation if 400k lines/s stops being enough.
- No time-range index: every query is a full scan.

## Security

Log content is treated as untrusted. Control characters in fields are escaped before
they reach the terminal, so a crafted user agent cannot inject ANSI or OSC sequences.
See [SECURITY.md](SECURITY.md).

## Development

```bash
cargo test                                  # 19 unit + 16 CLI integration tests
cargo clippy --all-targets -- -D warnings
cargo fmt --check
cargo audit
```

CI runs rustfmt and clippy on the MSRV toolchain (1.85), tests on Linux and macOS, and
the release-build throughput smoke test. Tagging `v*` builds release binaries.

## License

MIT © Herman Hoffman
