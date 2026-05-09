# Merge and Rename Tools Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `rename_entity` and `merge_entities` MCP tools to the Advanced Memory MCP server so callers can rename entities (rewriting all referencing relations) and merge duplicate entities (unioning observations, re-pointing relations).

**Architecture:** Two new methods on `KnowledgeGraphManager` (in `knowledge_graph.py`), each doing one `load_graph` → mutate → `save_graph` cycle. Two thin MCP-tool wrappers in `server.py` follow the existing pattern: token check → write-permission check → manager call. Direct unit tests of the manager methods live in a new test file (matches the `test_knowledge_graph.py` style).

**Tech Stack:** Python 3.11+, FastMCP, pytest, `uv` for dependency management.

**Spec:** `docs/superpowers/specs/2026-05-09-merge-rename-tools-design.md`

---

## File Structure

| File | Action | Responsibility |
|---|---|---|
| `knowledge_graph.py` | Modify | Add `rename_entity` and `merge_entities` methods to `KnowledgeGraphManager` |
| `tests/test_rename_merge.py` | Create | Direct unit tests for the two new methods |
| `server.py` | Modify | Register `rename_entity` and `merge_entities` MCP tools inside `register_tools(mcp)` |
| `README.md` | Modify | Add the two new tools to the "Write Operations" table |

---

## Task 1: Implement `rename_entity` on `KnowledgeGraphManager`

**Files:**
- Create: `tests/test_rename_merge.py`
- Modify: `knowledge_graph.py` (add method + import shared helper if needed)

- [ ] **Step 1: Write the failing tests**

Create `tests/test_rename_merge.py` with the following content:

```python
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


def test_rename_entity_errors_on_empty_strings(manager: KnowledgeGraphManager):
    with pytest.raises(ValueError):
        manager.rename_entity("", "Bob")
    with pytest.raises(ValueError):
        manager.rename_entity("Alice", "")
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `uv run pytest tests/test_rename_merge.py -v`

Expected: All `rename_entity` tests fail with `AttributeError: 'KnowledgeGraphManager' object has no attribute 'rename_entity'`.

- [ ] **Step 3: Implement `rename_entity`**

Open `knowledge_graph.py`. Add the following method to the `KnowledgeGraphManager` class. Place it after `delete_entities` (around line 128, before `update_observation`):

```python
    def rename_entity(self, name: str, new_name: str) -> dict:
        """Rename an entity and rewrite every relation that references it.

        Errors if the source is missing or the target name already exists.
        Use merge_entities to combine two existing entities.
        """
        if not name or not new_name:
            raise ValueError("name and new_name must both be non-empty")
        if name == new_name:
            raise ValueError("rename is a no-op")

        graph = self.load_graph()
        entity_map = {e["name"]: e for e in graph["entities"]}

        if name not in entity_map:
            raise ValueError(f"Entity '{name}' not found")
        if new_name in entity_map:
            raise ValueError(
                f"Entity '{new_name}' already exists; use merge_entities to combine them"
            )

        now = _now_iso()
        entity = entity_map[name]
        entity["name"] = new_name
        entity["lastUpdated"] = now

        relations_updated = 0
        for r in graph["relations"]:
            touched = False
            if r["from"] == name:
                r["from"] = new_name
                touched = True
            if r["to"] == name:
                r["to"] = new_name
                touched = True
            if touched:
                r["lastUpdated"] = now
                relations_updated += 1

        self.save_graph(graph)
        return {
            "renamed": {"from": name, "to": new_name},
            "relationsUpdated": relations_updated,
        }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `uv run pytest tests/test_rename_merge.py -v -k rename`

Expected: All seven `rename_entity` tests pass.

- [ ] **Step 5: Commit**

```bash
git add knowledge_graph.py tests/test_rename_merge.py
git commit -m "$(cat <<'EOF'
Add rename_entity to KnowledgeGraphManager

Renames an entity in place and rewrites every relation that references the
old name. Errors on missing source, target collision, self-rename, and
empty-string inputs - rename never auto-merges.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Implement `merge_entities` on `KnowledgeGraphManager`

**Files:**
- Modify: `tests/test_rename_merge.py` (append merge tests)
- Modify: `knowledge_graph.py` (add `merge_entities` method)

- [ ] **Step 1: Write the failing tests**

Append the following block to `tests/test_rename_merge.py` (after the existing rename tests):

```python


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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `uv run pytest tests/test_rename_merge.py -v -k merge`

