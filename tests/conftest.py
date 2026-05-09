import json
from pathlib import Path

import httpx
import pytest

from knowledge_graph import KnowledgeGraphManager
from token_config import TokenConfig
from webui import build_starlette_app


RW_TOKEN = "alice_" + "a" * 64
RO_TOKEN = "bob_" + "b" * 64
UI_SECRET = "test-secret-not-for-production"


@pytest.fixture
def populated_data_dir(tmp_path: Path) -> Path:
    data_dir = tmp_path / "data"
    data_dir.mkdir()

    jsonl = data_dir / "alice.jsonl"
    lines = [
        {
            "type": "entity",
            "name": "Alice",
            "entityType": "Person",
            "observations": ["Likes coffee", "Lives in Boston"],
            "createdAt": "2026-01-01T00:00:00+00:00",
            "lastUpdated": "2026-01-02T00:00:00+00:00",
        },
        {
            "type": "entity",
            "name": "Bob",
            "entityType": "Person",
            "observations": ["Plays guitar"],
            "createdAt": "2026-01-01T00:00:00+00:00",
            "lastUpdated": "2026-01-01T00:00:00+00:00",
        },
        {
            "type": "entity",
            "name": "Acme Corp",
            "entityType": "Company",
            "observations": ["Founded 1999"],
            "createdAt": "2026-01-01T00:00:00+00:00",
            "lastUpdated": "2026-01-01T00:00:00+00:00",
        },
        {
            "type": "relation",
            "from": "Alice",
            "to": "Bob",
            "relationType": "knows",
            "createdAt": "2026-01-01T00:00:00+00:00",
            "lastUpdated": "2026-01-01T00:00:00+00:00",
        },
        {
            "type": "relation",
            "from": "Alice",
            "to": "Acme Corp",
            "relationType": "works_at",
            "createdAt": "2026-01-01T00:00:00+00:00",
            "lastUpdated": "2026-01-01T00:00:00+00:00",
        },
    ]
    with jsonl.open("w") as f:
        for line in lines:
            f.write(json.dumps(line) + "\n")
    return data_dir


@pytest.fixture
def token_config(tmp_path: Path) -> TokenConfig:
    config_path = tmp_path / "tokens.json"
    config_path.write_text(json.dumps({
        RW_TOKEN: {"file": "alice.jsonl", "mode": "read-write"},
        RO_TOKEN: {"file": "alice.jsonl", "mode": "read-only"},
    }))
    return TokenConfig(str(config_path))


@pytest.fixture
def get_manager(populated_data_dir: Path, token_config: TokenConfig):
    cache: dict[str, KnowledgeGraphManager] = {}

    def _get(token: str) -> KnowledgeGraphManager:
        entry = token_config.get(token)
        if entry.file not in cache:
            cache[entry.file] = KnowledgeGraphManager(str(populated_data_dir / entry.file))
        return cache[entry.file]

    return _get


@pytest.fixture
def app(token_config, get_manager):
    return build_starlette_app(
        token_config=token_config,
        get_manager=get_manager,
        ui_secret=UI_SECRET,
    )


@pytest.fixture
async def client(app):
    transport = httpx.ASGITransport(app=app)
    async with httpx.AsyncClient(transport=transport, base_url="http://test") as c:
        yield c


async def _login(client: httpx.AsyncClient, token: str) -> None:
    response = await client.post(
        "/ui/login", data={"token": token}, follow_redirects=False
    )
    assert response.status_code in (302, 303), response.text


@pytest.fixture
async def rw_client(client: httpx.AsyncClient):
    await _login(client, RW_TOKEN)
    return client


@pytest.fixture
async def ro_client(client: httpx.AsyncClient):
    await _login(client, RO_TOKEN)
    return client
