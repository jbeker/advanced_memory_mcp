"""Direct unit tests for KnowledgeGraphManager.update_observation.

The web UI's edit-observation endpoint depends on this method; existing MCP
tools don't, so it's tested in isolation here rather than indirectly via the
existing MCP tool tests.
"""

import json
from pathlib import Path

import pytest

from knowledge_graph import KnowledgeGraphManager


@pytest.fixture
def manager(tmp_path: Path) -> KnowledgeGraphManager:
    file_path = tmp_path / "kg.jsonl"
    lines = [
        {
            "type": "entity",
            "name": "Alice",
            "entityType": "Person",
            "observations": ["one", "two", "three"],
            "createdAt": "2026-01-01T00:00:00+00:00",
            "lastUpdated": "2026-01-01T00:00:00+00:00",
        },
    ]
    with file_path.open("w") as f:
        for line in lines:
            f.write(json.dumps(line) + "\n")
    return KnowledgeGraphManager(str(file_path))


def test_update_observation_replaces_in_place(manager: KnowledgeGraphManager):
    result = manager.update_observation("Alice", "two", "TWO")
    assert result is True
    graph = manager.read_graph()
    obs = graph["entities"][0]["observations"]
    # Order preserved, only the matched item replaced.
    assert obs == ["one", "TWO", "three"]


def test_update_observation_touches_lastUpdated(manager: KnowledgeGraphManager):
    before = manager.read_graph()["entities"][0]["lastUpdated"]
    manager.update_observation("Alice", "two", "TWO")
    after = manager.read_graph()["entities"][0]["lastUpdated"]
    assert after != before


def test_update_observation_returns_false_for_unknown_entity(manager: KnowledgeGraphManager):
    result = manager.update_observation("Nobody", "two", "TWO")
    assert result is False
    # File unchanged.
    obs = manager.read_graph()["entities"][0]["observations"]
    assert obs == ["one", "two", "three"]


def test_update_observation_returns_false_for_missing_text(manager: KnowledgeGraphManager):
    result = manager.update_observation("Alice", "no-such-text", "X")
    assert result is False
    obs = manager.read_graph()["entities"][0]["observations"]
    assert obs == ["one", "two", "three"]


def test_update_observation_only_replaces_first_match(manager: KnowledgeGraphManager):
    # Pre-populate with a duplicate.
    g = manager.read_graph()
    g["entities"][0]["observations"] = ["dup", "x", "dup"]
    manager.save_graph(g)

    manager.update_observation("Alice", "dup", "REPLACED")
    obs = manager.read_graph()["entities"][0]["observations"]
    assert obs == ["REPLACED", "x", "dup"]
