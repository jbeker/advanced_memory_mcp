import argparse
import json
import os
import secrets
import tomllib
from pathlib import Path

from fastmcp import FastMCP, Context
from fastmcp.server.dependencies import get_http_request

from knowledge_graph import KnowledgeGraphManager
from token_config import TokenConfig
from webui import register_routes as register_webui_routes

# Global state
_data_dir: str = ""
_token_config: TokenConfig | None = None
_type_aliases: dict[str, str] = {}
_managers: dict[str, KnowledgeGraphManager] = {}


def _get_token(ctx: Context) -> str:
    """Extract bearer token from HTTP Authorization header and validate against config."""
    token = None

    # Extract Bearer token from the HTTP Authorization header
    try:
        request = get_http_request()
        auth_header = request.headers.get("authorization", "")
        if auth_header.lower().startswith("bearer "):
            token = auth_header[7:]
    except RuntimeError:
        pass

    # Fallback: check if there's a default token via environment
    if not token:
        token = os.environ.get("MEMORY_TOKEN")

    if not token:
        raise ValueError(
            "No authentication token provided. Send a Bearer token in the Authorization header."
        )

    # Validate token against config
    _token_config.get(token)
    return token


def get_graph_manager(token: str) -> KnowledgeGraphManager:
    """Get or create a KnowledgeGraphManager for the given token's data file."""
    entry = _token_config.get(token)
    if entry.file not in _managers:
        file_path = os.path.join(_data_dir, entry.file)
        _managers[entry.file] = KnowledgeGraphManager(file_path, type_aliases=_type_aliases)
    return _managers[entry.file]


def _check_write_permission(token: str) -> None:
    """Check if the token has write permission."""
    entry = _token_config.get(token)
    if entry.mode == "read-only":
        raise ValueError("This token has read-only access. Write operations are not allowed.")


def register_tools(mcp: FastMCP) -> None:
    @mcp.tool()
    def create_entities(entities: list[dict], ctx: Context) -> dict:
        """Create multiple new entities in the knowledge graph.

        Each entity should have 'name', 'entityType', and 'observations' fields.
        Deduplicates by entity name - existing entities are skipped.
        New entities receive 'createdAt' and 'lastUpdated' ISO 8601 UTC timestamps."""
        token = _get_token(ctx)
        _check_write_permission(token)
        manager = get_graph_manager(token)
        created = manager.create_entities(entities)
        return {"created": created}

    @mcp.tool()
    def create_relations(relations: list[dict], ctx: Context) -> dict:
        """Create multiple new relations between entities.

        Each relation should have 'from', 'to', and 'relationType' fields.
        Deduplicates by the (from, to, relationType) tuple.
        New relations receive 'createdAt' and 'lastUpdated' ISO 8601 UTC timestamps."""
        token = _get_token(ctx)
        _check_write_permission(token)
        manager = get_graph_manager(token)
        created = manager.create_relations(relations)
        return {"created": created}

    @mcp.tool()
    def add_observations(observations: list[dict], ctx: Context) -> dict:
        """Add new observations to existing entities.

        Each observation should have 'entityName' and 'contents' (list of strings).
        Returns error if entity doesn't exist. Deduplicates observations."""
        token = _get_token(ctx)
        _check_write_permission(token)
        manager = get_graph_manager(token)
        results = manager.add_observations(observations)
        return {"results": results}

    @mcp.tool()
    def delete_entities(entityNames: list[str], ctx: Context) -> dict:
        """Delete entities and their associated relations from the knowledge graph.

        Takes a list of entity names to delete. Relations involving deleted entities
        are also removed (cascade delete)."""
        token = _get_token(ctx)
        _check_write_permission(token)
        manager = get_graph_manager(token)
        manager.delete_entities(entityNames)
        return {"deleted": entityNames}

    @mcp.tool()
    def delete_observations(deletions: list[dict], ctx: Context) -> dict:
        """Delete specific observations from entities.

        Each deletion should have 'entityName' and 'observations' (list of strings to remove).
        Silently ignores missing entities or observations."""
        token = _get_token(ctx)
        _check_write_permission(token)
        manager = get_graph_manager(token)
        manager.delete_observations(deletions)
        return {"deleted": deletions}

    @mcp.tool()
    def delete_relations(relations: list[dict], ctx: Context) -> dict:
        """Delete specific relations from the knowledge graph.

        Each relation should have 'from', 'to', and 'relationType' fields.
        All three fields must match for deletion."""
        token = _get_token(ctx)
        _check_write_permission(token)
        manager = get_graph_manager(token)
        manager.delete_relations(relations)
        return {"deleted": relations}

    @mcp.tool()
    def rename_entity(name: str, new_name: str, ctx: Context) -> dict:
        """Rename an entity in the knowledge graph.

        Updates the entity's name and rewrites every relation that references it
        (both 'from' and 'to' endpoints). Errors if the source entity is not found,
        if an entity with 'new_name' already exists (use merge_entities to combine
        them), or if name == new_name. Bumps lastUpdated on the entity and on every
        touched relation. Returns {renamed: {from, to}, relationsUpdated: N}."""
        token = _get_token(ctx)
        _check_write_permission(token)
        manager = get_graph_manager(token)
        return manager.rename_entity(name, new_name)

    @mcp.tool()
    def merge_entities(source: str, target: str, ctx: Context) -> dict:
        """Merge the source entity into the target entity.

        The source is removed; the target absorbs the source's observations
        (unioned, target order preserved) and relations (re-pointed, self-loops
        dropped, duplicates collapsed keeping the earliest createdAt). Target's
        entityType is kept; if source's entityType differed it is returned as
        'discardedType' in the response. createdAt becomes the earliest non-null
        of the two. Errors if either entity is missing or source == target."""
        token = _get_token(ctx)
        _check_write_permission(token)
        manager = get_graph_manager(token)
        return manager.merge_entities(source, target)

    @mcp.tool()
    def read_graph(ctx: Context) -> dict:
        """Read the entire knowledge graph.

        Returns all entities and relations for the authenticated user.
        Entities include 'createdAt' and 'lastUpdated' timestamps (null for legacy data)."""
        token = _get_token(ctx)
        manager = get_graph_manager(token)
        return manager.read_graph()

    @mcp.tool()
    def search_nodes(query: str, ctx: Context) -> dict:
        """Search for nodes in the knowledge graph.

        Performs case-insensitive search across entity names, types, and observations.
        Returns matching entities (with 'createdAt'/'lastUpdated' timestamps) and any
        relations where at least one endpoint matches."""
        token = _get_token(ctx)
        manager = get_graph_manager(token)
        return manager.search_nodes(query)

    @mcp.tool()
    def open_nodes(names: list[str], ctx: Context) -> dict:
        """Open specific nodes by name from the knowledge graph.

        Returns the requested entities (with 'createdAt'/'lastUpdated' timestamps) and
        any relations where at least one endpoint is in the requested set."""
        token = _get_token(ctx)
        manager = get_graph_manager(token)
        return manager.open_nodes(names)

    @mcp.tool()
    def normalize_entity_types(ctx: Context) -> dict:
        """Normalize all entity types in the knowledge graph using configured aliases.

        Applies type alias mappings and Title Case normalization to all existing entities.
        Returns a summary of changes made. Read-only tokens are rejected."""
        token = _get_token(ctx)
        _check_write_permission(token)
        manager = get_graph_manager(token)
        return manager.normalize_all_entity_types()


