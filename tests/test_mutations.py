import json
from pathlib import Path

import httpx


def _read_entities(data_dir: Path) -> list[dict]:
    out = []
    with (data_dir / "alice.jsonl").open() as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            item = json.loads(line)
            if item.get("type") == "entity":
                out.append(item)
    return out


def _find_entity(data_dir: Path, name: str) -> dict | None:
    return next((e for e in _read_entities(data_dir) if e["name"] == name), None)


# ---- Edit observation ----

async def test_edit_observation_persists_and_preserves_order(
    rw_client: httpx.AsyncClient, populated_data_dir: Path
):
    response = await rw_client.patch(
        "/ui/api/observations",
        json={"entity": "Alice", "original_text": "Likes coffee", "new_text": "Likes tea"},
    )
    assert response.status_code == 200
    # Returned fragment should show the new text, not the old.
    assert "Likes tea" in response.text
    assert "Likes coffee" not in response.text

    alice = _find_entity(populated_data_dir, "Alice")
    assert alice["observations"] == ["Likes tea", "Lives in Boston"]


async def test_edit_observation_returns_404_for_unknown(
    rw_client: httpx.AsyncClient,
):
    response = await rw_client.patch(
        "/ui/api/observations",
        json={"entity": "Alice", "original_text": "no-such-thing", "new_text": "X"},
    )
    assert response.status_code == 404


async def test_edit_observation_rejected_for_read_only_token(
    ro_client: httpx.AsyncClient, populated_data_dir: Path
):
    response = await ro_client.patch(
        "/ui/api/observations",
        json={"entity": "Alice", "original_text": "Likes coffee", "new_text": "Likes tea"},
    )
    assert response.status_code == 403
    alice = _find_entity(populated_data_dir, "Alice")
    assert "Likes coffee" in alice["observations"]


# ---- Delete observation ----

async def test_delete_observation_persists(
    rw_client: httpx.AsyncClient, populated_data_dir: Path
):
    response = await rw_client.post(
        "/ui/api/observations/delete",
        json={"entity": "Alice", "text": "Likes coffee"},
    )
    assert response.status_code == 200
    alice = _find_entity(populated_data_dir, "Alice")
    assert alice["observations"] == ["Lives in Boston"]


async def test_delete_observation_rejected_for_read_only(
    ro_client: httpx.AsyncClient, populated_data_dir: Path
):
    response = await ro_client.post(
        "/ui/api/observations/delete",
        json={"entity": "Alice", "text": "Likes coffee"},
    )
    assert response.status_code == 403
    alice = _find_entity(populated_data_dir, "Alice")
    assert "Likes coffee" in alice["observations"]


# ---- Delete entity (cascades relations) ----

async def test_delete_entity_persists_and_cascades_relations(
    rw_client: httpx.AsyncClient, populated_data_dir: Path
):
    response = await rw_client.delete("/ui/api/entities/Alice")
    assert response.status_code == 200
    # Alice gone.
    assert _find_entity(populated_data_dir, "Alice") is None
    # Relations involving Alice gone.
    with (populated_data_dir / "alice.jsonl").open() as f:
        contents = f.read()
    assert "Alice" not in contents
    # Bob and Acme Corp still there.
    assert _find_entity(populated_data_dir, "Bob") is not None
    assert _find_entity(populated_data_dir, "Acme Corp") is not None


async def test_delete_entity_redirects_via_hx_redirect(rw_client: httpx.AsyncClient):
    """Deleting an entity from its detail page should tell htmx to navigate
    back to the list (no entity to display anymore)."""
    response = await rw_client.delete("/ui/api/entities/Alice")
    assert response.status_code == 200
    assert response.headers.get("hx-redirect") == "/ui/"


async def test_delete_entity_url_decodes_name(
    rw_client: httpx.AsyncClient, populated_data_dir: Path
):
    response = await rw_client.delete("/ui/api/entities/Acme%20Corp")
    assert response.status_code == 200
    assert _find_entity(populated_data_dir, "Acme Corp") is None


async def test_delete_entity_404_for_unknown(rw_client: httpx.AsyncClient):
    response = await rw_client.delete("/ui/api/entities/Nobody")
    assert response.status_code == 404


async def test_delete_entity_rejected_for_read_only(
    ro_client: httpx.AsyncClient, populated_data_dir: Path
):
    response = await ro_client.delete("/ui/api/entities/Alice")
    assert response.status_code == 403
    assert _find_entity(populated_data_dir, "Alice") is not None
