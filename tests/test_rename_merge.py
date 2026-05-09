"""Direct unit tests for KnowledgeGraphManager.rename_entity and merge_entities.

These methods back the rename_entity and merge_entities MCP tools. The MCP
tool wrappers are thin (token check + permission check + manager call), so
the behavioural coverage lives here.
"""

import json
from pathlib import Path

import pytest

from knowledge_graph import KnowledgeGraphManager


def _write_jsonl(path: Path, items: list[dict]) -> None:
    with path.open("w") as f:
        for item in items:
            f.write(json.dumps(item) + "\n")


@pytest.fixture
def manager(tmp_path: Path) -> KnowledgeGraphManager:
    """Three entities, three relations. Used by both rename and merge tests."""
    file_path = tmp_path / "kg.jsonl"
    items = [
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
        {
            "type": "relation",
            "from": "Bob",
            "to": "Alice",
            "relationType": "knows",
            "createdAt": "2026-01-01T00:00:00+00:00",
            "lastUpdated": "2026-01-01T00:00:00+00:00",
        },
    ]
    _write_jsonl(file_path, items)
    return KnowledgeGraphManager(str(file_path))


# ---- rename_entity ----


def test_rename_entity_changes_name_and_bumps_lastUpdated(manager: KnowledgeGraphManager):
    before = next(e for e in manager.read_graph()["entities"] if e["name"] == "Alice")
    before_updated = before["lastUpdated"]

    result = manager.rename_entity("Alice", "Alice Smith")

    assert result["renamed"] == {"from": "Alice", "to": "Alice Smith"}
    graph = manager.read_graph()
    names = {e["name"] for e in graph["entities"]}
    assert "Alice Smith" in names
    assert "Alice" not in names
    renamed = next(e for e in graph["entities"] if e["name"] == "Alice Smith")
    assert renamed["lastUpdated"] != before_updated
    # createdAt is preserved.
    assert renamed["createdAt"] == before["createdAt"]


def test_rename_entity_rewrites_relations_on_both_sides(manager: KnowledgeGraphManager):
    result = manager.rename_entity("Alice", "Alice Smith")
    graph = manager.read_graph()

    # All three relations involved Alice (two as `from`, one as `to`).
    assert result["relationsUpdated"] == 3
    for r in graph["relations"]:
        assert r["from"] != "Alice"
        assert r["to"] != "Alice"

    # Specific relations rewritten correctly.
    keys = {(r["from"], r["to"], r["relationType"]) for r in graph["relations"]}
    assert ("Alice Smith", "Bob", "knows") in keys
    assert ("Alice Smith", "Acme Corp", "works_at") in keys
    assert ("Bob", "Alice Smith", "knows") in keys


def test_rename_entity_only_touches_affected_relations_lastUpdated(
    manager: KnowledgeGraphManager,
):
    # Add a relation that doesn't involve Alice.
    graph = manager.read_graph()
    graph["relations"].append(
        {
            "from": "Bob",
            "to": "Acme Corp",
            "relationType": "works_at",
            "createdAt": "2026-01-01T00:00:00+00:00",
            "lastUpdated": "2026-01-01T00:00:00+00:00",
        }
    )
    manager.save_graph(graph)

    manager.rename_entity("Alice", "Alice Smith")

    after = manager.read_graph()
    untouched = next(
        r for r in after["relations"]
        if r["from"] == "Bob" and r["to"] == "Acme Corp" and r["relationType"] == "works_at"
    )
    # Untouched relation keeps its original lastUpdated.
    assert untouched["lastUpdated"] == "2026-01-01T00:00:00+00:00"

    touched = next(
        r for r in after["relations"]
        if r["from"] == "Alice Smith" and r["to"] == "Bob"
    )
    assert touched["lastUpdated"] != "2026-01-01T00:00:00+00:00"


def test_rename_entity_errors_when_source_missing(manager: KnowledgeGraphManager):
    with pytest.raises(ValueError, match="not found"):
        manager.rename_entity("Nobody", "Somebody")


def test_rename_entity_errors_when_target_already_exists(manager: KnowledgeGraphManager):
    with pytest.raises(ValueError, match="merge_entities"):
        manager.rename_entity("Alice", "Bob")


def test_rename_entity_errors_on_self_rename(manager: KnowledgeGraphManager):
    with pytest.raises(ValueError, match="no-op"):
        manager.rename_entity("Alice", "Alice")


@pytest.mark.parametrize("name,new_name", [("", "Bob"), ("Alice", "")])
def test_rename_entity_errors_on_empty_string_inputs(
    manager: KnowledgeGraphManager, name: str, new_name: str
):
    with pytest.raises(ValueError):
        manager.rename_entity(name, new_name)


def test_rename_entity_does_not_write_on_validation_failure(
    manager: KnowledgeGraphManager,
):
    mtime_before = manager.file_path.stat().st_mtime_ns
    with pytest.raises(ValueError):
        manager.rename_entity("Nobody", "Somebody")
    assert manager.file_path.stat().st_mtime_ns == mtime_before
