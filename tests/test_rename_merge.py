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


# ---- merge_entities ----


def test_merge_entities_unions_observations_preserving_target_order(
    manager: KnowledgeGraphManager,
):
    # Give Alice an extra observation that overlaps Bob's, plus a new one.
    graph = manager.read_graph()
    bob = next(e for e in graph["entities"] if e["name"] == "Bob")
    bob["observations"] = ["Plays guitar", "Likes coffee"]
    manager.save_graph(graph)

    result = manager.merge_entities("Bob", "Alice")

    alice = next(e for e in manager.read_graph()["entities"] if e["name"] == "Alice")
    # Target's order preserved; only the genuinely new observation appended.
    assert alice["observations"] == ["Likes coffee", "Lives in Boston", "Plays guitar"]
    assert result["observationsAdded"] == 1


def test_merge_entities_removes_source_and_keeps_target(manager: KnowledgeGraphManager):
    manager.merge_entities("Bob", "Alice")
    names = {e["name"] for e in manager.read_graph()["entities"]}
    assert "Bob" not in names
    assert "Alice" in names


def test_merge_entities_repoints_relations_on_both_sides(manager: KnowledgeGraphManager):
    # Bob is involved in two relations: Alice→Bob (knows) and Bob→Alice (knows).
    # After merge, Alice→Alice (knows) self-loops are dropped on both sides.
    result = manager.merge_entities("Bob", "Alice")

    relations = manager.read_graph()["relations"]
    # No relation should still mention Bob.
    for r in relations:
        assert r["from"] != "Bob"
        assert r["to"] != "Bob"

    # Two relations got re-pointed (both touched Bob).
    assert result["relationsRePointed"] == 2
    # Both became Alice→Alice self-loops, both dropped.
    assert result["relationsDropped"] == 2

    # Alice→Acme Corp (works_at) survives untouched.
    surviving = {(r["from"], r["to"], r["relationType"]) for r in relations}
    assert ("Alice", "Acme Corp", "works_at") in surviving


def test_merge_entities_dedupes_relations_keeping_earliest_createdAt(tmp_path: Path):
    # Two relations that collide after re-pointing, with different createdAt.
    file_path = tmp_path / "kg.jsonl"
    items = [
        {"type": "entity", "name": "Alice", "entityType": "Person", "observations": [],
         "createdAt": "2026-01-01T00:00:00+00:00", "lastUpdated": "2026-01-01T00:00:00+00:00"},
        {"type": "entity", "name": "alice@corp", "entityType": "Person", "observations": [],
         "createdAt": "2026-01-01T00:00:00+00:00", "lastUpdated": "2026-01-01T00:00:00+00:00"},
        {"type": "entity", "name": "Acme", "entityType": "Company", "observations": [],
         "createdAt": "2026-01-01T00:00:00+00:00", "lastUpdated": "2026-01-01T00:00:00+00:00"},
        # Relation already exists on Alice (newer).
        {"type": "relation", "from": "Alice", "to": "Acme", "relationType": "works_at",
         "createdAt": "2026-03-01T00:00:00+00:00", "lastUpdated": "2026-03-01T00:00:00+00:00"},
        # Same relation on alice@corp (older). After merge it collides; the older
        # createdAt should win on the survivor.
        {"type": "relation", "from": "alice@corp", "to": "Acme", "relationType": "works_at",
         "createdAt": "2026-01-15T00:00:00+00:00", "lastUpdated": "2026-01-15T00:00:00+00:00"},
    ]
    _write_jsonl(file_path, items)
    mgr = KnowledgeGraphManager(str(file_path))

    result = mgr.merge_entities("alice@corp", "Alice")

    relations = mgr.read_graph()["relations"]
    survivors = [r for r in relations if r["from"] == "Alice" and r["to"] == "Acme"]
    assert len(survivors) == 1
    assert survivors[0]["createdAt"] == "2026-01-15T00:00:00+00:00"
    # One re-pointed (alice@corp→Alice), one dropped by dedupe.
    assert result["relationsRePointed"] == 1
    assert result["relationsDropped"] == 1


