"""htmx mutation endpoints and JSON API for the web UI.

Mutation endpoints follow a small, consistent shape:
- Auth gate via @login_required_api (401 for unauthenticated requests).
- Permission gate via _require_write (403 for read-only tokens).
- Body parsed as JSON.
- Successful response is the HTML fragment htmx will swap into the page,
  except delete-entity which redirects via the HX-Redirect header.
"""

from __future__ import annotations

from starlette.exceptions import HTTPException
from starlette.requests import Request
from starlette.responses import HTMLResponse, JSONResponse, Response

from .auth import CurrentUser, login_required_api


def _require_write(user: CurrentUser) -> None:
    if user.entry.mode == "read-only":
        raise HTTPException(status_code=403, detail="Read-only token.")


@login_required_api
async def graph_data(request: Request) -> Response:
    deps = request.app.state.webui_deps
    user = request.state.current_user
    manager = deps.get_manager(user.token)
    graph = manager.read_graph()

    entities = graph["entities"]
    relations = graph["relations"]
    name_to_entity = {e["name"]: e for e in entities}

    center = request.query_params.get("center")
    is_full = request.query_params.get("all") == "1"

    if center:
        if center not in name_to_entity:
            raise HTTPException(status_code=404)
        depth_raw = request.query_params.get("depth", "2")
        try:
            depth = max(1, min(int(depth_raw), 10))
        except ValueError:
            depth = 2
        included = {center}
        frontier = {center}
        for _ in range(depth):
            next_frontier = set()
            for r in relations:
                if r["from"] in frontier and r["to"] not in included:
                    next_frontier.add(r["to"])
                if r["to"] in frontier and r["from"] not in included:
                    next_frontier.add(r["from"])
            included |= next_frontier
            frontier = next_frontier
            if not frontier:
                break
        nodes_in = [name_to_entity[n] for n in included if n in name_to_entity]
        edges_in = [
            r for r in relations if r["from"] in included and r["to"] in included
        ]
    elif is_full:
        nodes_in = entities
        edges_in = relations
    else:
        nodes_in = entities
        edges_in = relations

    return JSONResponse(
        {
            "nodes": [
                {
                    "data": {
                        "id": e["name"],
                        "type": e.get("entityType", ""),
                    }
                }
                for e in nodes_in
            ],
            "edges": [
                {
                    "data": {
                        "id": f"{r['from']}|{r['relationType']}|{r['to']}",
                        "source": r["from"],
                        "target": r["to"],
                        "label": r["relationType"],
                    }
                }
                for r in edges_in
            ],
            "count": len(nodes_in),
        }
    )


@login_required_api
async def update_observation(request: Request) -> Response:
    deps = request.app.state.webui_deps
    user = request.state.current_user
    _require_write(user)
    payload = await request.json()
    entity = payload["entity"]
    original = payload["original_text"]
    new_text = payload["new_text"]

    manager = deps.get_manager(user.token)
    ok = manager.update_observation(entity, original, new_text)
    if not ok:
        raise HTTPException(status_code=404, detail="Entity or observation not found.")

    html = deps.templates.get_template("observation_row.html").render(
        obs=new_text,
        entity={"name": entity},
        read_only=False,
    )
    return HTMLResponse(html)


@login_required_api
async def delete_observation(request: Request) -> Response:
    deps = request.app.state.webui_deps
    user = request.state.current_user
    _require_write(user)
    payload = await request.json()
    entity = payload["entity"]
    text = payload["text"]

    manager = deps.get_manager(user.token)
    manager.delete_observations([{"entityName": entity, "observations": [text]}])
    # htmx swaps the row out with the empty response.
    return HTMLResponse("")


@login_required_api
async def delete_entity(request: Request) -> Response:
    deps = request.app.state.webui_deps
    user = request.state.current_user
    _require_write(user)
    name = request.path_params["name"]

    manager = deps.get_manager(user.token)
    graph = manager.read_graph()
    if not any(e["name"] == name for e in graph["entities"]):
        raise HTTPException(status_code=404, detail="Entity not found.")
    manager.delete_entities([name])

    response = JSONResponse({"deleted": name})
    response.headers["HX-Redirect"] = "/ui/"
    return response
