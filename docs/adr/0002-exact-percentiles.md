# 0002. Exact percentiles instead of a streaming sketch

Status: accepted
Date: 2026-10-17

## Context
`--stats` reports p50 to p99. Inputs are files or pipes that end, typically up to a
few million lines. People compare the output against dashboards and scripts, so
numbers that drift from NumPy or Prometheus by a sketch error bound cause confusion.

## Options
1. Store every sample as `f64`, sort once, interpolate linearly: exact, 8 bytes per value.
2. t-digest or HDR histogram: constant memory, works on unbounded streams, approximate.

## Decision
Option 1. At CLI scale the memory is small (1M values is about 8 MB) and exact output
removes a whole class of "why does this differ" questions.

## Consequences
- Results match NumPy's default percentile method.
- Memory grows linearly with matched values, so aggregations are disabled with `-f`.
- Revisit when follow-mode aggregation is needed; that will require a sketch and an
  explicit "approximate" label in the output.
