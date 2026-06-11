# Advanced Memory MCP

A multi-user [Model Context Protocol](https://modelcontextprotocol.io/) (MCP) server that provides persistent knowledge graph management. This branch is the **Rust implementation** (v0.3.x): the same MCP tool surface and data file formats as the Python v0.2.x server, reimplemented for speed and safety. The Python sources remain in the tree for reference and parity testing.

## Features

- **Knowledge graph storage** -- entities, relations, and observations persisted as JSONL
- **Multi-user support** -- each token maps to its own data file; multiple tokens can share a file for team access
- **Token-based authentication** -- Bearer tokens with read-only or read-write permission modes
- **Entity type normalization** -- configurable alias mappings with Title Case fallback
- **Automatic timestamps** -- ISO 8601 UTC `createdAt` and `lastUpdated` on all entities and relations
- **Deduplication** -- entities deduplicated by name, relations by `(from, to, relationType)` tuple
- **In-memory storage engine** -- each data file is loaded once and served from memory; mutations persist via temp-file + fsync + atomic rename, so concurrent writes cannot corrupt the store
- **Web UI** -- entity browser, search, and graph view served from the same binary at `/ui/`
- **Docker-ready** -- multi-stage Dockerfile (82 MB image) and docker-compose configuration included

## Quick Start

### Prerequisites

- Rust toolchain (stable)

### Install and Run

```bash
# Generate a token (any way of editing tokens.json works; the Python
# helper is still included)
python3 generate_token.py myuser \
  --config tokens.json \
  --file myuser.jsonl \
  --mode read-write

# Build and start the server
cargo run --release -- \
  --data-dir ./data \
  --token-config ./tokens.json \
  --type-config ./entity_types.json
```

The server starts on `http://0.0.0.0:8765` by default.

### Tests and benchmarks

```bash
cargo test                          # unit + integration suites
uv run benchmarks/parity_check.py   # wire-level parity vs Python fixtures
uv run benchmarks/mcp_bench.py      # performance benchmark (see benchmarks/README.md)
```

### Docker

```bash
docker build -t advanced-memory-mcp .

docker run -p 8765:8765 \
  -v memory-data:/data \
  -v ./tokens.json:/config/tokens.json:ro \
  -v ./entity_types.json:/config/entity_types.json:ro \
  advanced-memory-mcp
```

Or with docker-compose:

```bash
docker-compose up -d
```

## Configuration

### Command-Line Arguments / Environment Variables

| Argument | Env Var | Default | Description |
|---|---|---|---|
| `--host` | `MCP_HOST` | `0.0.0.0` | Bind address |
| `--port` | `MCP_PORT` | `8765` | Listen port |
| `--data-dir` | `MCP_DATA_DIR` | *(required)* | Directory for JSONL data files |
| `--token-config` | `MCP_TOKEN_CONFIG` | *(required)* | Path to `tokens.json` |
| `--type-config` | `MCP_TYPE_CONFIG` | *(none)* | Path to `entity_types.json` |
| `--transport` | `MCP_TRANSPORT` | `http` | `http` or `streamable-http` (aliases for the MCP streamable HTTP transport; the deprecated `sse` transport was removed) |

`MEMORY_UI_SECRET` signs web UI session cookies; if unset, an ephemeral
secret is generated and sessions reset on restart.

### tokens.json

Maps bearer tokens to data files and permission levels:

```json
{
  "myuser_a1b2c3...": {
    "file": "myuser.jsonl",
    "mode": "read-write"
  },
  "viewer_d4e5f6...": {
    "file": "myuser.jsonl",
    "mode": "read-only"
  }
}
```

Generate tokens with the included utility:

```bash
python generate_token.py <username> \
  --config tokens.json \
  --file <data_file> \
  --mode read-write|read-only
```

### entity_types.json

Optional alias mappings for entity type normalization. When an entity is created, its type is matched case-insensitively against this map. Unmatched types fall back to Title Case.

```json
{
  "aliases": {
    "person": "Person",
    "colleague": "Person",
    "project": "Project",
    "company": "Organization"
  }
}
```

## MCP Tools

### Write Operations (read-write tokens only)

| Tool | Description |
|---|---|
| `create_entities` | Create entities with name, type, and observations. Skips duplicates. |
| `create_relations` | Create directed relations between entities (`from`, `to`, `relationType`). |
| `add_observations` | Append observations to existing entities. |
| `delete_entities` | Delete entities and cascade-delete their relations. |
| `delete_observations` | Remove specific observations from entities. |
| `delete_relations` | Remove specific relations by exact match. |
| `rename_entity` | Rename an entity and rewrite every relation that references it. Errors on target collision. |
| `merge_entities` | Merge a source entity into a target: union observations, re-point relations, drop self-loops and duplicates. |
| `normalize_entity_types` | Apply type alias mappings to all existing entities. |

### Read Operations (all tokens)

| Tool | Description |
|---|---|
| `read_graph` | Return the full knowledge graph (entities and relations). |
| `search_nodes` | Case-insensitive search across entity names, types, and observations. |
| `open_nodes` | Retrieve specific entities by name with their related relations. |

## Authentication

Clients authenticate by sending a Bearer token in the HTTP `Authorization` header:

```
Authorization: Bearer myuser_a1b2c3...
```

A Bearer token is required on every request. (The Python version's
`MEMORY_TOKEN` environment fallback, which silently authenticated
header-less requests, was removed.)

## MCP Client Configuration

To use this server with Claude Desktop or another MCP client, configure it as a remote MCP server pointing to the host and port where it's running. The client must send a Bearer token in the Authorization header with each request.

## Data Storage

Knowledge graphs are stored as JSONL files (one JSON object per line) in the configured data directory. Each line is either an entity or a relation:

```jsonl
{"type": "entity", "name": "Alice", "entityType": "Person", "observations": ["Engineer", "Works on Project X"], "createdAt": "2026-03-23T14:30:00+00:00", "lastUpdated": "2026-03-23T14:30:00+00:00"}
{"type": "relation", "from": "Alice", "to": "Project X", "relationType": "worksOn", "createdAt": "2026-03-23T14:30:00+00:00", "lastUpdated": "2026-03-23T14:30:00+00:00"}
```

## Differences from the Python implementation (v0.2.x)

The MCP wire contract and data file format are identical — verified by
`benchmarks/parity_check.py` against captured fixtures, and data files are
byte-identical modulo timestamps. Intentional differences:

- **Auth**: `MEMORY_TOKEN` env fallback removed; token comparison is
  constant-time.
- **Transports**: `sse` removed (deprecated by the MCP spec); `http` and
  `streamable-http` both select streamable HTTP.
- **Storage**: in-memory with atomic, fsynced writes. Concurrent writes
  serialize instead of corrupting the file. A malformed line still fails the
  load, but the error names the file and line number.
- **Error text**: malformed tool inputs produce serde messages (e.g.
  ``missing field `name` ``) instead of Python's bare KeyError text.
  Documented error messages ("Entity 'X' not found", read-only rejection,
  unknown token) are unchanged.
- **Input hygiene**: a `"type"` key inside a submitted entity/relation is
  stripped; in Python it could clobber the JSONL line discriminator and make
  the row vanish on the next load.
- **Web UI**: identical routes and templates; sessions are HMAC-SHA256
  cookies (existing Python sessions are invalidated once at cutover); the
  graph page URL-encodes the center parameter, closing a reflected-XSS
  vector.
- **Tool listing**: tools are listed alphabetically and FastMCP's internal
  `meta.fastmcp.tags` is absent.

## License

All rights reserved.
