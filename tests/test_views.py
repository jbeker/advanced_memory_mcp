import httpx


async def test_entity_list_renders_all_entities(rw_client: httpx.AsyncClient):
    response = await rw_client.get("/ui/")
    assert response.status_code == 200
    body = response.text
    assert "Alice" in body
    assert "Bob" in body
    assert "Acme Corp" in body


async def test_entity_list_shows_type_and_observation_count(rw_client: httpx.AsyncClient):
    response = await rw_client.get("/ui/")
    body = response.text
    # Alice has 2 observations and is a Person
    assert "Person" in body
    assert "Company" in body
    # Observation count for Alice = 2
    assert "2" in body


async def test_entity_list_links_to_detail_page(rw_client: httpx.AsyncClient):
    response = await rw_client.get("/ui/")
    body = response.text
    assert "/ui/entity/Alice" in body
    assert "/ui/entity/Bob" in body
    assert "/ui/entity/Acme%20Corp" in body or "/ui/entity/Acme Corp" in body


async def test_search_filters_by_name(rw_client: httpx.AsyncClient):
    response = await rw_client.get("/ui/search?q=Alice")
    assert response.status_code == 200
    body = response.text
    assert "Alice" in body
    assert "Bob" not in body
    assert "Acme Corp" not in body


async def test_search_filters_by_observation_text(rw_client: httpx.AsyncClient):
    response = await rw_client.get("/ui/search?q=guitar")
    body = response.text
    assert "Bob" in body
    assert "Alice" not in body


async def test_search_filters_by_type(rw_client: httpx.AsyncClient):
    response = await rw_client.get("/ui/search?type=Person")
    body = response.text
    assert "Alice" in body
    assert "Bob" in body
    assert "Acme Corp" not in body


async def test_search_combines_query_and_type(rw_client: httpx.AsyncClient):
    response = await rw_client.get("/ui/search?q=Alice&type=Person")
    body = response.text
    assert "Alice" in body
    assert "Bob" not in body
    assert "Acme Corp" not in body


async def test_search_empty_returns_all(rw_client: httpx.AsyncClient):
    response = await rw_client.get("/ui/search")
    body = response.text
    assert "Alice" in body
    assert "Bob" in body
    assert "Acme Corp" in body


async def test_search_returns_fragment_not_full_page(rw_client: httpx.AsyncClient):
    """Search responses are htmx-swappable fragments, not full HTML pages."""
    response = await rw_client.get("/ui/search?q=Alice")
    assert response.status_code == 200
    body = response.text.lstrip()
    assert not body.startswith("<!DOCTYPE")
    assert "<html" not in body


async def test_entity_list_exposes_distinct_types_in_filter(rw_client: httpx.AsyncClient):
    """The full page should expose Person and Company as filter options."""
    response = await rw_client.get("/ui/")
    body = response.text
    # Filter <select> should contain options for the distinct entity types.
    assert 'value="Person"' in body
    assert 'value="Company"' in body
