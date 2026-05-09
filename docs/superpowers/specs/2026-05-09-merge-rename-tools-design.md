# Merge and Rename Tools — Design Spec

**Date:** 2026-05-09
**Status:** Approved, ready for implementation plan

## Overview

Add two MCP tools to the Advanced Memory MCP server: `rename_entity` and `merge_entities`. These cover two graph-maintenance use cases the existing tool surface does not handle cleanly:

- **Rename** — change an entity's name and update every relation that references it (e.g. `"Project X"` → `"Atlas"`).
- **Merge** — fold one entity into another, unioning observations and re-pointing relations (e.g. `"alice@corp"` and `"Alice"` are the same person).

Both are write operations and require a read-write token.

## Tool semantics

### `rename_entity(name, new_name)`

Renames an entity in place. Updates the entity's `name` field and rewrites every relation whose `from` or `to` references the old name.

**Behavior:**

1. Validate inputs:
   - Both arguments must be non-empty strings.
   - `name != new_name`, else raise `ValueError("rename is a no-op")`.
2. Look up the entity by `name`. If missing, raise `ValueError("Entity '<name>' not found")`.
3. If an entity named `new_name` already exists (and is a different entity), raise `ValueError("Entity '<new_name>' already exists; use merge_entities to combine them")`. **Rename never auto-merges** — collisions are surfaced so the caller chooses explicitly.
4. Set `entity.name = new_name`. Bump the entity's `lastUpdated` to now.
5. For every relation in the graph: if `from == name`, set `from = new_name`; if `to == name`, set `to = new_name`. Bump `lastUpdated` only on relations that were actually touched.
6. Save the graph.

**Return shape:**

```json
{
  "renamed": {"from": "<name>", "to": "<new_name>"},
  "relationsUpdated": <int>
}
```

### `merge_entities(source, target)`

Merges `source` into `target`. The source entity is removed; the target absorbs the source's observations and relations.

**Behavior:**

1. Validate inputs:
   - `source != target`, else raise `ValueError("cannot merge an entity with itself")`.
2. Look up both entities. If either is missing, raise `ValueError("Entity '<name>' not found")`.
3. Build the merged target entity:
   - **`name`** — target's name (unchanged).
   - **`entityType`** — target's type wins. If `source.entityType != target.entityType`, record the source's type as `discardedType` in the response so the caller can react.
   - **`observations`** — union, preserving target's order: keep target's list as-is, then append any source observations not already present (deduped).
   - **`createdAt`** — earliest non-null of source's and target's `createdAt`. If both are null (legacy data), result is null.
   - **`lastUpdated`** — now.
4. Re-point relations: for every relation, if `from == source`, rewrite to `target`; if `to == source`, rewrite to `target`.
5. Drop self-relations (`from == to`) that result from re-pointing.
6. Dedupe surviving relations by `(from, to, relationType)`. When duplicates collapse, keep the earliest non-null `createdAt`.
7. Remove the source entity from the graph.
8. Save the graph.

**Return shape:**

```json
{
  "merged": {"source": "<source>", "target": "<target>"},
  "observationsAdded": <int>,
  "relationsRePointed": <int>,
  "relationsDropped": <int>,
  "discardedType": "<source-type>" | null
}
```

`relationsRePointed` counts relations whose endpoints were rewritten (before the drop+dedupe pass). `relationsDropped` counts the relations removed by the self-loop drop and the dedupe pass combined.

## Implementation surface

### `knowledge_graph.py` — `KnowledgeGraphManager`

Add two methods:

```python
def rename_entity(self, name: str, new_name: str) -> dict: ...
def merge_entities(self, source: str, target: str) -> dict: ...
```

Each method does one `load_graph` → mutate in memory → `save_graph` cycle, mirroring the existing methods.

### `server.py` — MCP tool registration

Add two tools inside `register_tools(mcp)`:

```python
@mcp.tool()
def rename_entity(name: str, new_name: str, ctx: Context) -> dict: ...

@mcp.tool()
def merge_entities(source: str, target: str, ctx: Context) -> dict: ...
```

Each follows the existing pattern exactly:

1. `token = _get_token(ctx)`
2. `_check_write_permission(token)`
3. `manager = get_graph_manager(token)`
4. `return manager.rename_entity(...)` (or `merge_entities`)

Docstrings should describe the key semantic decisions an LLM caller needs to know:

- Rename errors on target collision; use `merge_entities` to combine.
- Merge keeps target's `entityType` when they differ, and reports the discarded type.
- Both error on missing entities.

## Error model

All input-validation and not-found conditions raise `ValueError`, surfaced to the MCP client as a tool error. This matches the existing `add_observations` precedent. Read-only tokens are rejected by `_check_write_permission` with the existing error message.

## Tests

New tests under `tests/`, following the existing test style.

**`rename_entity`:**
- Happy path: entity name changes; entity's `lastUpdated` bumped.
- Missing source name → `ValueError`.
- Target name already exists → `ValueError` mentioning `merge_entities`.
- Self-rename (`name == new_name`) → `ValueError`.
- Empty-string inputs → `ValueError`.
- Relations: both `from` and `to` references updated.
- Touched relations get `lastUpdated` bumped; untouched relations do not.

**`merge_entities`:**
- Happy path: source removed, target absorbs observations and relations.
- Missing source or target → `ValueError`.
- Self-merge (`source == target`) → `ValueError`.
- Observations dedupe: target's order preserved; only new ones appended.
- Relations re-pointed on both `from` and `to` sides.
- Self-loops created by re-pointing are dropped.
- Duplicate relations collapsed; earliest non-null `createdAt` kept on the survivor.
- `entityType` mismatch returns `discardedType` in the response.
- `createdAt` = earliest non-null; both-null case returns null.
- Counts in response (`observationsAdded`, `relationsRePointed`, `relationsDropped`) are accurate.

## Documentation

Update the README's "Write Operations" table to list both new tools with one-line descriptions.

## Out of scope

- Batch versions of either tool. Callers can loop in the rare bulk case.
- Web UI surfacing for rename/merge. The existing UI has edit-observation; surfacing rename/merge can come later if needed.
- Any change to the token model, read-only semantics, or read tools.
- A generic `transform_graph(operations)` API. We do not have concrete need for additional operations beyond rename/merge today.
