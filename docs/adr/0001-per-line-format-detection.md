# 0001. Detect the log format per line

Status: accepted
Date: 2026-10-17

## Context
The main use is incident triage on a live host. Input often comes from
`kubectl logs` or `docker compose logs`, where one stream mixes JSON from the app,
logfmt from a sidecar and plain text from a crashing process. A tool that picks one
format at startup drops or mis-parses most of such a stream.

## Options
1. Detect once from the first lines: cheapest, but wrong on mixed streams.
2. Detect per line: first-byte check (`{` means JSON), then one anchored regex for the
   nginx shape, logfmt otherwise, plain text as last resort. Costs a little CPU per line.
3. Require `--format` always: predictable, but makes the common case tedious.

## Decision
Option 2 by default, with `--format` to pin a format and `--strict` to fail on lines
that do not parse. Correct output on mixed streams matters more than a few percent of
throughput.

## Consequences
- Mixed container logs work with no flags.
- A logfmt-looking plain-text line can be parsed as logfmt. `--format` is the escape hatch.
- Detection cost is part of every benchmark number in the README.
- Revisit if a format is added that cannot be told apart cheaply from its first bytes.
