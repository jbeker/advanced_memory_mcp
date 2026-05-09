"""Web UI package for the Advanced Memory MCP server.

Exposes two entry points:

- `build_starlette_app(...)` returns a standalone Starlette app, used by tests
  and anyone who wants to mount the UI under their own ASGI runner.
- `register_routes(mcp, ...)` mounts the same routes onto an existing FastMCP
  instance via `@mcp.custom_route()`, sharing the MCP server's port.
"""

from __future__ import annotations

import mimetypes
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

from jinja2 import Environment, FileSystemLoader, select_autoescape
from starlette.applications import Starlette
from starlette.requests import Request
from starlette.responses import FileResponse, Response
from starlette.routing import Route

from knowledge_graph import KnowledgeGraphManager
from token_config import TokenConfig

from .api import delete_entity, delete_observation, graph_data, update_observation
from .views import (
    entity_detail,
    entity_list,
    entity_list_rows,
    graph_page,
    login_get,
    login_post,
    logout,
)


_PACKAGE_DIR = Path(__file__).resolve().parent
_STATIC_DIR = _PACKAGE_DIR / "static"
_TEMPLATE_DIR = _PACKAGE_DIR / "templates"


@dataclass(frozen=True)
class WebUIDeps:
    token_config: TokenConfig
    get_manager: Callable[[str], KnowledgeGraphManager]
    ui_secret: str
    templates: Environment


def _make_template_env() -> Environment:
    return Environment(
        loader=FileSystemLoader(str(_TEMPLATE_DIR)),
        autoescape=select_autoescape(["html"]),
    )


async def _serve_static(request: Request) -> Response:
    rel = request.path_params["path"]
    target = (_STATIC_DIR / rel).resolve()
    if not target.is_file() or not target.is_relative_to(_STATIC_DIR.resolve()):
        return Response(status_code=404)
    media_type, _ = mimetypes.guess_type(target.name)
    return FileResponse(target, media_type=media_type or "application/octet-stream")


def _build_routes() -> list[Route]:
    return [
        Route("/ui/login", login_get, methods=["GET"]),
        Route("/ui/login", login_post, methods=["POST"]),
        Route("/ui/logout", logout, methods=["POST"]),
        Route("/ui/", entity_list, methods=["GET"]),
        Route("/ui/search", entity_list_rows, methods=["GET"]),
        Route("/ui/entity/{name}", entity_detail, methods=["GET"]),
        Route("/ui/graph", graph_page, methods=["GET"]),
        Route("/ui/api/graph", graph_data, methods=["GET"]),
        Route("/ui/api/observations", update_observation, methods=["PATCH"]),
        Route("/ui/api/observations/delete", delete_observation, methods=["POST"]),
        Route("/ui/api/entities/{name}", delete_entity, methods=["DELETE"]),
        Route("/ui/static/{path:path}", _serve_static, methods=["GET"]),
    ]


def build_starlette_app(
    *,
    token_config: TokenConfig,
    get_manager: Callable[[str], KnowledgeGraphManager],
    ui_secret: str,
) -> Starlette:
    app = Starlette(routes=_build_routes())
    app.state.webui_deps = WebUIDeps(
        token_config=token_config,
        get_manager=get_manager,
        ui_secret=ui_secret,
        templates=_make_template_env(),
    )
    return app


def register_routes(
    mcp,
    *,
    token_config: TokenConfig,
    get_manager: Callable[[str], KnowledgeGraphManager],
    ui_secret: str,
) -> None:
    """Register UI routes onto an existing FastMCP instance."""
    deps = WebUIDeps(
        token_config=token_config,
        get_manager=get_manager,
        ui_secret=ui_secret,
        templates=_make_template_env(),
    )
    for route in _build_routes():
        # FastMCP wraps the handler in a Starlette Route under the hood, so
        # request.app.state.webui_deps would point to the FastMCP app's state,
        # not ours. Stash deps on the request directly via a thin shim.
        endpoint = _wrap_with_deps(route.endpoint, deps)
        mcp.custom_route(route.path, methods=list(route.methods or []))(endpoint)


def _wrap_with_deps(endpoint: Callable, deps: WebUIDeps) -> Callable:
    async def wrapped(request: Request) -> Response:
        # Attach deps to the underlying app state so handlers' existing
        # `request.app.state.webui_deps` lookup keeps working.
        try:
            request.app.state.webui_deps = deps
        except Exception:
            pass
        return await endpoint(request)

    wrapped.__name__ = endpoint.__name__
    return wrapped
