import argparse
import os
from pathlib import Path

from mcp.server.fastmcp import FastMCP, Context

from knowledge_graph import KnowledgeGraphManager

# Global state
_data_dir: str = ""
_mode: str = "read-only"
_managers: dict[str, KnowledgeGraphManager] = {}


def _get_token(ctx: Context) -> str:
    """Extract bearer token from request context."""
    request = ctx.request_context
    meta = getattr(request, "meta", None) or getattr(request, "_meta", None)
    if meta:
        token = getattr(meta, "auth_token", None)
        if token:
            return token

    # Fallback: check if there's a default token via environment
    token = os.environ.get("MEMORY_TOKEN")
    if token:
        return token

    raise ValueError(
        "No authentication token provided. Send a Bearer token in the Authorization header."
    )


def get_graph_manager(token: str) -> KnowledgeGraphManager:
    """Get or create a KnowledgeGraphManager for the given user token."""
    if token not in _managers:
        file_path = os.path.join(_data_dir, f"{token}.jsonl")
        _managers[token] = KnowledgeGraphManager(file_path)
    return _managers[token]


def _check_write_mode() -> None:
    if _mode == "read-only":
        raise ValueError("Server is in read-only mode. Write operations are not allowed.")


def register_tools(mcp: FastMCP) -> None:
    @mcp.tool()
    def create_entities(entities: list[dict], ctx: Context) -> dict:
        """Create multiple new entities in the knowledge graph.

        Each entity should have 'name', 'entityType', and 'observations' fields.
        Deduplicates by entity name - existing entities are skipped."""
        _check_write_mode()
        token = _get_token(ctx)
        manager = get_graph_manager(token)
        created = manager.create_entities(entities)
        return {"created": created}

    @mcp.tool()
    def create_relations(relations: list[dict], ctx: Context) -> dict:
        """Create multiple new relations between entities.

        Each relation should have 'from', 'to', and 'relationType' fields.
        Deduplicates by the (from, to, relationType) tuple."""
        _check_write_mode()
        token = _get_token(ctx)
        manager = get_graph_manager(token)
        created = manager.create_relations(relations)
        return {"created": created}

    @mcp.tool()
    def add_observations(observations: list[dict], ctx: Context) -> dict:
        """Add new observations to existing entities.

        Each observation should have 'entityName' and 'contents' (list of strings).
        Returns error if entity doesn't exist. Deduplicates observations."""
        _check_write_mode()
        token = _get_token(ctx)
        manager = get_graph_manager(token)
        results = manager.add_observations(observations)
        return {"results": results}

    @mcp.tool()
    def delete_entities(entityNames: list[str], ctx: Context) -> dict:
        """Delete entities and their associated relations from the knowledge graph.

        Takes a list of entity names to delete. Relations involving deleted entities
        are also removed (cascade delete)."""
        _check_write_mode()
        token = _get_token(ctx)
        manager = get_graph_manager(token)
        manager.delete_entities(entityNames)
        return {"deleted": entityNames}

    @mcp.tool()
    def delete_observations(deletions: list[dict], ctx: Context) -> dict:
        """Delete specific observations from entities.

        Each deletion should have 'entityName' and 'observations' (list of strings to remove).
        Silently ignores missing entities or observations."""
        _check_write_mode()
        token = _get_token(ctx)
        manager = get_graph_manager(token)
        manager.delete_observations(deletions)
        return {"deleted": deletions}

    @mcp.tool()
    def delete_relations(relations: list[dict], ctx: Context) -> dict:
        """Delete specific relations from the knowledge graph.

        Each relation should have 'from', 'to', and 'relationType' fields.
        All three fields must match for deletion."""
        _check_write_mode()
        token = _get_token(ctx)
        manager = get_graph_manager(token)
        manager.delete_relations(relations)
        return {"deleted": relations}

    @mcp.tool()
    def read_graph(ctx: Context) -> dict:
        """Read the entire knowledge graph.

        Returns all entities and relations for the authenticated user."""
        token = _get_token(ctx)
        manager = get_graph_manager(token)
        return manager.read_graph()

    @mcp.tool()
    def search_nodes(query: str, ctx: Context) -> dict:
        """Search for nodes in the knowledge graph.

        Performs case-insensitive search across entity names, types, and observations.
        Returns matching entities and any relations where at least one endpoint matches."""
        token = _get_token(ctx)
        manager = get_graph_manager(token)
        return manager.search_nodes(query)

    @mcp.tool()
    def open_nodes(names: list[str], ctx: Context) -> dict:
        """Open specific nodes by name from the knowledge graph.

        Returns the requested entities and any relations where at least one endpoint
        is in the requested set."""
        token = _get_token(ctx)
        manager = get_graph_manager(token)
        return manager.open_nodes(names)


def main():
    global _data_dir, _mode

    parser = argparse.ArgumentParser(description="Advanced Memory MCP Server")
    parser.add_argument("--host", default="0.0.0.0", help="Host to bind to (default: 0.0.0.0)")
    parser.add_argument("--port", type=int, default=8765, help="Port to listen on (default: 8765)")
    parser.add_argument("--data-dir", required=True, help="Directory for per-user JSONL data files")
    parser.add_argument(
        "--mode",
        choices=["read-write", "read-only"],
        default="read-only",
        help="Server mode (default: read-only)",
    )
    parser.add_argument(
        "--transport",
        choices=["sse", "streamable-http"],
        default="sse",
        help="Transport protocol (default: sse)",
    )

    args = parser.parse_args()

    _data_dir = args.data_dir
    _mode = args.mode

    # Ensure data directory exists
    Path(_data_dir).mkdir(parents=True, exist_ok=True)

    mcp = FastMCP("Advanced Memory MCP", host=args.host, port=args.port)
    register_tools(mcp)
    mcp.run(transport=args.transport)


if __name__ == "__main__":
    main()
