import httpx


async def test_entity_detail_renders_known_entity(rw_client: httpx.AsyncClient):
    response = await rw_client.get("/ui/entity/Alice")
    assert response.status_code == 200
    body = response.text
    assert "Alice" in body
    assert "Person" in body


async def test_entity_detail_lists_observations(rw_client: httpx.AsyncClient):
    response = await rw_client.get("/ui/entity/Alice")
    body = response.text
    assert "Likes coffee" in body
    assert "Lives in Boston" in body


async def test_entity_detail_shows_outgoing_relations(rw_client: httpx.AsyncClient):
    """Alice -> knows -> Bob, Alice -> works_at -> Acme Corp."""
    response = await rw_client.get("/ui/entity/Alice")
    body = response.text
    assert "knows" in body
    assert "works_at" in body
    assert "Bob" in body
    assert "Acme Corp" in body


async def test_entity_detail_shows_incoming_relations(rw_client: httpx.AsyncClient):
    """Bob has Alice -> knows -> Bob as incoming."""
    response = await rw_client.get("/ui/entity/Bob")
    body = response.text
    assert "Alice" in body
    assert "knows" in body


async def test_entity_detail_404_for_unknown_entity(rw_client: httpx.AsyncClient):
    response = await rw_client.get("/ui/entity/Nobody")
    assert response.status_code == 404


async def test_entity_detail_handles_url_encoded_names(rw_client: httpx.AsyncClient):
    """Entity names can contain spaces; the route must decode them."""
    response = await rw_client.get("/ui/entity/Acme%20Corp")
    assert response.status_code == 200
    body = response.text
    assert "Acme Corp" in body
    assert "Founded 1999" in body


async def test_entity_detail_shows_timestamps(rw_client: httpx.AsyncClient):
    response = await rw_client.get("/ui/entity/Alice")
    body = response.text
    assert "2026-01-01" in body  # createdAt
    assert "2026-01-02" in body  # lastUpdated


async def test_entity_detail_disables_mutation_controls_for_read_only(
    ro_client: httpx.AsyncClient,
):
    """Read-only tokens see disabled delete/edit affordances."""
    response = await ro_client.get("/ui/entity/Alice")
    body = response.text
    # The simplest signal: render the read-only label and don't render any
    # active delete buttons (hx-delete attributes).
    assert "read-only" in body.lower()
    assert "hx-delete" not in body


async def test_entity_detail_renders_mutation_controls_for_read_write(
    rw_client: httpx.AsyncClient,
):
    response = await rw_client.get("/ui/entity/Alice")
    body = response.text
    assert "hx-delete" in body
