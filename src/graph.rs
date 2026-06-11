//! In-memory knowledge graph model and operations.
//!
//! Port of `knowledge_graph.py`. The Python implementation is the behavioral
//! spec: dedup rules, error messages, timestamp handling, and response shapes
//! must match it exactly. Unlike Python (which re-reads the file per call),
//! this graph lives in memory; persistence is `store.rs`'s concern.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;

/// ISO 8601 UTC timestamp matching Python's `datetime.now(timezone.utc).isoformat()`
/// output: microsecond precision with a `+00:00` suffix.
pub fn now_iso() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.6f+00:00").to_string()
}

/// Port of Python `str.title()`: uppercase any cased character that follows
/// a non-cased character, lowercase the rest. ("1password employee" ->
/// "1Password Employee", "o'neil" -> "O'Neil")
pub fn py_title(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_cased = false;
    for c in s.chars() {
        let cased = c.is_uppercase() || c.is_lowercase();
        if cased {
            if prev_cased {
                out.extend(c.to_lowercase());
            } else {
                out.extend(c.to_uppercase());
            }
        } else {
            out.push(c);
        }
        prev_cased = cased;
    }
    out
}

/// Normalize an entity type using aliases (matched on the lowercased input),
/// falling back to Title Case. Empty input is returned unchanged.
pub fn normalize_entity_type(entity_type: &str, aliases: &HashMap<String, String>) -> String {
    if entity_type.is_empty() {
        return String::new();
    }
    if let Some(canonical) = aliases.get(&entity_type.to_lowercase()) {
        return canonical.clone();
    }
    py_title(entity_type)
}

/// An entity line. Field declaration order is the on-disk key order.
/// `entityType`/`observations` are omitted when absent (legacy lines);
/// `createdAt`/`lastUpdated` are always written, `null` when unknown,
/// matching Python's `setdefault(..., None)` on load. Unknown keys
/// round-trip through `extra`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Entity {
    pub name: String,
    #[serde(rename = "entityType", default, skip_serializing_if = "Option::is_none")]
    pub entity_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observations: Option<Vec<String>>,
    #[serde(rename = "createdAt", default)]
    pub created_at: Option<String>,
    #[serde(rename = "lastUpdated", default)]
    pub last_updated: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Entity {
    pub fn entity_type_str(&self) -> &str {
        self.entity_type.as_deref().unwrap_or("")
    }

    pub fn observations_slice(&self) -> &[String] {
        self.observations.as_deref().unwrap_or(&[])
    }
}

