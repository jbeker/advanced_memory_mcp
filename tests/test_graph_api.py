import httpx


async def test_graph_api_full_returns_all_nodes_and_edges(rw_client: httpx.AsyncClient):
    response = await rw_client.get("/ui/api/graph?all=1")
    assert response.status_code == 200
    payload = response.json()
    names = {n["data"]["id"] for n in payload["nodes"]}
    assert names == {"Alice", "Bob", "Acme Corp"}
    assert payload["count"] == 3
    edge_keys = {
        (e["data"]["source"], e["data"]["target"], e["data"]["label"])
        for e in payload["edges"]
    }
    assert edge_keys == {
        ("Alice", "Bob", "knows"),
        ("Alice", "Acme Corp", "works_at"),
    }


async def test_graph_api_neighborhood_depth_1(rw_client: httpx.AsyncClient):
    response = await rw_client.get("/ui/api/graph?center=Alice&depth=1")
    assert response.status_code == 200
    payload = response.json()
    names = {n["data"]["id"] for n in payload["nodes"]}
    # Alice + her direct neighbors only.
    assert names == {"Alice", "Bob", "Acme Corp"}


async def test_graph_api_neighborhood_excludes_unconnected(rw_client: httpx.AsyncClient, populated_data_dir):
    """Add an unconnected entity; depth=1 from Alice should not include it."""
    import json
    line = {
        "type": "entity",
        "name": "Loner",
        "entityType": "Person",
        "observations": [],
        "createdAt": "2026-01-01T00:00:00+00:00",
        "lastUpdated": "2026-01-01T00:00:00+00:00",
    }
    with (populated_data_dir / "alice.jsonl").open("a") as f:
        f.write(json.dumps(line) + "\n")

    response = await rw_client.get("/ui/api/graph?center=Alice&depth=1")
    payload = response.json()
    names = {n["data"]["id"] for n in payload["nodes"]}
    assert "Loner" not in names


async def test_graph_api_neighborhood_404_for_unknown_center(rw_client: httpx.AsyncClient):
    response = await rw_client.get("/ui/api/graph?center=Nobody&depth=1")
    assert response.status_code == 404


async def test_graph_page_renders(rw_client: httpx.AsyncClient):
    response = await rw_client.get("/ui/graph?center=Alice&depth=2")
    assert response.status_code == 200
    body = response.text
    # The page is a shell that loads cytoscape.js.
    assert "cytoscape" in body.lower()
    assert "/ui/api/graph" in body


async def test_graph_full_page_warns_when_large(rw_client: httpx.AsyncClient, populated_data_dir):
    """When entity count exceeds the threshold, the page renders a warning
    instead of immediately drawing the graph."""
    import json
    with (populated_data_dir / "alice.jsonl").open("a") as f:
        for i in range(600):  # tip over the 500 threshold
            f.write(json.dumps({
                "type": "entity",
                "name": f"E{i}",
                "entityType": "Bulk",
                "observations": [],
                "createdAt": "2026-01-01T00:00:00+00:00",
                "lastUpdated": "2026-01-01T00:00:00+00:00",
            }) + "\n")

    response = await rw_client.get("/ui/graph?all=1")
    assert response.status_code == 200
    body = response.text.lower()
    assert "warning" in body or "render anyway" in body or "large" in body
