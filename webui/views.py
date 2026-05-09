"""HTML page handlers for the web UI."""

from __future__ import annotations

from urllib.parse import urlparse

from starlette.exceptions import HTTPException
from starlette.requests import Request
from starlette.responses import HTMLResponse, RedirectResponse, Response

from .auth import (
    clear_session_cookie,
    issue_session_cookie,
    login_required_html,
)


def _filter_entities(
    entities: list[dict], query: str | None, type_filter: str | None
) -> list[dict]:
    """Filter entities by case-insensitive substring across name/type/observations
    AND an exact entityType match. Empty filters mean no constraint."""
    q = (query or "").strip().lower()
    t = (type_filter or "").strip()

    def matches(entity: dict) -> bool:
        if t and entity.get("entityType") != t:
            return False
        if q:
            if q in entity["name"].lower():
                return True
            if q in entity.get("entityType", "").lower():
                return True
            return any(q in obs.lower() for obs in entity.get("observations", []))
        return True

    return [e for e in entities if matches(e)]


def _safe_next(raw: str | None) -> str:
    """Only allow same-origin paths beginning with /ui/ as redirect targets."""
    if not raw:
        return "/ui/"
    parsed = urlparse(raw)
    if parsed.scheme or parsed.netloc:
        return "/ui/"
    if not parsed.path.startswith("/ui/"):
        return "/ui/"
    return raw


async def login_get(request: Request) -> Response:
    deps = request.app.state.webui_deps
    next_url = _safe_next(request.query_params.get("next"))
    html = deps.templates.get_template("login.html").render(
        error=None, next_url=next_url
    )
    return HTMLResponse(html)


async def login_post(request: Request) -> Response:
    deps = request.app.state.webui_deps
    form = await request.form()
    token = (form.get("token") or "").strip()
    next_url = _safe_next(request.query_params.get("next"))

    try:
        deps.token_config.get(token)
    except ValueError:
        html = deps.templates.get_template("login.html").render(
            error="Invalid token.", next_url=next_url
        )
        return HTMLResponse(html, status_code=200)

    response = RedirectResponse(url=next_url, status_code=303)
    issue_session_cookie(
        response,
        secret=deps.ui_secret,
        token=token,
        secure=request.url.scheme == "https",
    )
    return response


async def logout(request: Request) -> Response:
    response = RedirectResponse(url="/ui/login", status_code=303)
    clear_session_cookie(response)
    return response


def _user_label(token: str) -> str:
    """Show enough of the token to identify the session without leaking it.
    Tokens are formatted as `<username>_<hex>` by generate_token.py."""
    if "_" in token:
        return token.split("_", 1)[0]
    return token[:6]


@login_required_html
async def entity_list(request: Request) -> Response:
    deps = request.app.state.webui_deps
    user = request.state.current_user
    manager = deps.get_manager(user.token)
    graph = manager.read_graph()
    entities = sorted(graph["entities"], key=lambda e: e["name"].lower())
    distinct_types = sorted({e.get("entityType", "") for e in entities if e.get("entityType")})
    html = deps.templates.get_template("entity_list.html").render(
        entities=entities,
        distinct_types=distinct_types,
        user_label=_user_label(user.token),
        read_only=user.entry.mode == "read-only",
    )
    return HTMLResponse(html)


@login_required_html
async def entity_detail(request: Request) -> Response:
    deps = request.app.state.webui_deps
    user = request.state.current_user
    name = request.path_params["name"]
    manager = deps.get_manager(user.token)
    result = manager.open_nodes([name])
    matched = next((e for e in result["entities"] if e["name"] == name), None)
    if matched is None:
        raise HTTPException(status_code=404)
    relations = result["relations"]
    outgoing = [r for r in relations if r["from"] == name]
    incoming = [r for r in relations if r["to"] == name and r["from"] != name]
    html = deps.templates.get_template("entity_detail.html").render(
        entity=matched,
        outgoing=outgoing,
        incoming=incoming,
        user_label=_user_label(user.token),
        read_only=user.entry.mode == "read-only",
    )
    return HTMLResponse(html)


FULL_GRAPH_WARN_THRESHOLD = 500


@login_required_html
async def graph_page(request: Request) -> Response:
    deps = request.app.state.webui_deps
    user = request.state.current_user
    manager = deps.get_manager(user.token)
    graph = manager.read_graph()

    center = request.query_params.get("center")
    depth_raw = request.query_params.get("depth", "2")
    try:
        depth = max(1, min(int(depth_raw), 10))
    except ValueError:
        depth = 2
    is_full = request.query_params.get("all") == "1"
    confirmed = request.query_params.get("confirm") == "1"

    warn_large = False
    api_query = ""
    if center:
        if not any(e["name"] == center for e in graph["entities"]):
            raise HTTPException(status_code=404)
        api_query = f"center={center}&depth={depth}"
    elif is_full:
        if len(graph["entities"]) > FULL_GRAPH_WARN_THRESHOLD and not confirmed:
            warn_large = True
        api_query = "all=1"
    else:
        # Default: show full graph (with warning if large).
        if len(graph["entities"]) > FULL_GRAPH_WARN_THRESHOLD and not confirmed:
            warn_large = True
        api_query = "all=1"

    html = deps.templates.get_template("graph.html").render(
        center=center,
        depth=depth,
        entity_count=len(graph["entities"]),
        relation_count=len(graph["relations"]),
        warn_large=warn_large,
        api_query=api_query,
        user_label=_user_label(user.token),
        read_only=user.entry.mode == "read-only",
    )
    return HTMLResponse(html)


@login_required_html
async def entity_list_rows(request: Request) -> Response:
    """htmx fragment endpoint: returns just the <tbody> rows."""
    deps = request.app.state.webui_deps
    user = request.state.current_user
    manager = deps.get_manager(user.token)
    graph = manager.read_graph()
    filtered = _filter_entities(
        graph["entities"],
        request.query_params.get("q"),
        request.query_params.get("type"),
    )
    filtered.sort(key=lambda e: e["name"].lower())
    html = deps.templates.get_template("entity_list_rows.html").render(
        entities=filtered
    )
    return HTMLResponse(html)