def test_merge_entities_keeps_target_entityType_and_reports_discarded(
    manager: KnowledgeGraphManager,
):
    # Bob is "Person", Acme Corp is "Company". Merging Bob into Acme Corp
    # should keep "Company" and report "Person" as discarded.
    result = manager.merge_entities("Bob", "Acme Corp")

    acme = next(e for e in manager.read_graph()["entities"] if e["name"] == "Acme Corp")
    assert acme["entityType"] == "Company"
    assert result["discardedType"] == "Person"


def test_merge_entities_discardedType_null_when_types_match(
    manager: KnowledgeGraphManager,
):
    # Alice and Bob are both "Person".
    result = manager.merge_entities("Bob", "Alice")
    assert result["discardedType"] is None


def test_merge_entities_picks_earliest_non_null_createdAt(tmp_path: Path):
    file_path = tmp_path / "kg.jsonl"
    items = [
        {"type": "entity", "name": "A", "entityType": "Person", "observations": [],
         "createdAt": "2026-03-01T00:00:00+00:00", "lastUpdated": "2026-03-01T00:00:00+00:00"},
        {"type": "entity", "name": "B", "entityType": "Person", "observations": [],
         "createdAt": "2026-01-01T00:00:00+00:00", "lastUpdated": "2026-01-01T00:00:00+00:00"},
    ]
    _write_jsonl(file_path, items)
    mgr = KnowledgeGraphManager(str(file_path))

    mgr.merge_entities("B", "A")
    a = mgr.read_graph()["entities"][0]
    assert a["name"] == "A"
    assert a["createdAt"] == "2026-01-01T00:00:00+00:00"


def test_merge_entities_handles_null_createdAt_on_either_side(tmp_path: Path):
    file_path = tmp_path / "kg.jsonl"
    items = [
        # Legacy entity with no timestamps.
        {"type": "entity", "name": "Legacy", "entityType": "Person", "observations": []},
        {"type": "entity", "name": "Modern", "entityType": "Person", "observations": [],
         "createdAt": "2026-03-01T00:00:00+00:00", "lastUpdated": "2026-03-01T00:00:00+00:00"},
    ]
    _write_jsonl(file_path, items)
    mgr = KnowledgeGraphManager(str(file_path))

    # Source is legacy (null createdAt), target is modern. Target keeps its own.
    mgr.merge_entities("Legacy", "Modern")
    modern = mgr.read_graph()["entities"][0]
    assert modern["createdAt"] == "2026-03-01T00:00:00+00:00"


def test_merge_entities_uses_source_createdAt_when_target_is_null(tmp_path: Path):
    file_path = tmp_path / "kg.jsonl"
    items = [
        {"type": "entity", "name": "Source", "entityType": "Person", "observations": [],
         "createdAt": "2026-01-01T00:00:00+00:00", "lastUpdated": "2026-01-01T00:00:00+00:00"},
        # Target is legacy.
        {"type": "entity", "name": "Target", "entityType": "Person", "observations": []},
    ]
    _write_jsonl(file_path, items)
    mgr = KnowledgeGraphManager(str(file_path))

    mgr.merge_entities("Source", "Target")
    target = mgr.read_graph()["entities"][0]
    assert target["createdAt"] == "2026-01-01T00:00:00+00:00"


def test_merge_entities_errors_on_self_merge(manager: KnowledgeGraphManager):
    with pytest.raises(ValueError, match="itself"):
        manager.merge_entities("Alice", "Alice")


def test_merge_entities_errors_when_source_missing(manager: KnowledgeGraphManager):
    with pytest.raises(ValueError, match="not found"):
        manager.merge_entities("Nobody", "Alice")


def test_merge_entities_errors_when_target_missing(manager: KnowledgeGraphManager):
    with pytest.raises(ValueError, match="not found"):
        manager.merge_entities("Alice", "Nobody")