Expected: All twelve `merge_entities` tests fail with `AttributeError: 'KnowledgeGraphManager' object has no attribute 'merge_entities'`.

- [ ] **Step 3: Implement `merge_entities`**

Open `knowledge_graph.py`. Add the following method to the `KnowledgeGraphManager` class. Place it directly after `rename_entity` from Task 1:

```python
    def merge_entities(self, source: str, target: str) -> dict:
        """Merge source entity into target, removing source.

        Target keeps its name and entityType. Observations are unioned (target
        order preserved). Relations involving source are re-pointed to target;
        resulting self-loops are dropped, and surviving duplicates collapse on
        (from, to, relationType), keeping the earliest non-null createdAt.
        Errors on self-merge or missing entities.
        """
        if source == target:
            raise ValueError("cannot merge an entity with itself")

        graph = self.load_graph()
        entity_map = {e["name"]: e for e in graph["entities"]}

        if source not in entity_map:
            raise ValueError(f"Entity '{source}' not found")
        if target not in entity_map:
            raise ValueError(f"Entity '{target}' not found")

        src = entity_map[source]
        tgt = entity_map[target]
        now = _now_iso()

        # Observations: union, target order preserved.
        existing_obs = set(tgt.get("observations", []))
        new_obs = [o for o in src.get("observations", []) if o not in existing_obs]
        tgt.setdefault("observations", []).extend(new_obs)
        observations_added = len(new_obs)

        # entityType: target wins; report source's if it differs.
        discarded_type = None
        src_type = src.get("entityType")
        tgt_type = tgt.get("entityType")
        if src_type and src_type != tgt_type:
            discarded_type = src_type

        # createdAt: earliest non-null. Both null -> stay null.
        src_created = src.get("createdAt")
        tgt_created = tgt.get("createdAt")
        if src_created and tgt_created:
            tgt["createdAt"] = min(src_created, tgt_created)
        elif src_created and not tgt_created:
            tgt["createdAt"] = src_created
        # else: keep tgt_created (possibly null).

        tgt["lastUpdated"] = now

        # Re-point every relation involving source.
        relations_re_pointed = 0
        for r in graph["relations"]:
            touched = False
            if r["from"] == source:
                r["from"] = target
                touched = True
            if r["to"] == source:
                r["to"] = target
                touched = True
            if touched:
                relations_re_pointed += 1

        # Drop self-loops created by re-pointing.
        before_loops = len(graph["relations"])
        graph["relations"] = [r for r in graph["relations"] if r["from"] != r["to"]]
        self_dropped = before_loops - len(graph["relations"])

        # Dedupe by (from, to, relationType); keep earliest non-null createdAt.
        seen: dict[tuple, dict] = {}
        for r in graph["relations"]:
            key = (r["from"], r["to"], r["relationType"])
            if key not in seen:
                seen[key] = r
                continue
            existing = seen[key]
            ec = existing.get("createdAt")
            rc = r.get("createdAt")
            # Replace if r's createdAt is non-null and earlier (or existing is null).
            if rc and (not ec or rc < ec):
                seen[key] = r
        deduped = list(seen.values())
        dup_dropped = len(graph["relations"]) - len(deduped)
        graph["relations"] = deduped

        relations_dropped = self_dropped + dup_dropped

        # Remove the source entity.
        graph["entities"] = [e for e in graph["entities"] if e["name"] != source]

        self.save_graph(graph)
        return {
            "merged": {"source": source, "target": target},
            "observationsAdded": observations_added,
            "relationsRePointed": relations_re_pointed,
            "relationsDropped": relations_dropped,
            "discardedType": discarded_type,
        }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `uv run pytest tests/test_rename_merge.py -v`

Expected: All nineteen tests pass (seven from Task 1 + twelve from Task 2).

- [ ] **Step 5: Run the full test suite to confirm no regressions**

Run: `uv run pytest -v`

Expected: All tests pass — including pre-existing tests in `test_knowledge_graph.py`, `test_mutations.py`, `test_auth.py`, etc.

- [ ] **Step 6: Commit**

```bash
git add knowledge_graph.py tests/test_rename_merge.py
git commit -m "$(cat <<'EOF'
Add merge_entities to KnowledgeGraphManager

