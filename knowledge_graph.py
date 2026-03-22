import json
import os
from pathlib import Path


class KnowledgeGraphManager:
    def __init__(self, file_path: str):
        self.file_path = Path(file_path)

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
                    entities.append(item)
                elif item_type == "relation":
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
        new_entities = [e for e in entities if e["name"] not in existing_names]
        graph["entities"].extend(new_entities)
        self.save_graph(graph)
        return new_entities

    def create_relations(self, relations: list[dict]) -> list[dict]:
        graph = self.load_graph()
        existing = {
            (r["from"], r["to"], r["relationType"]) for r in graph["relations"]
        }
        new_relations = [
            r
            for r in relations
            if (r["from"], r["to"], r["relationType"]) not in existing
        ]
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

    def delete_observations(self, deletions: list[dict]) -> None:
        graph = self.load_graph()
        entity_map = {e["name"]: e for e in graph["entities"]}

        for deletion in deletions:
            entity_name = deletion["entityName"]
            if entity_name not in entity_map:
                continue
            entity = entity_map[entity_name]
            to_remove = set(deletion["observations"])
            entity["observations"] = [
                o for o in entity.get("observations", []) if o not in to_remove
            ]

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