/// A relation line. Same timestamp/extra-field semantics as `Entity`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Relation {
    pub from: String,
    pub to: String,
    #[serde(rename = "relationType")]
    pub relation_type: String,
    #[serde(rename = "createdAt", default)]
    pub created_at: Option<String>,
    #[serde(rename = "lastUpdated", default)]
    pub last_updated: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Relation {
    fn key(&self) -> (String, String, String) {
        (self.from.clone(), self.to.clone(), self.relation_type.clone())
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Graph {
    pub entities: Vec<Entity>,
    pub relations: Vec<Relation>,
}

/// Per-entity result of `add_observations`.
#[derive(Debug, Serialize)]
pub struct AddedObservations {
    #[serde(rename = "entityName")]
    pub entity_name: String,
    #[serde(rename = "addedObservations")]
    pub added_observations: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct TypeChange {
    pub name: String,
    #[serde(rename = "oldType")]
    pub old_type: String,
    #[serde(rename = "newType")]
    pub new_type: String,
}

impl Graph {
    fn entity_index(&self, name: &str) -> Option<usize> {
        self.entities.iter().position(|e| e.name == name)
    }

    /// Add entities, skipping any whose name already exists. New entities get
    /// a normalized type and timestamps. Returns the entities actually added.
    pub fn create_entities(
        &mut self,
        entities: Vec<Entity>,
        aliases: &HashMap<String, String>,
    ) -> Vec<Entity> {
        // NOTE: like Python, dedup is only against pre-existing entities —
        // duplicates within one input batch are all added.
        let existing: std::collections::HashSet<&str> =
            self.entities.iter().map(|e| e.name.as_str()).collect();
        let now = now_iso();
        let mut added = Vec::new();
        for mut entity in entities {
            if existing.contains(entity.name.as_str()) {
                continue;
            }
            entity.entity_type =
                Some(normalize_entity_type(entity.entity_type_str(), aliases));
            entity.created_at = Some(now.clone());
            entity.last_updated = Some(now.clone());
            added.push(entity);
        }
        self.entities.extend(added.iter().cloned());
        added
    }

    /// Add relations, skipping duplicates of (from, to, relationType).
    pub fn create_relations(&mut self, relations: Vec<Relation>) -> Vec<Relation> {
        // Same as create_entities: no intra-batch dedup, matching Python.
        let existing: std::collections::HashSet<(String, String, String)> =
            self.relations.iter().map(|r| r.key()).collect();
        let now = now_iso();
        let mut added: Vec<Relation> = Vec::new();
        for mut relation in relations {
            if existing.contains(&relation.key()) {
                continue;
            }
            relation.created_at = Some(now.clone());
            relation.last_updated = Some(now.clone());
            added.push(relation);
        }
        self.relations.extend(added.iter().cloned());
        added
    }

    /// Append observations to existing entities, deduplicating against the
    /// entity's current list. Errors (without mutating) if any named entity
    /// is missing — Python aborts before saving, so the net effect there is
    /// also "no change".
    pub fn add_observations(
        &mut self,
        observations: Vec<(String, Vec<String>)>,
    ) -> Result<Vec<AddedObservations>, String> {
        for (entity_name, _) in &observations {
            if self.entity_index(entity_name).is_none() {
                return Err(format!("Entity '{entity_name}' not found"));
            }
        }
        let now = now_iso();
        let mut results = Vec::new();
        for (entity_name, contents) in observations {
            let idx = self.entity_index(&entity_name).unwrap();
            let entity = &mut self.entities[idx];
            let existing: std::collections::HashSet<&String> =
                entity.observations_slice().iter().collect();
            let new_obs: Vec<String> = contents
                .iter()
                .filter(|o| !existing.contains(o))
                .cloned()
                .collect();
            entity
                .observations
                .get_or_insert_with(Vec::new)
                .extend(new_obs.iter().cloned());
            if !new_obs.is_empty() {
                entity.last_updated = Some(now.clone());
            }
            results.push(AddedObservations {
                entity_name,
                added_observations: new_obs,
            });
        }
        Ok(results)
    }

    /// Delete entities by name, cascading to relations that touch them.
    pub fn delete_entities(&mut self, entity_names: &[String]) {
        let names: std::collections::HashSet<&str> =
            entity_names.iter().map(|s| s.as_str()).collect();
        self.entities.retain(|e| !names.contains(e.name.as_str()));
        self.relations
            .retain(|r| !names.contains(r.from.as_str()) && !names.contains(r.to.as_str()));
    }

    /// Rename an entity, rewriting every relation endpoint that references it.
    pub fn rename_entity(&mut self, name: &str, new_name: &str) -> Result<Value, String> {
        if name.is_empty() || new_name.is_empty() {
            return Err("name and new_name must both be non-empty".into());
        }
        if name == new_name {
            return Err("rename is a no-op".into());
        }
        if self.entity_index(name).is_none() {
            return Err(format!("Entity '{name}' not found"));
        }
        if self.entity_index(new_name).is_some() {
            return Err(format!(
                "Entity '{new_name}' already exists; use merge_entities to combine them"
            ));
        }

        let now = now_iso();
        let idx = self.entity_index(name).unwrap();
        self.entities[idx].name = new_name.to_string();
        self.entities[idx].last_updated = Some(now.clone());

        let mut relations_updated = 0u64;
        for r in &mut self.relations {
            let mut touched = false;
            if r.from == name {
                r.from = new_name.to_string();
                touched = true;
            }
            if r.to == name {
                r.to = new_name.to_string();
                touched = true;
            }
            if touched {
                r.last_updated = Some(now.clone());
                relations_updated += 1;
            }
        }

        Ok(serde_json::json!({
            "renamed": {"from": name, "to": new_name},
            "relationsUpdated": relations_updated,
        }))
    }

    /// Merge `source` into `target`: union observations (target order
    /// preserved), re-point relations, drop self-loops, dedupe relations on
    /// (from, to, relationType) keeping the earliest non-null createdAt,
    /// earliest non-null createdAt on the target, then remove the source.
    pub fn merge_entities(&mut self, source: &str, target: &str) -> Result<Value, String> {
        if source.is_empty() || target.is_empty() {
            return Err("source and target must both be non-empty".into());
        }
        if source == target {
            return Err("cannot merge an entity with itself".into());
        }
        let src_idx = self
            .entity_index(source)
            .ok_or_else(|| format!("Entity '{source}' not found"))?;
        let tgt_idx = self
            .entity_index(target)
            .ok_or_else(|| format!("Entity '{target}' not found"))?;

        let now = now_iso();
        let src = self.entities[src_idx].clone();

        // Observations: union, target order preserved.
        let tgt = &mut self.entities[tgt_idx];
        let existing: std::collections::HashSet<&String> =
            tgt.observations_slice().iter().collect();
        let new_obs: Vec<String> = src
            .observations_slice()
            .iter()
            .filter(|o| !existing.contains(o))
            .cloned()
            .collect();
        let observations_added = new_obs.len() as u64;
        tgt.observations
            .get_or_insert_with(Vec::new)
            .extend(new_obs);

        // entityType: target wins; report source's if it differs.
        let mut discarded_type: Option<String> = None;
        let src_type = src.entity_type.clone().filter(|t| !t.is_empty());
        if let Some(st) = src_type {
            if Some(st.as_str()) != tgt.entity_type.as_deref() {
                discarded_type = Some(st);
            }
        }

        // createdAt: earliest non-null. Both null -> stay null.
        match (&src.created_at, &tgt.created_at) {
            (Some(sc), Some(tc)) => {
                if sc < tc {
                    tgt.created_at = Some(sc.clone());
                }
            }
            (Some(sc), None) => tgt.created_at = Some(sc.clone()),
            _ => {}
        }
        tgt.last_updated = Some(now);

        // Re-point every relation involving source.
        let mut relations_re_pointed = 0u64;
        for r in &mut self.relations {
            let mut touched = false;
            if r.from == source {
                r.from = target.to_string();
                touched = true;
            }
            if r.to == source {
                r.to = target.to_string();
                touched = true;
            }
            if touched {
                relations_re_pointed += 1;
            }
        }

        // Drop self-loops created by re-pointing.
        let before_loops = self.relations.len();
        self.relations.retain(|r| r.from != r.to);
        let self_dropped = before_loops - self.relations.len();

        // Dedupe by (from, to, relationType); keep earliest non-null createdAt.
        let mut seen: HashMap<(String, String, String), Relation> = HashMap::new();
        let mut order: Vec<(String, String, String)> = Vec::new();
        for r in self.relations.drain(..) {
            let key = r.key();
            match seen.get(&key) {
                None => {
                    order.push(key.clone());
                    seen.insert(key, r);
                }
                Some(existing_rel) => {
                    let ec = existing_rel.created_at.clone();
                    let rc = r.created_at.clone();
                    if let Some(rc) = rc {
                        if ec.is_none() || rc < ec.unwrap() {
                            seen.insert(key, r);
                        }
                    }
                }
            }
        }
        let dedup_len = order.len();
        self.relations = order
            .into_iter()
            .map(|k| seen.remove(&k).unwrap())
            .collect();
        let dup_dropped = before_loops - self_dropped - dedup_len;

        let relations_dropped = (self_dropped + dup_dropped) as u64;

        // Remove the source entity.
        self.entities.retain(|e| e.name != source);

        Ok(serde_json::json!({
            "merged": {"source": source, "target": target},
            "observationsAdded": observations_added,
            "relationsRePointed": relations_re_pointed,
            "relationsDropped": relations_dropped,
            "discardedType": discarded_type,
        }))
    }

    /// Replace the first observation matching `original_text` in place
    /// (web UI edit flow). Returns false if entity or text is missing.
    pub fn update_observation(
        &mut self,
        entity_name: &str,
        original_text: &str,
        new_text: &str,
    ) -> bool {
        match self.entity_index(entity_name) {
            None => false,
            Some(idx) => {
                let entity = &mut self.entities[idx];
                let obs = entity.observations.get_or_insert_with(Vec::new);
                match obs.iter().position(|o| o == original_text) {
                    None => false,
                    Some(i) => {
                        obs[i] = new_text.to_string();
                        entity.last_updated = Some(now_iso());
                        true
                    }
                }
            }
        }
    }

    /// Remove specific observations from entities. Missing entities and
    /// missing observations are silently ignored.
    pub fn delete_observations(&mut self, deletions: Vec<(String, Vec<String>)>) {
        let now = now_iso();
        for (entity_name, observations) in deletions {
            let Some(idx) = self.entity_index(&entity_name) else {
                continue;
            };
            let to_remove: std::collections::HashSet<&String> = observations.iter().collect();
            let entity = &mut self.entities[idx];
            let before = entity.observations_slice().len();
            if let Some(obs) = &mut entity.observations {
                obs.retain(|o| !to_remove.contains(o));
                if obs.len() < before {
                    entity.last_updated = Some(now.clone());
                }
            }
        }
    }

    /// Remove relations matching (from, to, relationType) exactly.
    pub fn delete_relations(&mut self, relations: Vec<(String, String, String)>) {
        let to_delete: std::collections::HashSet<(String, String, String)> =
            relations.into_iter().collect();
        self.relations.retain(|r| !to_delete.contains(&r.key()));
    }

    /// Case-insensitive substring search across entity names, types, and
    /// observations; includes relations where either endpoint matched.
    pub fn search_nodes(&self, query: &str) -> Graph {
        let q = query.to_lowercase();
        let entities: Vec<Entity> = self
            .entities
            .iter()
            .filter(|e| {
                e.name.to_lowercase().contains(&q)
                    || e.entity_type_str().to_lowercase().contains(&q)
                    || e.observations_slice()
                        .iter()
                        .any(|o| o.to_lowercase().contains(&q))
            })
            .cloned()
            .collect();
        let names: std::collections::HashSet<&str> =
            entities.iter().map(|e| e.name.as_str()).collect();
        let relations = self
            .relations
            .iter()
            .filter(|r| names.contains(r.from.as_str()) || names.contains(r.to.as_str()))
            .cloned()
            .collect();
        Graph { entities, relations }
    }

    /// Fetch entities by exact name plus relations touching any of them.
    pub fn open_nodes(&self, names: &[String]) -> Graph {
        let names_set: std::collections::HashSet<&str> =
            names.iter().map(|s| s.as_str()).collect();
        let entities = self
            .entities
            .iter()
            .filter(|e| names_set.contains(e.name.as_str()))
            .cloned()
            .collect();
        let relations = self
            .relations
            .iter()
            .filter(|r| names_set.contains(r.from.as_str()) || names_set.contains(r.to.as_str()))
            .cloned()
            .collect();
        Graph { entities, relations }
    }

    /// Apply alias/Title Case normalization to every entity's type.
    /// Returns the list of changes; mutates only entities whose type changed.
    pub fn normalize_all_entity_types(
        &mut self,
        aliases: &HashMap<String, String>,
    ) -> Vec<TypeChange> {
        let now = now_iso();
        let mut changes = Vec::new();
        for entity in &mut self.entities {
            let old_type = entity.entity_type_str().to_string();
            let new_type = normalize_entity_type(&old_type, aliases);
            if old_type != new_type {
                changes.push(TypeChange {
                    name: entity.name.clone(),
                    old_type,
                    new_type: new_type.clone(),
                });
                entity.entity_type = Some(new_type);
                entity.last_updated = Some(now.clone());
            }
        }
        changes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aliases() -> HashMap<String, String> {
        HashMap::from([
            ("person".to_string(), "Person".to_string()),
            ("colleague".to_string(), "Person".to_string()),
            ("work project".to_string(), "Project".to_string()),
        ])
    }

    fn entity(name: &str, etype: &str, obs: &[&str]) -> Entity {
        Entity {
            name: name.into(),
            entity_type: Some(etype.into()),
            observations: Some(obs.iter().map(|s| s.to_string()).collect()),
            created_at: None,
            last_updated: None,
            extra: Map::new(),
        }
    }

    fn relation(from: &str, to: &str, rt: &str) -> Relation {
        Relation {
            from: from.into(),
            to: to.into(),
            relation_type: rt.into(),
            created_at: None,
            last_updated: None,
            extra: Map::new(),
        }
    }

    #[test]
    fn py_title_matches_python_semantics() {
        assert_eq!(py_title("hello world"), "Hello World");
        assert_eq!(py_title("1password employee"), "1Password Employee");
        assert_eq!(py_title("o'neil"), "O'Neil");
        assert_eq!(py_title("ABC DEF"), "Abc Def");
        assert_eq!(py_title("weird gadget thing"), "Weird Gadget Thing");
        assert_eq!(py_title(""), "");
    }

    #[test]
    fn normalize_uses_alias_case_insensitively() {
        let a = aliases();
        assert_eq!(normalize_entity_type("PERSON", &a), "Person");
        assert_eq!(normalize_entity_type("colleague", &a), "Person");
        assert_eq!(normalize_entity_type("Work Project", &a), "Project");
        assert_eq!(normalize_entity_type("strange thing", &a), "Strange Thing");
        assert_eq!(normalize_entity_type("", &a), "");
    }

    #[test]
    fn create_entities_dedups_against_existing_only() {
        let mut g = Graph::default();
        let added = g.create_entities(vec![entity("Alice", "person", &["a"])], &aliases());
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].entity_type.as_deref(), Some("Person"));
        assert!(added[0].created_at.is_some());

        let added2 = g.create_entities(vec![entity("Alice", "person", &[])], &aliases());
        assert!(added2.is_empty());
        assert_eq!(g.entities.len(), 1);

        // Python quirk preserved: duplicates within one batch are all added.
        let added3 = g.create_entities(
            vec![entity("Bob", "person", &[]), entity("Bob", "person", &[])],
            &aliases(),
        );
        assert_eq!(added3.len(), 2);
    }

    #[test]
    fn create_relations_dedups_against_existing_only() {
        let mut g = Graph::default();
        g.create_entities(
            vec![entity("A", "person", &[]), entity("B", "person", &[])],
            &aliases(),
        );
        let added = g.create_relations(vec![
            relation("A", "B", "knows"),
            relation("A", "B", "manages"),
        ]);
        assert_eq!(added.len(), 2);
        let added2 = g.create_relations(vec![relation("A", "B", "knows")]);
        assert!(added2.is_empty());
    }

    #[test]
    fn add_observations_dedups_and_errors_without_mutation() {
        let mut g = Graph::default();
        g.create_entities(vec![entity("Alice", "person", &["x"])], &aliases());

        let res = g
            .add_observations(vec![("Alice".into(), vec!["x".into(), "y".into()])])
            .unwrap();
        assert_eq!(res[0].added_observations, vec!["y".to_string()]);

        let err = g
            .add_observations(vec![
                ("Alice".into(), vec!["z".into()]),
                ("Nobody".into(), vec!["w".into()]),
            ])
            .unwrap_err();
        assert_eq!(err, "Entity 'Nobody' not found");
        // Pre-validation means Alice was NOT mutated by the failed call.
        assert_eq!(g.entities[0].observations_slice(), ["x", "y"]);
    }

    #[test]
    fn delete_entities_cascades_relations() {
        let mut g = Graph::default();
        g.create_entities(
            vec![entity("A", "t", &[]), entity("B", "t", &[]), entity("C", "t", &[])],
            &aliases(),
        );
        g.create_relations(vec![
            relation("A", "B", "knows"),
            relation("B", "C", "knows"),
            relation("C", "A", "knows"),
        ]);
        g.delete_entities(&["A".to_string()]);
        assert_eq!(g.entities.len(), 2);
        assert_eq!(g.relations.len(), 1);
        assert_eq!(g.relations[0].from, "B");
    }

    #[test]
    fn rename_entity_rewrites_relations_and_errors() {
        let mut g = Graph::default();
        g.create_entities(
            vec![entity("Old", "t", &[]), entity("Other", "t", &[])],
            &aliases(),
        );
        g.create_relations(vec![
            relation("Old", "Other", "knows"),
            relation("Other", "Old", "knows"),
        ]);

        let result = g.rename_entity("Old", "New").unwrap();
        assert_eq!(result["renamed"]["from"], "Old");
        assert_eq!(result["renamed"]["to"], "New");
        assert_eq!(result["relationsUpdated"], 2);
        assert!(g.entity_index("New").is_some());
        assert!(g.relations.iter().all(|r| r.from != "Old" && r.to != "Old"));

        assert_eq!(g.rename_entity("Nobody", "X").unwrap_err(), "Entity 'Nobody' not found");
        assert_eq!(
            g.rename_entity("New", "Other").unwrap_err(),
            "Entity 'Other' already exists; use merge_entities to combine them"
        );
        assert_eq!(g.rename_entity("New", "New").unwrap_err(), "rename is a no-op");
        assert_eq!(
            g.rename_entity("", "X").unwrap_err(),
            "name and new_name must both be non-empty"
        );
    }

    #[test]
    fn merge_entities_full_semantics() {
        let mut g = Graph::default();
        let mut src = entity("Bob", "Colleague", &["b1", "shared"]);
        src.created_at = Some("2020-01-01T00:00:00+00:00".into());
        let mut tgt = entity("Alice", "Person", &["shared", "a1"]);
        tgt.created_at = Some("2021-01-01T00:00:00+00:00".into());
        g.entities.push(src);
        g.entities.push(tgt);
        g.entities.push(entity("P", "Project", &[]));
        g.create_relations(vec![
            relation("Bob", "P", "worksOn"),
            relation("Alice", "P", "worksOn"), // will collide after re-point
            relation("Alice", "Bob", "manages"), // becomes self-loop
        ]);

        let result = g.merge_entities("Bob", "Alice").unwrap();
        assert_eq!(result["merged"]["source"], "Bob");
        assert_eq!(result["observationsAdded"], 1);
        assert_eq!(result["relationsRePointed"], 2);
        assert_eq!(result["relationsDropped"], 2); // 1 self-loop + 1 duplicate
        assert_eq!(result["discardedType"], "Colleague");

        let alice = &g.entities[g.entity_index("Alice").unwrap()];
        assert_eq!(alice.observations_slice(), ["shared", "a1", "b1"]);
        assert_eq!(alice.created_at.as_deref(), Some("2020-01-01T00:00:00+00:00"));
        assert!(g.entity_index("Bob").is_none());
        assert_eq!(g.relations.len(), 1);

        assert_eq!(
            g.merge_entities("Alice", "Alice").unwrap_err(),
            "cannot merge an entity with itself"
        );
        assert_eq!(
            g.merge_entities("Nobody", "Alice").unwrap_err(),
            "Entity 'Nobody' not found"
        );
    }

    #[test]
    fn merge_keeps_earliest_created_at_on_duplicate_relations() {
        let mut g = Graph::default();
        g.entities.push(entity("S", "t", &[]));
        g.entities.push(entity("T", "t", &[]));
        g.entities.push(entity("P", "t", &[]));
        let mut r1 = relation("S", "P", "knows");
        r1.created_at = Some("2020-01-01T00:00:00+00:00".into());
        let mut r2 = relation("T", "P", "knows");
        r2.created_at = Some("2022-01-01T00:00:00+00:00".into());
        g.relations.push(r2);
        g.relations.push(r1);

        g.merge_entities("S", "T").unwrap();
        assert_eq!(g.relations.len(), 1);
        assert_eq!(
            g.relations[0].created_at.as_deref(),
            Some("2020-01-01T00:00:00+00:00")
        );
    }

    #[test]
    fn search_is_case_insensitive_across_fields() {
        let mut g = Graph::default();
        g.create_entities(
            vec![
                entity("Alice", "person", &["Lives in Boston"]),
                entity("Bob", "person", &[]),
                entity("Carol", "work project", &[]),
            ],
            &aliases(),
        );
        g.create_relations(vec![relation("Alice", "Bob", "knows")]);

        assert_eq!(g.search_nodes("ALICE").entities.len(), 1);
        assert_eq!(g.search_nodes("boston").entities.len(), 1);
        assert_eq!(g.search_nodes("person").entities.len(), 2);
        let r = g.search_nodes("alice");
        assert_eq!(r.relations.len(), 1); // Bob endpoint included
        assert!(g.search_nodes("zzz").entities.is_empty());
    }

    #[test]
    fn update_observation_replaces_in_place() {
        let mut g = Graph::default();
        g.create_entities(vec![entity("A", "t", &["one", "two"])], &aliases());
        assert!(g.update_observation("A", "one", "uno"));
        assert_eq!(g.entities[0].observations_slice(), ["uno", "two"]);
        assert!(!g.update_observation("A", "missing", "x"));
        assert!(!g.update_observation("Nobody", "one", "x"));
    }

    #[test]
    fn delete_observations_silent_on_missing() {
        let mut g = Graph::default();
        g.create_entities(vec![entity("A", "t", &["one", "two"])], &aliases());
        g.delete_observations(vec![
            ("A".into(), vec!["one".into(), "not-there".into()]),
            ("Nobody".into(), vec!["x".into()]),
        ]);
        assert_eq!(g.entities[0].observations_slice(), ["two"]);
    }

    #[test]
    fn normalize_all_reports_changes() {
        let mut g = Graph::default();
        g.entities.push(entity("A", "colleague", &[]));
        g.entities.push(entity("B", "Person", &[]));
        let changes = g.normalize_all_entity_types(&aliases());
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].old_type, "colleague");
        assert_eq!(changes[0].new_type, "Person");
        assert_eq!(g.entities[0].entity_type.as_deref(), Some("Person"));
    }
}
