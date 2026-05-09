import json
import os
from datetime import datetime, timezone
from pathlib import Path


def _now_iso() -> str:
    return datetime.now(timezone.utc).isoformat()


def normalize_entity_type(entity_type: str, aliases: dict[str, str]) -> str:
    """Normalize an entity type using aliases, falling back to Title Case."""
    if not entity_type:
        return entity_type
    canonical = aliases.get(entity_type.lower())
    if canonical:
        return canonical
    return entity_type.title()


class KnowledgeGraphManager:
    def __init__(self, file_path: str, type_aliases: dict[str, str] | None = None):
        self.file_path = Path(file_path)
        self._type_aliases = type_aliases or {}

    def load_graph(self) -> dict:
        if not self.file_path.exists() or self.file_path.stat().st_size == 0:
            return {"entities": [], "relations": []}

        entities = []
        relations = []
        with open(self.file_path, "r") as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                item = json.loads(line)
                item_type = item.pop("type", None)
                if item_type == "entity":
                    item.setdefault("createdAt", None)
                    item.setdefault("lastUpdated", None)
                    entities.append(item)
                elif item_type == "relation":
                    item.setdefault("createdAt", None)
                    item.setdefault("lastUpdated", None)
                    relations.append(item)

        return {"entities": entities, "relations": relations}

    def save_graph(self, graph: dict) -> None:
        self.file_path.parent.mkdir(parents=True, exist_ok=True)
        with open(self.file_path, "w") as f:
            for entity in graph["entities"]:
                line = {"type": "entity", **entity}
                f.write(json.dumps(line) + "\n")
            for relation in graph["relations"]:
                line = {"type": "relation", **relation}
                f.write(json.dumps(line) + "\n")

    def create_entities(self, entities: list[dict]) -> list[dict]:
        graph = self.load_graph()
        existing_names = {e["name"] for e in graph["entities"]}
        now = _now_iso()
        new_entities = [e for e in entities if e["name"] not in existing_names]
        for entity in new_entities:
            entity["entityType"] = normalize_entity_type(
                entity.get("entityType", ""), self._type_aliases
            )
            entity["createdAt"] = now
            entity["lastUpdated"] = now
        graph["entities"].extend(new_entities)
        self.save_graph(graph)
        return new_entities

    def create_relations(self, relations: list[dict]) -> list[dict]:
        graph = self.load_graph()
        existing = {
            (r["from"], r["to"], r["relationType"]) for r in graph["relations"]
        }
        now = _now_iso()
        new_relations = [
            r
            for r in relations
            if (r["from"], r["to"], r["relationType"]) not in existing
        ]
        for relation in new_relations:
            relation["createdAt"] = now
            relation["lastUpdated"] = now
        graph["relations"].extend(new_relations)
        self.save_graph(graph)
        return new_relations

    def add_observations(
        self, observations: list[dict]
    ) -> list[dict]:
        graph = self.load_graph()
        entity_map = {e["name"]: e for e in graph["entities"]}

        results = []
        for obs in observations:
            entity_name = obs["entityName"]
            if entity_name not in entity_map:
                raise ValueError(f"Entity '{entity_name}' not found")

            entity = entity_map[entity_name]
            existing = set(entity.get("observations", []))
            new_obs = [o for o in obs["contents"] if o not in existing]
            entity.setdefault("observations", []).extend(new_obs)
            if new_obs:
                entity["lastUpdated"] = _now_iso()
            results.append({"entityName": entity_name, "addedObservations": new_obs})

        self.save_graph(graph)
        return results

    def delete_entities(self, entity_names: list[str]) -> None:
        graph = self.load_graph()
        names_set = set(entity_names)
        graph["entities"] = [
            e for e in graph["entities"] if e["name"] not in names_set
        ]
        graph["relations"] = [
            r
            for r in graph["relations"]
            if r["from"] not in names_set and r["to"] not in names_set
        ]
        self.save_graph(graph)

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

    def merge_entities(self, source: str, target: str) -> dict:
        """Merge source entity into target, removing source.

        Target keeps its name and entityType. Observations are unioned (target
        order preserved). Relations involving source are re-pointed to target;
        resulting self-loops are dropped, and surviving duplicates collapse on
        (from, to, relationType), keeping the earliest non-null createdAt.
        Errors on self-merge or missing entities.
        """
        if not source or not target:
            raise ValueError("source and target must both be non-empty")
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

    def update_observation(
        self, entity_name: str, original_text: str, new_text: str
    ) -> bool:
        """Replace the first observation matching original_text in place.

        Preserves observation order and updates lastUpdated. Returns True on a
        successful replace, False if the entity or original text isn't found.
        Used by the web UI's edit-observation flow.
        """
        graph = self.load_graph()
        for entity in graph["entities"]:
            if entity["name"] != entity_name:
                continue
            obs_list = entity.get("observations", [])
            for i, existing in enumerate(obs_list):
                if existing == original_text:
                    obs_list[i] = new_text
                    entity["observations"] = obs_list
                    entity["lastUpdated"] = _now_iso()
                    self.save_graph(graph)
                    return True
            return False
        return False

    def delete_observations(self, deletions: list[dict]) -> None:
        graph = self.load_graph()
        entity_map = {e["name"]: e for e in graph["entities"]}

        for deletion in deletions:
            entity_name = deletion["entityName"]
            if entity_name not in entity_map:
                continue
            entity = entity_map[entity_name]
            to_remove = set(deletion["observations"])
            before_count = len(entity.get("observations", []))
            entity["observations"] = [
                o for o in entity.get("observations", []) if o not in to_remove
            ]
            if len(entity["observations"]) < before_count:
                entity["lastUpdated"] = _now_iso()

        self.save_graph(graph)

    def delete_relations(self, relations: list[dict]) -> None:
        graph = self.load_graph()
        to_delete = {
            (r["from"], r["to"], r["relationType"]) for r in relations
        }
        graph["relations"] = [
            r
            for r in graph["relations"]
            if (r["from"], r["to"], r["relationType"]) not in to_delete
        ]
        self.save_graph(graph)

    def read_graph(self) -> dict:
        return self.load_graph()

    def search_nodes(self, query: str) -> dict:
        graph = self.load_graph()
        query_lower = query.lower()

        matching_entities = []
        for entity in graph["entities"]:
            if query_lower in entity["name"].lower():
                matching_entities.append(entity)
                continue
            if query_lower in entity.get("entityType", "").lower():
                matching_entities.append(entity)
                continue
            if any(
                query_lower in obs.lower()
                for obs in entity.get("observations", [])
            ):
                matching_entities.append(entity)

        matching_names = {e["name"] for e in matching_entities}
        matching_relations = [
            r
            for r in graph["relations"]
            if r["from"] in matching_names or r["to"] in matching_names
        ]

        return {"entities": matching_entities, "relations": matching_relations}

    def normalize_all_entity_types(self) -> dict:
        graph = self.load_graph()
        changes = []
        now = _now_iso()
        for entity in graph["entities"]:
            old_type = entity.get("entityType", "")
            new_type = normalize_entity_type(old_type, self._type_aliases)
            if old_type != new_type:
                changes.append({"name": entity["name"], "oldType": old_type, "newType": new_type})
                entity["entityType"] = new_type
                entity["lastUpdated"] = now
        if changes:
            self.save_graph(graph)
        return {"changes": changes, "total": len(changes)}

    def open_nodes(self, names: list[str]) -> dict:
        graph = self.load_graph()
        names_set = set(names)

        matching_entities = [
            e for e in graph["entities"] if e["name"] in names_set
        ]
        matching_relations = [
            r
            for r in graph["relations"]
            if r["from"] in names_set or r["to"] in names_set
        ]

        return {"entities": matching_entities, "relations": matching_relations}