def main():
    global _data_dir, _token_config, _type_aliases

    parser = argparse.ArgumentParser(description="Advanced Memory MCP Server")
    parser.add_argument("--host", default=os.environ.get("MCP_HOST", "0.0.0.0"), help="Host to bind to (default: 0.0.0.0, env: MCP_HOST)")
    parser.add_argument("--port", type=int, default=int(os.environ.get("MCP_PORT", "8765")), help="Port to listen on (default: 8765, env: MCP_PORT)")
    parser.add_argument("--data-dir", default=os.environ.get("MCP_DATA_DIR"), help="Directory for per-user JSONL data files (env: MCP_DATA_DIR)")
    parser.add_argument(
        "--token-config",
        default=os.environ.get("MCP_TOKEN_CONFIG"),
        help="Path to tokens.json config file (env: MCP_TOKEN_CONFIG)",
    )
    parser.add_argument(
        "--type-config",
        default=os.environ.get("MCP_TYPE_CONFIG"),
        help="Path to entity_types.json config file (env: MCP_TYPE_CONFIG)",
    )
    parser.add_argument(
        "--transport",
        choices=["http", "sse", "streamable-http"],
        default=os.environ.get("MCP_TRANSPORT", "http"),
        help="Transport protocol (default: http, env: MCP_TRANSPORT)",
    )

    args = parser.parse_args()

    if not args.data_dir:
        parser.error("--data-dir is required (or set MCP_DATA_DIR)")

    if not args.token_config:
        parser.error("--token-config is required (or set MCP_TOKEN_CONFIG)")

    _data_dir = args.data_dir
    _token_config = TokenConfig(args.token_config)

    if args.type_config:
        type_config_path = Path(args.type_config)
        if type_config_path.exists():
            with open(type_config_path) as f:
                type_config = json.load(f)
            _type_aliases = type_config.get("aliases", {})

    # Ensure data directory exists
    Path(_data_dir).mkdir(parents=True, exist_ok=True)

    _pyproject = Path(__file__).parent / "pyproject.toml"
    with open(_pyproject, "rb") as f:
        server_version = tomllib.load(f)["project"]["version"]
    import logging
    logging.basicConfig(level=logging.INFO)
    log = logging.getLogger("advanced-memory-mcp")
    log.info(f"Advanced Memory MCP v{server_version}")
    log.info(f"Transport: {args.transport} | Host: {args.host} | Port: {args.port}")
    log.info(f"Data dir: {_data_dir}")
    log.info(f"Type aliases loaded: {len(_type_aliases)}")

    ui_secret = os.environ.get("MEMORY_UI_SECRET")
    if not ui_secret:
        ui_secret = secrets.token_urlsafe(32)
        log.warning(
            "MEMORY_UI_SECRET not set; using ephemeral secret. "
            "UI sessions will be invalidated when the server restarts."
        )

    mcp = FastMCP("Advanced Memory MCP")
    register_tools(mcp)
    register_webui_routes(
        mcp,
        token_config=_token_config,
        get_manager=get_graph_manager,
        ui_secret=ui_secret,
    )
    log.info("Web UI available at /ui/")
    mcp.run(transport=args.transport, host=args.host, port=args.port, stateless_http=True)


if __name__ == "__main__":
    main()
