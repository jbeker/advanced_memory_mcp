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

## Baseline results

`results/` holds committed runs for comparing implementations. Name files
`<implementation>-v<version>.json`.

`results/python-v0.2.0.json` is the Python server at v0.2.0 on an Apple
Silicon Mac (local loopback, 2026-06-11). Highlights:

| entities | search_nodes | open_nodes | read_graph | create_entities(1) |
|---------:|-------------:|-----------:|-----------:|-------------------:|
|      100 |       4.4 ms |     4.0 ms |     7.2 ms |             4.3 ms |
|    1,000 |        10 ms |     6.5 ms |      32 ms |              12 ms |
|   10,000 |        73 ms |      40 ms |     308 ms |              80 ms |

Latency grows linearly with graph size because every operation re-reads, and
every write rewrites, the whole JSONL file. The race check failed: 40
concurrent creates produced 39 errors and corrupted the store (NUL bytes and
truncated lines), after which all reads failed. Any refresh should beat both
the latency curve and the race check.

`results/rust-v0.3.0.json` is the Rust server on the same machine
(2026-06-11). The race check passes (40/40 concurrent creates survive,
zero errors):

| entities | search_nodes | open_nodes | read_graph | create_entities(1) |
|---------:|-------------:|-----------:|-----------:|-------------------:|
|      100 |       1.8 ms |     1.5 ms |     2.8 ms |             6.7 ms |
|    1,000 |       3.4 ms |     1.7 ms |      15 ms |             8.2 ms |
|   10,000 |        18 ms |     1.7 ms |     148 ms |              21 ms |
|   50,000 |        27 ms |     3.2 ms |     769 ms |              80 ms |

Reads are served from memory (no file I/O); writes persist the whole file
via temp-file + fsync + atomic rename, which is why a single-entity write
costs more at small sizes than Python's unsynced write (6.7 ms vs 4.3 ms at
100 entities) but scales far better (21 ms vs 80 ms at 10k). `read_graph`
remains size-bound because it serializes the entire graph over the wire.

## Parity checking

`parity_check.py` replays the captured fixture sequence
(`fixtures/python-v0.2.0/`) against a live server and diffs every result
against the Python ground truth (timestamps masked):

```bash
uv run benchmarks/parity_check.py --url http://127.0.0.1:8765/mcp \
  --rw-token <rw-token> --ro-token <ro-token>
```

The target must be freshly started on an empty data file with a read-write
and a read-only token sharing that file. The Rust v0.3.0 server passes with
only FastMCP-internal tool metadata (`meta.fastmcp.tags`) differing.
