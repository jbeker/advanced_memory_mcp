//! JSONL persistence and the in-memory store registry.
//!
//! This replaces Python's load-on-every-call / truncate-write-on-every-call
//! model with: load once, serve from memory, persist mutations via
//! write-to-temp + fsync + atomic rename. Concurrent mutations serialize on a
//! per-file write lock, so the two corruption modes of the Python server
//! (interleaved writers, readers seeing a half-truncated file) cannot occur.

use crate::graph::{Entity, Graph, Relation};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("failed to read {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid JSON on line {line} of {path}: {source}")]
    Parse {
        path: String,
        line: usize,
        #[source]
        source: serde_json::Error,
    },
}

/// Wire format for a JSONL line: the discriminator first, then the item's
/// own fields, matching Python's `{"type": ..., **item}` construction.
#[derive(Serialize)]
struct Line<'a, T: Serialize> {
    r#type: &'static str,
    #[serde(flatten)]
    item: &'a T,
}

/// Parse a JSONL data file. Blank lines are skipped; lines whose "type" is
/// neither "entity" nor "relation" are ignored (Python drops them too).
/// A missing or empty file yields an empty graph.
pub fn load_graph(path: &Path) -> Result<Graph, StoreError> {
    let to_io = |source| StoreError::Io { path: path.display().to_string(), source };

    if !path.exists() {
        return Ok(Graph::default());
    }
    let contents = std::fs::read_to_string(path).map_err(to_io)?;

    let mut graph = Graph::default();
    for (i, raw_line) in contents.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        let mut obj: serde_json::Map<String, Value> =
            serde_json::from_str(line).map_err(|source| StoreError::Parse {
                path: path.display().to_string(),
                line: i + 1,
                source,
            })?;
        let item_type = obj.remove("type");
        let parse = |obj: serde_json::Map<String, Value>, line: usize| {
            (Value::Object(obj), line)
        };
        match item_type.as_ref().and_then(Value::as_str) {
            Some("entity") => {
                let (value, line_no) = parse(obj, i + 1);
                let entity: Entity =
                    serde_json::from_value(value).map_err(|source| StoreError::Parse {
                        path: path.display().to_string(),
                        line: line_no,
                        source,
                    })?;
                graph.entities.push(entity);
            }
            Some("relation") => {
                let (value, line_no) = parse(obj, i + 1);
                let relation: Relation =
                    serde_json::from_value(value).map_err(|source| StoreError::Parse {
                        path: path.display().to_string(),
                        line: line_no,
                        source,
                    })?;
                graph.relations.push(relation);
            }
            _ => {}
        }
    }
    Ok(graph)
}