Merges source into target: target keeps its name and entityType, observations
are unioned (target order preserved), relations re-pointed and deduped
(self-loops dropped, duplicates collapsed keeping earliest createdAt). Source
entity removed.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Register `rename_entity` and `merge_entities` MCP tools

**Files:**
- Modify: `server.py` (add two `@mcp.tool()` wrappers inside `register_tools`)

- [ ] **Step 1: Add the MCP tool wrappers**

Open `server.py`. Find `register_tools(mcp)` (starts around line 65). After the existing `delete_relations` tool (which ends around line 138, just before `read_graph`), add the following two tools:

```python
    @mcp.tool()
    def rename_entity(name: str, new_name: str, ctx: Context) -> dict:
        """Rename an entity in the knowledge graph.

        Updates the entity's name and rewrites every relation that references it
        (both 'from' and 'to' endpoints). Errors if the source entity is not found,
        if an entity with 'new_name' already exists (use merge_entities to combine
        them), or if name == new_name. Bumps lastUpdated on the entity and on every
        touched relation."""
        token = _get_token(ctx)
        _check_write_permission(token)
        manager = get_graph_manager(token)
        return manager.rename_entity(name, new_name)

    @mcp.tool()
    def merge_entities(source: str, target: str, ctx: Context) -> dict:
        """Merge the source entity into the target entity.

        The source is removed; the target absorbs the source's observations
        (unioned, target order preserved) and relations (re-pointed, self-loops
        dropped, duplicates collapsed keeping the earliest createdAt). Target's
        entityType is kept; if source's entityType differed it is returned as
        'discardedType' in the response. createdAt becomes the earliest non-null
        of the two. Errors if either entity is missing or source == target."""
        token = _get_token(ctx)
        _check_write_permission(token)
        manager = get_graph_manager(token)
        return manager.merge_entities(source, target)
```

- [ ] **Step 2: Smoke-check the server still starts**

Run: `uv run python -c "import server; mcp = __import__('fastmcp').FastMCP('test'); server.register_tools(mcp)"`

Expected: Exits cleanly with no output. (Confirms the new tools register without raising.)

- [ ] **Step 3: Run the full test suite to confirm no regressions**

Run: `uv run pytest -v`

Expected: All tests pass.

- [ ] **Step 4: Commit**

```bash
git add server.py
git commit -m "$(cat <<'EOF'
Register rename_entity and merge_entities MCP tools

Thin wrappers over KnowledgeGraphManager.rename_entity and merge_entities.
Both require a read-write token; semantics are described in the docstrings
so MCP clients can pick the right tool.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Document the new tools in the README

**Files:**
- Modify: `README.md` (extend the "Write Operations" table)

- [ ] **Step 1: Update the Write Operations table**

Open `README.md`. Find the "Write Operations (read-write tokens only)" table (around line 120). Add two rows so the table reads (the `normalize_entity_types` row stays at the bottom):

```markdown
| Tool | Description |
|---|---|
| `create_entities` | Create entities with name, type, and observations. Skips duplicates. |
| `create_relations` | Create directed relations between entities (`from`, `to`, `relationType`). |
| `add_observations` | Append observations to existing entities. |
| `delete_entities` | Delete entities and cascade-delete their relations. |
| `delete_observations` | Remove specific observations from entities. |
| `delete_relations` | Remove specific relations by exact match. |
| `rename_entity` | Rename an entity and rewrite every relation that references it. Errors on target collision. |
| `merge_entities` | Merge a source entity into a target: union observations, re-point relations, drop self-loops and duplicates. |
| `normalize_entity_types` | Apply type alias mappings to all existing entities. |
```

- [ ] **Step 2: Commit**

```bash
git add README.md
git commit -m "$(cat <<'EOF'
Document rename_entity and merge_entities in README

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Done

After Task 4:

- Two new methods on `KnowledgeGraphManager`, fully unit-tested.
- Two new MCP tools exposed to clients with read-write tokens.
- README reflects the new tool surface.
- All pre-existing tests still pass.
