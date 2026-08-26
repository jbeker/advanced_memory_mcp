// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Jeremy Beker

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

/// A single observation as a bitemporal fact.
///
/// `validFrom`/`validTo` bound when the fact was true in the world (null
/// `validTo` = still current); `recordedAt` is when the server learned it;
/// `source` is a free-form provenance reference. Legacy v1 files store
/// observations as plain strings — the deserializer accepts both and the
/// serializer always writes objects, so files upgrade in place on first
/// write. A fact's identity within its entity is its exact `text` (dedup,
/// delete, update, and `supersedes` references all match on it).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Fact {
    pub text: String,
    #[serde(rename = "validFrom", skip_serializing_if = "Option::is_none")]
    pub valid_from: Option<String>,
    #[serde(rename = "validTo", skip_serializing_if = "Option::is_none")]
    pub valid_to: Option<String>,
    #[serde(rename = "recordedAt", skip_serializing_if = "Option::is_none")]
    pub recorded_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Fact {
    pub fn from_text(text: impl Into<String>) -> Self {
        Fact {
            text: text.into(),
            valid_from: None,
            valid_to: None,
            recorded_at: None,
            source: None,
            extra: Map::new(),
        }
    }

    /// Still-current: no end to its validity interval.
    pub fn is_active(&self) -> bool {
        self.valid_to.is_none()
    }
}