/// Serialize the graph to JSONL, entities first then relations, one compact
/// JSON object per line — byte-compatible with Python's json.dumps output
/// except for separator spacing (Python uses ", "/": "; both parse alike).
pub fn serialize_graph(graph: &Graph) -> String {
    let mut out = String::new();
    for entity in &graph.entities {
        out.push_str(&python_style_json(&Line { r#type: "entity", item: entity }));
        out.push('\n');
    }
    for relation in &graph.relations {
        out.push_str(&python_style_json(&Line { r#type: "relation", item: relation }));
        out.push('\n');
    }
    out
}

/// json.dumps default separators: ", " between items, ": " after keys.
/// serde_json's pretty printer doesn't match, so use a custom formatter to
/// keep data files byte-identical with the Python implementation.
fn python_style_json<T: Serialize>(value: &T) -> String {
    let mut buf = Vec::new();
    let formatter = PythonFormatter;
    let mut ser = serde_json::Serializer::with_formatter(&mut buf, formatter);
    value.serialize(&mut ser).expect("graph serialization cannot fail");
    String::from_utf8(buf).expect("serde_json output is UTF-8")
}

/// Compact JSON with Python's default `", "` / `": "` separators and
/// `ensure_ascii=True` escaping (non-ASCII written as `\uXXXX`, surrogate
/// pairs for astral characters), so data files stay byte-identical with
/// what the Python implementation writes.
#[derive(Default)]
struct PythonFormatter;

impl serde_json::ser::Formatter for PythonFormatter {
    fn begin_object_value<W: ?Sized + Write>(&mut self, writer: &mut W) -> std::io::Result<()> {
        writer.write_all(b": ")
    }
    fn begin_array_value<W: ?Sized + Write>(
        &mut self,
        writer: &mut W,
        first: bool,
    ) -> std::io::Result<()> {
        if !first {
            writer.write_all(b", ")?;
        }
        Ok(())
    }
    fn begin_object_key<W: ?Sized + Write>(
        &mut self,
        writer: &mut W,
        first: bool,
    ) -> std::io::Result<()> {
        if !first {
            writer.write_all(b", ")?;
        }
        Ok(())
    }
    fn write_string_fragment<W: ?Sized + Write>(
        &mut self,
        writer: &mut W,
        fragment: &str,
    ) -> std::io::Result<()> {
        for c in fragment.chars() {
            if c.is_ascii() {
                writer.write_all(&[c as u8])?;
            } else {
                let mut buf = [0u16; 2];
                for unit in c.encode_utf16(&mut buf) {
                    write!(writer, "\\u{unit:04x}")?;
                }
            }
        }
        Ok(())
    }
}

/// Atomically persist the graph: write `<file>.tmp` in the same directory,
/// fsync, rename over the target. Readers never observe a partial file.
pub fn save_graph(path: &Path, graph: &Graph) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp_path = tmp_path_for(path);
    {
        let mut file = std::fs::File::create(&tmp_path)?;
        file.write_all(serialize_graph(graph).as_bytes())?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}

fn tmp_path_for(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    path.with_file_name(name)
}

/// One data file: in-memory graph behind a lock, loaded lazily.
pub struct Store {
    path: PathBuf,
    graph: RwLock<Graph>,
}

impl Store {
    fn open(path: PathBuf) -> Result<Self, StoreError> {
        let graph = load_graph(&path)?;
        Ok(Store { path, graph: RwLock::new(graph) })
    }

    /// Run a read-only closure against the in-memory graph.
    pub fn read<T>(&self, f: impl FnOnce(&Graph) -> T) -> T {
        let graph = self.graph.read().expect("graph lock poisoned");
        f(&graph)
    }

    /// Run a mutating closure, persisting atomically afterwards. If the
    /// closure errors, nothing is persisted and (by the closures' contract
    /// in graph.rs: validate first, mutate after) memory is unchanged.
    pub fn mutate<T, E: From<std::io::Error>>(
        &self,
        f: impl FnOnce(&mut Graph) -> Result<T, E>,
    ) -> Result<T, E> {
        let mut graph = self.graph.write().expect("graph lock poisoned");
        let result = f(&mut graph)?;
        save_graph(&self.path, &graph)?;
        Ok(result)
    }
}

/// Registry of stores keyed by logical file name (the `file` field in
/// tokens.json), mirroring Python's `_managers` cache.
pub struct StoreMap {
    data_dir: PathBuf,
    stores: Mutex<HashMap<String, Arc<Store>>>,
}

impl StoreMap {
    pub fn new(data_dir: PathBuf) -> Self {
        StoreMap { data_dir, stores: Mutex::new(HashMap::new()) }
    }

    pub fn get(&self, file: &str) -> Result<Arc<Store>, StoreError> {
        let mut stores = self.stores.lock().expect("store map lock poisoned");
        if let Some(store) = stores.get(file) {
            return Ok(store.clone());
        }
        let store = Arc::new(Store::open(self.data_dir.join(file))?);
        stores.insert(file.to_string(), store.clone());
        Ok(store)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap as Aliases;

    /// A v1 file (Python-era: observations as plain strings). Loading is
    /// supported forever; writing upgrades observations to fact objects.
    const V1_FILE: &str = concat!(
        "{\"type\": \"entity\", \"name\": \"Alice\", \"entityType\": \"Person\", \"observations\": [\"Likes coffee\", \"Lives in Boston\"], \"createdAt\": \"2026-01-01T00:00:00+00:00\", \"lastUpdated\": \"2026-01-02T00:00:00+00:00\"}\n",
        "{\"type\": \"entity\", \"name\": \"Legacy\", \"entityType\": \"Project\", \"observations\": [], \"createdAt\": null, \"lastUpdated\": null}\n",
        "{\"type\": \"relation\", \"from\": \"Alice\", \"to\": \"Legacy\", \"relationType\": \"worksOn\", \"createdAt\": \"2026-01-01T00:00:00+00:00\", \"lastUpdated\": \"2026-01-01T00:00:00+00:00\"}\n",
    );

    /// The same data in v2 form: each observation is `{"text": ...}` with no
    /// spurious temporal fields invented during the upgrade.
    const V2_FILE: &str = concat!(
        "{\"type\": \"entity\", \"name\": \"Alice\", \"entityType\": \"Person\", \"observations\": [{\"text\": \"Likes coffee\"}, {\"text\": \"Lives in Boston\"}], \"createdAt\": \"2026-01-01T00:00:00+00:00\", \"lastUpdated\": \"2026-01-02T00:00:00+00:00\"}\n",
        "{\"type\": \"entity\", \"name\": \"Legacy\", \"entityType\": \"Project\", \"observations\": [], \"createdAt\": null, \"lastUpdated\": null}\n",
        "{\"type\": \"relation\", \"from\": \"Alice\", \"to\": \"Legacy\", \"relationType\": \"worksOn\", \"createdAt\": \"2026-01-01T00:00:00+00:00\", \"lastUpdated\": \"2026-01-01T00:00:00+00:00\"}\n",
    );

    #[test]
    fn v1_file_upgrades_to_v2_on_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("g.jsonl");
        std::fs::write(&path, V1_FILE).unwrap();

        let graph = load_graph(&path).unwrap();
        assert_eq!(graph.entities.len(), 2);
        assert_eq!(graph.relations.len(), 1);
        assert_eq!(serialize_graph(&graph), V2_FILE);
    }

    #[test]
    fn v2_file_round_trips_byte_identically() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("g.jsonl");
        std::fs::write(&path, V2_FILE).unwrap();

        let graph = load_graph(&path).unwrap();
        assert_eq!(serialize_graph(&graph), V2_FILE);
    }

    #[test]
    fn temporal_fact_fields_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("g.jsonl");
        let line = "{\"type\": \"entity\", \"name\": \"A\", \"entityType\": \"T\", \"observations\": [{\"text\": \"target: April\", \"validFrom\": \"2026-03-01\", \"validTo\": \"2026-04-02\", \"recordedAt\": \"2026-03-01T10:00:00+00:00\", \"source\": \"1:1 notes\"}, {\"text\": \"mixed legacy\"}], \"createdAt\": null, \"lastUpdated\": null}\n";
        std::fs::write(&path, line).unwrap();

        let graph = load_graph(&path).unwrap();
        let fact = &graph.entities[0].observations_slice()[0];
        assert_eq!(fact.valid_from.as_deref(), Some("2026-03-01"));
        assert_eq!(fact.valid_to.as_deref(), Some("2026-04-02"));
        assert_eq!(fact.source.as_deref(), Some("1:1 notes"));
        assert_eq!(serialize_graph(&graph), line);
    }

    #[test]
    fn unknown_extra_fields_survive_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("g.jsonl");
        let line = "{\"type\": \"entity\", \"name\": \"X\", \"entityType\": \"T\", \"observations\": [], \"createdAt\": null, \"lastUpdated\": null, \"customField\": {\"a\": 1}}\n";
        std::fs::write(&path, line).unwrap();

        let graph = load_graph(&path).unwrap();
        assert_eq!(graph.entities[0].extra["customField"]["a"], 1);
        assert_eq!(serialize_graph(&graph), line);
    }

    #[test]
    fn missing_timestamps_become_null_on_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("g.jsonl");
        // Legacy line: no timestamps at all, no observations.
        std::fs::write(&path, "{\"type\": \"entity\", \"name\": \"Old\", \"entityType\": \"T\"}\n")
            .unwrap();

        let graph = load_graph(&path).unwrap();
        let out = serialize_graph(&graph);
        assert_eq!(
            out,
            "{\"type\": \"entity\", \"name\": \"Old\", \"entityType\": \"T\", \"createdAt\": null, \"lastUpdated\": null}\n"
        );
    }

    #[test]
    fn non_ascii_escaped_like_python_ensure_ascii() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("g.jsonl");
        // Exactly what Python json.dumps writes for "Café 日本 𝄞".
        let line = "{\"type\": \"entity\", \"name\": \"Caf\\u00e9 \\u65e5\\u672c \\ud834\\udd1e\", \"createdAt\": null, \"lastUpdated\": null}\n";
        std::fs::write(&path, line).unwrap();

        let graph = load_graph(&path).unwrap();
        assert_eq!(graph.entities[0].name, "Café 日本 𝄞");
        assert_eq!(serialize_graph(&graph), line);
    }

    #[test]
    fn blank_lines_and_unknown_types_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("g.jsonl");
        std::fs::write(
            &path,
            "\n{\"type\": \"banana\", \"name\": \"?\"}\n\n{\"type\": \"entity\", \"name\": \"A\", \"createdAt\": null, \"lastUpdated\": null}\n",
        )
        .unwrap();
        let graph = load_graph(&path).unwrap();
        assert_eq!(graph.entities.len(), 1);
    }

    #[test]
    fn missing_file_loads_empty_and_parse_error_reports_line() {
        let dir = tempfile::tempdir().unwrap();
        let graph = load_graph(&dir.path().join("nope.jsonl")).unwrap();
        assert!(graph.entities.is_empty());

        let path = dir.path().join("bad.jsonl");
        std::fs::write(&path, "{\"type\": \"entity\", \"name\": \"A\", \"createdAt\": null, \"lastUpdated\": null}\nnot json\n").unwrap();
        let err = load_graph(&path).unwrap_err();
        assert!(err.to_string().contains("line 2"), "got: {err}");
    }

    #[test]
    fn store_mutate_persists_atomically_and_read_sees_it() {
        let dir = tempfile::tempdir().unwrap();
        let stores = StoreMap::new(dir.path().to_path_buf());
        let store = stores.get("team.jsonl").unwrap();

        store
            .mutate(|g| -> Result<(), std::io::Error> {
                g.create_entities(
                    vec![crate::graph::Entity {
                        name: "A".into(),
                        entity_type: Some("person".into()),
                        observations: Some(vec![]),
                        created_at: None,
                        last_updated: None,
                        extra: Default::default(),
                    }],
                    &Aliases::new(),
                );
                Ok(())
            })
            .unwrap();

        assert_eq!(store.read(|g| g.entities.len()), 1);
        // No stray temp file, and the data is on disk.
        assert!(!dir.path().join("team.jsonl.tmp").exists());
        let reloaded = load_graph(&dir.path().join("team.jsonl")).unwrap();
        assert_eq!(reloaded.entities.len(), 1);
        assert_eq!(reloaded.entities[0].entity_type.as_deref(), Some("Person"));

        // Same logical file returns the same store instance.
        assert!(Arc::ptr_eq(&store, &stores.get("team.jsonl").unwrap()));
    }

    #[test]
    fn concurrent_mutations_do_not_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let stores = Arc::new(StoreMap::new(dir.path().to_path_buf()));
        let store = stores.get("c.jsonl").unwrap();

        let handles: Vec<_> = (0..40)
            .map(|i| {
                let store = store.clone();
                std::thread::spawn(move || {
                    store
                        .mutate(|g| -> Result<(), std::io::Error> {
                            g.create_entities(
                                vec![crate::graph::Entity {
                                    name: format!("Race {i}"),
                                    entity_type: Some("Person".into()),
                                    observations: Some(vec![crate::graph::Fact::from_text("x")]),
                                    created_at: None,
                                    last_updated: None,
                                    extra: Default::default(),
                                }],
                                &Aliases::new(),
                            );
                            Ok(())
                        })
                        .unwrap();
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(store.read(|g| g.entities.len()), 40);
        let reloaded = load_graph(&dir.path().join("c.jsonl")).unwrap();
        assert_eq!(reloaded.entities.len(), 40);
    }
}
