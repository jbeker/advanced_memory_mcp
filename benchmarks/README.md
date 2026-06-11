# MCP Benchmarks

`mcp_bench.py` is a black-box performance benchmark for any server that
implements the Advanced Memory MCP tool set. It speaks only the public MCP
streamable-HTTP protocol, so the same script can compare this Python server
against a reimplementation in another language.

## Warning

The benchmark creates and deletes thousands of entities through the public
API. Point it at a **throwaway token and data file**, never at real data. It
cleans up after itself unless `--keep-data` is passed.

## Usage

Start a server with a scratch data dir and token, then:

```bash
uv run benchmarks/mcp_bench.py \
  --url http://127.0.0.1:8765/mcp \
  --token <read-write-token> \
  --sizes 100,1000,10000 \
  --json results.json
```

Dependencies are declared inline (PEP 723); `uv run` resolves them
automatically with no project install.

## What it measures

For each graph size in `--sizes` (seeded cumulatively through
`create_entities` / `create_relations`):

- `search_nodes`, `open_nodes`, `read_graph` — read-path latency
- `create_entities`, `add_observations`, `create_relations` (one item each) —
  write-path latency

It then runs a concurrency check: N parallel single-entity creates over
separate connections (default 40, `--race-concurrency 0` to skip), followed by
a count of survivors. Lost entities indicate read-modify-write races in the
server's storage layer. The script exits non-zero if any loss is detected.

All numbers are median/min/max milliseconds over `--repeats` calls (default
10), written to stdout as a table and optionally to `--json` for cross-version
comparison.