impl<'de> Deserialize<'de> for Fact {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct FactObject {
            text: String,
            #[serde(rename = "validFrom", default)]
            valid_from: Option<String>,
            #[serde(rename = "validTo", default)]
            valid_to: Option<String>,
            #[serde(rename = "recordedAt", default)]
            recorded_at: Option<String>,
            #[serde(default)]
            source: Option<String>,
            #[serde(flatten)]
            extra: Map<String, Value>,
        }

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Legacy(String),
            Object(FactObject),
        }

        Ok(match Repr::deserialize(deserializer)? {
            Repr::Legacy(text) => Fact::from_text(text),
            Repr::Object(o) => Fact {
                text: o.text,
                valid_from: o.valid_from,
                valid_to: o.valid_to,
                recorded_at: o.recorded_at,
                source: o.source,
                extra: o.extra,
            },
        })
    }
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
    pub observations: Option<Vec<Fact>>,
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

    pub fn observations_slice(&self) -> &[Fact] {
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

/// One observation to write: the fact fields a caller may set, plus an
/// optional `supersedes` reference (exact text of an active fact on the same
/// entity to close when this one lands).
#[derive(Debug, Clone)]
pub struct FactInput {
    pub text: String,
    pub valid_from: Option<String>,
    pub valid_to: Option<String>,
    pub source: Option<String>,
    pub supersedes: Option<String>,
    pub extra: Map<String, Value>,
}

impl FactInput {
    pub fn from_text(text: impl Into<String>) -> Self {
        FactInput {
            text: text.into(),
            valid_from: None,
            valid_to: None,
            source: None,
            supersedes: None,
            extra: Map::new(),
        }
    }
}

/// Per-entity result of `add_observations`.
#[derive(Debug, Serialize)]
pub struct AddedObservations {
    #[serde(rename = "entityName")]
    pub entity_name: String,
    #[serde(rename = "addedObservations")]
    pub added_observations: Vec<Fact>,
}

/// Filters for `search_facts`. All optional; they layer (AND).
#[derive(Debug, Default)]
pub struct FactQuery {
    /// Case-insensitive substring on fact text.
    pub query: Option<String>,
    /// Exact entity name.
    pub entity: Option<String>,
    /// Facts valid at this instant: validFrom <= asOf < validTo.
    pub as_of: Option<String>,
    /// validFrom >= since (facts without validFrom are excluded).
    pub since: Option<String>,
    /// validFrom <= until (facts without validFrom are excluded).
    pub until: Option<String>,
    /// Include facts whose validity has ended. Implied by as_of.
    pub include_superseded: bool,
    /// Sort ascending by validFrom instead of the default descending.
    pub ascending: bool,
    /// Maximum facts returned.
    pub limit: usize,
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

    /// Append observations as bitemporal facts, deduplicating by text
    /// against the entity's existing facts. `recordedAt` is set to now;
    /// `validFrom` defaults to now when not supplied. A `supersedes`
    /// reference closes the named active fact by setting its `validTo` to
    /// the new fact's `validFrom`.
    ///
    /// Errors (without mutating) if any named entity is missing, or if a
    /// supersedes target doesn't exist, isn't active, or would be closed
    /// twice in one call. Entries skipped by dedup do NOT apply their
    /// supersedes — the whole entry is a no-op.
    pub fn add_observations(
        &mut self,
        observations: Vec<(String, Vec<FactInput>)>,
    ) -> Result<Vec<AddedObservations>, String> {
        let now = now_iso();

        // Validate everything against current state before touching it.
        let mut pending_closures: std::collections::HashSet<(String, String)> =
            std::collections::HashSet::new();
        for (entity_name, inputs) in &observations {
            let Some(idx) = self.entity_index(entity_name) else {
                return Err(format!("Entity '{entity_name}' not found"));
            };
            let entity = &self.entities[idx];
            let existing_texts: std::collections::HashSet<&str> = entity
                .observations_slice()
                .iter()
                .map(|f| f.text.as_str())
                .collect();
            for input in inputs {
                let Some(target) = &input.supersedes else { continue };
                if existing_texts.contains(input.text.as_str()) {
                    // Dedup will skip this entry entirely; its supersedes is
                    // intentionally not applied.
                    continue;
                }
                let Some(target_fact) = entity
                    .observations_slice()
                    .iter()
                    .find(|f| &f.text == target)
                else {
                    return Err(format!(
                        "Cannot supersede: no observation with text '{target}' on entity '{entity_name}'"
                    ));
                };
                if !target_fact.is_active() {
                    return Err(format!(
                        "Cannot supersede: observation '{target}' on entity '{entity_name}' is already superseded"
                    ));
                }
                if !pending_closures.insert((entity_name.clone(), target.clone())) {
                    return Err(format!(
                        "Cannot supersede: observation '{target}' on entity '{entity_name}' is superseded twice in this call"
                    ));
                }
            }
        }

        let mut results = Vec::new();
        for (entity_name, inputs) in observations {
            let idx = self.entity_index(&entity_name).unwrap();
            let entity = &mut self.entities[idx];
            let mut added: Vec<Fact> = Vec::new();
            for input in inputs {
                let exists = entity
                    .observations_slice()
                    .iter()
                    .any(|f| f.text == input.text)
                    || added.iter().any(|f| f.text == input.text);
                if exists {
                    continue;
                }
                let fact = Fact {
                    text: input.text,
                    valid_from: Some(input.valid_from.unwrap_or_else(|| now.clone())),
                    valid_to: input.valid_to,
                    recorded_at: Some(now.clone()),
                    source: input.source,
                    extra: input.extra,
                };
                if let Some(target) = input.supersedes {
                    let closed_at = fact.valid_from.clone();
                    let target_fact = entity
                        .observations
                        .get_or_insert_with(Vec::new)
                        .iter_mut()
                        .find(|f| f.text == target)
                        .expect("validated above");
                    target_fact.valid_to = closed_at;
                }
                entity
                    .observations
                    .get_or_insert_with(Vec::new)
                    .push(fact.clone());
                added.push(fact);
            }
            if !added.is_empty() {
                entity.last_updated = Some(now.clone());
            }
            results.push(AddedObservations {
                entity_name,
                added_observations: added,
            });
        }
        Ok(results)
    }

    /// Fact-level temporal search. Returns (matching facts with their entity
    /// names, total match count before `limit`).
    pub fn search_facts(&self, q: &FactQuery) -> (Vec<(String, Fact)>, usize) {
        let needle = q.query.as_deref().map(str::to_lowercase);
        let mut hits: Vec<(&str, &Fact)> = Vec::new();

        for entity in &self.entities {
            if let Some(name) = &q.entity {
                if &entity.name != name {
                    continue;
                }
            }
            for fact in entity.observations_slice() {
                if let Some(needle) = &needle {
                    if !fact.text.to_lowercase().contains(needle) {
                        continue;
                    }
                }
                if let Some(as_of) = &q.as_of {
                    // Null validFrom counts as always-valid history.
                    if let Some(from) = &fact.valid_from {
                        if from > as_of {
                            continue;
                        }
                    }
                    if let Some(to) = &fact.valid_to {
                        if to <= as_of {
                            continue;
                        }
                    }
                } else if !q.include_superseded && !fact.is_active() {
                    continue;
                }
                if let Some(since) = &q.since {
                    match &fact.valid_from {
                        Some(from) if from >= since => {}
                        _ => continue, // unplaceable facts are excluded from windows
                    }
                }
                if let Some(until) = &q.until {
                    match &fact.valid_from {
                        Some(from) if from <= until => {}
                        _ => continue,
                    }
                }
                hits.push((&entity.name, fact));
            }
        }

        // validFrom (nulls last), ties by recordedAt; direction per query.
        let key = |f: &Fact| (f.valid_from.clone(), f.recorded_at.clone());
        if q.ascending {
            hits.sort_by(|a, b| match (key(a.1), key(b.1)) {
                ((None, _), (Some(_), _)) => std::cmp::Ordering::Greater,
                ((Some(_), _), (None, _)) => std::cmp::Ordering::Less,
                (ka, kb) => ka.cmp(&kb),
            });
        } else {
            hits.sort_by(|a, b| match (key(a.1), key(b.1)) {
                ((None, _), (Some(_), _)) => std::cmp::Ordering::Greater,
                ((Some(_), _), (None, _)) => std::cmp::Ordering::Less,
                (ka, kb) => kb.cmp(&ka),
            });
        }

        let total = hits.len();
        let facts = hits
            .into_iter()
            .take(q.limit)
            .map(|(name, fact)| (name.to_string(), fact.clone()))
            .collect();
        (facts, total)
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

        // Observations: union by text, target order preserved.
        let tgt = &mut self.entities[tgt_idx];
        let existing: std::collections::HashSet<&str> = tgt
            .observations_slice()
            .iter()
            .map(|f| f.text.as_str())
            .collect();
        let new_obs: Vec<Fact> = src
            .observations_slice()
            .iter()
            .filter(|f| !existing.contains(f.text.as_str()))
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

    /// Replace the text of the first fact matching `original_text` in place,
    /// preserving its temporal fields (web UI edit flow). Returns the
    /// updated fact, or None if entity or text is missing.
    pub fn update_observation(
        &mut self,
        entity_name: &str,
        original_text: &str,
        new_text: &str,
    ) -> Option<Fact> {
        let idx = self.entity_index(entity_name)?;
        let entity = &mut self.entities[idx];
        let obs = entity.observations.get_or_insert_with(Vec::new);
        let i = obs.iter().position(|f| f.text == original_text)?;
        obs[i].text = new_text.to_string();
        let updated = obs[i].clone();
        entity.last_updated = Some(now_iso());
        Some(updated)
    }

    /// Remove facts by exact text. Missing entities and missing
    /// observations are silently ignored.
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
                obs.retain(|f| !to_remove.contains(&f.text));
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
                        .any(|f| f.text.to_lowercase().contains(&q))
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
            observations: Some(obs.iter().map(|s| Fact::from_text(*s)).collect()),
            created_at: None,
            last_updated: None,
            extra: Map::new(),
        }
    }

    fn obs_texts(g: &Graph, name: &str) -> Vec<String> {
        let idx = g.entity_index(name).unwrap();
        g.entities[idx]
            .observations_slice()
            .iter()
            .map(|f| f.text.clone())
            .collect()
    }

    fn fact_at(text: &str, from: &str, to: Option<&str>) -> FactInput {
        FactInput {
            text: text.into(),
            valid_from: Some(from.into()),
            valid_to: to.map(Into::into),
            source: None,
            supersedes: None,
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
            .add_observations(vec![(
                "Alice".into(),
                vec![FactInput::from_text("x"), FactInput::from_text("y")],
            )])
            .unwrap();
        let texts: Vec<&str> = res[0]
            .added_observations
            .iter()
            .map(|f| f.text.as_str())
            .collect();
        assert_eq!(texts, ["y"]);

        let err = g
            .add_observations(vec![
                ("Alice".into(), vec![FactInput::from_text("z")]),
                ("Nobody".into(), vec![FactInput::from_text("w")]),
            ])
            .unwrap_err();
        assert_eq!(err, "Entity 'Nobody' not found");
        // Pre-validation means Alice was NOT mutated by the failed call.
        assert_eq!(obs_texts(&g, "Alice"), ["x", "y"]);
    }

    #[test]
    fn add_defaults_valid_from_and_recorded_at_to_now() {
        let mut g = Graph::default();
        g.create_entities(vec![entity("A", "t", &[])], &aliases());
        let res = g
            .add_observations(vec![("A".into(), vec![FactInput::from_text("fresh")])])
            .unwrap();
        let fact = &res[0].added_observations[0];
        assert!(fact.valid_from.is_some());
        assert_eq!(fact.valid_from, fact.recorded_at);
        assert!(fact.valid_to.is_none());

        // Explicit validFrom (event date) is preserved; recordedAt is now.
        let res = g
            .add_observations(vec![(
                "A".into(),
                vec![fact_at("backfilled", "2026-05-01", None)],
            )])
            .unwrap();
        let fact = &res[0].added_observations[0];
        assert_eq!(fact.valid_from.as_deref(), Some("2026-05-01"));
        assert_ne!(fact.recorded_at, fact.valid_from);
    }

    #[test]
    fn supersedes_closes_the_target_fact() {
        let mut g = Graph::default();
        g.create_entities(vec![entity("A", "t", &[])], &aliases());
        g.add_observations(vec![(
            "A".into(),
            vec![fact_at("target is January", "2026-01-10", None)],
        )])
        .unwrap();

        let mut input = fact_at("target is March", "2026-02-15", None);
        input.supersedes = Some("target is January".into());
        g.add_observations(vec![("A".into(), vec![input])]).unwrap();

        let idx = g.entity_index("A").unwrap();
        let old = &g.entities[idx].observations_slice()[0];
        assert_eq!(old.valid_to.as_deref(), Some("2026-02-15"));
        let new = &g.entities[idx].observations_slice()[1];
        assert!(new.is_active());
    }

    #[test]
    fn supersedes_error_paths_leave_graph_unchanged() {
        let mut g = Graph::default();
        g.create_entities(vec![entity("A", "t", &[])], &aliases());
        g.add_observations(vec![(
            "A".into(),
            vec![fact_at("old", "2026-01-01", Some("2026-02-01")), fact_at("active", "2026-02-01", None)],
        )])
        .unwrap();

        // Missing target.
        let mut input = fact_at("new1", "2026-03-01", None);
        input.supersedes = Some("nonexistent".into());
        let err = g.add_observations(vec![("A".into(), vec![input])]).unwrap_err();
        assert!(err.contains("no observation with text 'nonexistent'"), "{err}");

        // Already-superseded target.
        let mut input = fact_at("new2", "2026-03-01", None);
        input.supersedes = Some("old".into());
        let err = g.add_observations(vec![("A".into(), vec![input])]).unwrap_err();
        assert!(err.contains("already superseded"), "{err}");

        // Double-close in one call.
        let mut i1 = fact_at("new3", "2026-03-01", None);
        i1.supersedes = Some("active".into());
        let mut i2 = fact_at("new4", "2026-03-02", None);
        i2.supersedes = Some("active".into());
        let err = g
            .add_observations(vec![("A".into(), vec![i1, i2])])
            .unwrap_err();
        assert!(err.contains("superseded twice"), "{err}");

        // Nothing changed.
        assert_eq!(obs_texts(&g, "A"), ["old", "active"]);
        let idx = g.entity_index("A").unwrap();
        assert!(g.entities[idx].observations_slice()[1].is_active());
    }

    #[test]
    fn dedup_skipped_entry_does_not_apply_supersedes() {
        let mut g = Graph::default();
        g.create_entities(vec![entity("A", "t", &[])], &aliases());
        g.add_observations(vec![(
            "A".into(),
            vec![fact_at("existing", "2026-01-01", None), fact_at("victim", "2026-01-02", None)],
        )])
        .unwrap();

        // "existing" is a duplicate; its supersedes must be ignored.
        let mut input = fact_at("existing", "2026-03-01", None);
        input.supersedes = Some("victim".into());
        let res = g.add_observations(vec![("A".into(), vec![input])]).unwrap();
        assert!(res[0].added_observations.is_empty());
        let idx = g.entity_index("A").unwrap();
        assert!(g.entities[idx].observations_slice()[1].is_active());
    }

    #[test]
    fn search_facts_active_only_default_and_include_superseded() {
        let mut g = Graph::default();
        g.create_entities(vec![entity("A", "t", &[])], &aliases());
        g.add_observations(vec![(
            "A".into(),
            vec![
                fact_at("old plan", "2026-01-01", Some("2026-02-01")),
                fact_at("current plan", "2026-02-01", None),
            ],
        )])
        .unwrap();

        let (facts, total) = g.search_facts(&FactQuery {
            query: Some("plan".into()),
            limit: 10,
            ..Default::default()
        });
        assert_eq!(total, 1);
        assert_eq!(facts[0].1.text, "current plan");

        let (facts, total) = g.search_facts(&FactQuery {
            query: Some("plan".into()),
            include_superseded: true,
            limit: 10,
            ..Default::default()
        });
        assert_eq!(total, 2);
        // Default descending by validFrom.
        assert_eq!(facts[0].1.text, "current plan");
        assert_eq!(facts[1].1.text, "old plan");
    }

    #[test]
    fn search_facts_as_of_returns_truth_at_that_time() {
        let mut g = Graph::default();
        g.create_entities(vec![entity("A", "t", &["legacy note"])], &aliases());
        g.add_observations(vec![(
            "A".into(),
            vec![
                fact_at("guess: January", "2026-01-01", Some("2026-03-01")),
                fact_at("firm: April", "2026-03-01", None),
            ],
        )])
        .unwrap();

        let q = |as_of: &str| FactQuery {
            as_of: Some(as_of.into()),
            limit: 10,
            ..Default::default()
        };
        // Mid-February: the superseded guess was the standing truth.
        let (facts, _) = g.search_facts(&q("2026-02-01"));
        let texts: Vec<&str> = facts.iter().map(|f| f.1.text.as_str()).collect();
        assert!(texts.contains(&"guess: January"));
        assert!(!texts.contains(&"firm: April"));
        // Null validFrom (legacy) counts as always valid.
        assert!(texts.contains(&"legacy note"));

        // Boundary: validTo is exclusive, validFrom inclusive.
        let (facts, _) = g.search_facts(&q("2026-03-01"));
        let texts: Vec<&str> = facts.iter().map(|f| f.1.text.as_str()).collect();
        assert!(!texts.contains(&"guess: January"));
        assert!(texts.contains(&"firm: April"));
    }

    #[test]
    fn search_facts_window_excludes_unplaceable_facts() {
        let mut g = Graph::default();
        g.create_entities(vec![entity("A", "t", &["legacy note"])], &aliases());
        g.add_observations(vec![(
            "A".into(),
            vec![
                fact_at("february item", "2026-02-15", None),
                fact_at("may item", "2026-05-10", None),
            ],
        )])
        .unwrap();

        let (facts, total) = g.search_facts(&FactQuery {
            since: Some("2026-05-01".into()),
            limit: 10,
            ..Default::default()
        });
        assert_eq!(total, 1);
        assert_eq!(facts[0].1.text, "may item");

        let (facts, _) = g.search_facts(&FactQuery {
            since: Some("2026-01-01".into()),
            until: Some("2026-03-01".into()),
            limit: 10,
            ..Default::default()
        });
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].1.text, "february item");
    }

    #[test]
    fn search_facts_ordering_limit_and_entity_filter() {
        let mut g = Graph::default();
        g.create_entities(
            vec![entity("A", "t", &["nodate"]), entity("B", "t", &[])],
            &aliases(),
        );
        g.add_observations(vec![
            ("A".into(), vec![fact_at("first", "2026-01-01", None)]),
            ("B".into(), vec![fact_at("second", "2026-02-01", None)]),
        ])
        .unwrap();

        // History: ascending, nulls last.
        let (facts, total) = g.search_facts(&FactQuery {
            ascending: true,
            include_superseded: true,
            limit: 10,
            ..Default::default()
        });
        assert_eq!(total, 3);
        let texts: Vec<&str> = facts.iter().map(|f| f.1.text.as_str()).collect();
        assert_eq!(texts, ["first", "second", "nodate"]);

        // Limit truncates but total reports everything.
        let (facts, total) = g.search_facts(&FactQuery {
            limit: 1,
            ..Default::default()
        });
        assert_eq!(facts.len(), 1);
        assert_eq!(total, 3);
        assert_eq!(facts[0].1.text, "second"); // most recent validFrom first

        // Entity filter.
        let (facts, _) = g.search_facts(&FactQuery {
            entity: Some("B".into()),
            limit: 10,
            ..Default::default()
        });
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].0, "B");
    }

    #[test]
    fn fact_deserializes_from_string_or_object() {
        let legacy: Fact = serde_json::from_str("\"plain text\"").unwrap();
        assert_eq!(legacy.text, "plain text");
        assert!(legacy.valid_from.is_none());

        let full: Fact = serde_json::from_str(
            r#"{"text": "t", "validFrom": "2026-01-01", "validTo": null, "recordedAt": "2026-01-02", "source": "1:1 notes", "custom": 7}"#,
        )
        .unwrap();
        assert_eq!(full.valid_from.as_deref(), Some("2026-01-01"));
        assert!(full.valid_to.is_none());
        assert_eq!(full.source.as_deref(), Some("1:1 notes"));
        assert_eq!(full.extra["custom"], 7);
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

        assert_eq!(obs_texts(&g, "Alice"), ["shared", "a1", "b1"]);
        let alice = &g.entities[g.entity_index("Alice").unwrap()];
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
    fn update_observation_replaces_text_preserving_temporal_fields() {
        let mut g = Graph::default();
        g.create_entities(vec![entity("A", "t", &[])], &aliases());
        g.add_observations(vec![(
            "A".into(),
            vec![fact_at("one", "2026-01-01", None), fact_at("two", "2026-01-02", None)],
        )])
        .unwrap();
        let updated = g.update_observation("A", "one", "uno").unwrap();
        assert_eq!(updated.text, "uno");
        assert_eq!(updated.valid_from.as_deref(), Some("2026-01-01"));
        assert_eq!(obs_texts(&g, "A"), ["uno", "two"]);
        assert!(g.update_observation("A", "missing", "x").is_none());
        assert!(g.update_observation("Nobody", "one", "x").is_none());
    }

    #[test]
    fn delete_observations_silent_on_missing() {
        let mut g = Graph::default();
        g.create_entities(vec![entity("A", "t", &["one", "two"])], &aliases());
        g.delete_observations(vec![
            ("A".into(), vec!["one".into(), "not-there".into()]),
            ("Nobody".into(), vec!["x".into()]),
        ]);
        assert_eq!(obs_texts(&g, "A"), ["two"]);
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
