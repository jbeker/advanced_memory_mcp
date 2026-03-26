# Advanced Memory MCP

A multi-user [Model Context Protocol](https://modelcontextprotocol.io/) (MCP) server that provides persistent knowledge graph management. Built with [FastMCP](https://github.com/jlowin/fastmcp), it enables Claude and other MCP clients to store and query structured knowledge with per-user data isolation and token-based access control.

## Features

- **Knowledge graph storage** -- entities, relations, and observations persisted as JSONL
- **Multi-user support** -- each token maps to its own data file; multiple tokens can share a file for team access
- **Token-based authentication** -- Bearer tokens with read-only or read-write permission modes
- **Entity type normalization** -- configurable alias mappings with Title Case fallback
- **Automatic timestamps** -- ISO 8601 UTC `createdAt` and `lastUpdated` on all entities and relations
- **Deduplication** -- entities deduplicated by name, relations by `(from, to, relationType)` tuple
- **Stateless HTTP mode** -- survives server restarts; supports horizontal scaling
- **Docker-ready** -- Dockerfile and docker-compose configuration included

## Quick Start

### Prerequisites

- Python 3.11+
- [uv](https://github.com/astral-sh/uv) package manager

### Install and Run

```bash
# Install dependencies
uv sync

# Generate a token
python generate_token.py myuser \
  --config tokens.json \
  --file myuser.jsonl \
  --mode read-write

# Start the server
uv run advanced-memory-mcp \
  --data-dir ./data \
  --token-config ./tokens.json \
  --type-config ./entity_types.json
```

The server starts on `http://0.0.0.0:8765` by default.

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
| `--transport` | `MCP_TRANSPORT` | `http` | `http`, `sse`, or `streamable-http` |

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

A fallback `MEMORY_TOKEN` environment variable is checked if no header is present.

## MCP Client Configuration

To use this server with Claude Desktop or another MCP client, configure it as a remote MCP server pointing to the host and port where it's running. The client must send a Bearer token in the Authorization header with each request.

## Data Storage

Knowledge graphs are stored as JSONL files (one JSON object per line) in the configured data directory. Each line is either an entity or a relation:

```jsonl
{"type": "entity", "name": "Alice", "entityType": "Person", "observations": ["Engineer", "Works on Project X"], "createdAt": "2026-03-23T14:30:00+00:00", "lastUpdated": "2026-03-23T14:30:00+00:00"}
{"type": "relation", "from": "Alice", "to": "Project X", "relationType": "worksOn", "createdAt": "2026-03-23T14:30:00+00:00", "lastUpdated": "2026-03-23T14:30:00+00:00"}
```

## License

All rights reserved.
