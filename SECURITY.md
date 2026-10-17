# Security policy

## Reporting
Open a private advisory via GitHub "Report a vulnerability" on this repository.
Please do not file public issues for security problems. Expect a first answer within
5 working days.

## Supported versions
Only the latest release and `main` receive fixes.

## Threat model
- Asset: the terminal and machine of the person reading logs.
- Actor: anyone who can get text into a log line (user agents, request paths, error
  messages from upstream services).
- Entry points: files, stdin, follow mode.
- Risks and mitigations:
  - Terminal escape injection (ANSI/OSC sequences that rewrite the screen or set the
    clipboard): control characters in field values and keys are escaped in pretty and
    logfmt output; JSON output is escaped by `serde_json`.
  - Memory exhaustion from a newline-free or binary input: lines are capped at 1 MiB,
    the remainder of the line is discarded.
  - Regex denial of service in `-w` queries: the `regex` crate guarantees linear-time
    matching, no backtracking.
  - Invalid UTF-8: decoded lossily, never panics.
- Out of scope: `--stats` memory on very large inputs is bounded only by input size
  (see ADR 0002).

## Supply chain
`cargo audit` runs before every release; dependencies are locked in `Cargo.lock` and
CI builds with `--locked`.
