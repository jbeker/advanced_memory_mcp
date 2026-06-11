//! MCP tool surface. Port of `server.py`.
//!
//! Tool names, argument names, descriptions, input schemas, response shapes,
//! and error message text are pinned by benchmarks/fixtures/python-v0.2.0/.
//! Like FastMCP, every failure — including authentication — surfaces as an
//! in-band tool error (`isError: true`) with text
//! `Error calling tool '<name>': <message>`, not a protocol-level error.
//!
//! Differences from Python, by design:
//! - No `MEMORY_TOKEN` env fallback: a Bearer Authorization header is required.
//! - Malformed item errors say e.g. "missing field `name`" instead of
//!   Python's bare KeyError text ("'name'").

use crate::graph::{Entity, Relation};
use crate::store::{Store, StoreMap};
use crate::tokens::{TokenConfig, TokenEntry};
use http::request::Parts;
use rmcp::handler::server::common::Extension;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::tool::ToolCallContext;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, Content, Implementation, JsonObject,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler, tool, tool_router};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;

pub const SERVER_NAME: &str = "Advanced Memory MCP";

pub struct AppState {
    pub stores: StoreMap,
    pub tokens: TokenConfig,
    pub aliases: HashMap<String, String>,
}

#[derive(Clone)]
pub struct MemoryServer {
    state: Arc<AppState>,
    tool_router: ToolRouter<Self>,
}

/// Internal operation error: a user-facing message string, or an I/O error
/// from persistence (required by `Store::mutate`'s bound).
#[derive(Debug)]
enum OpError {
    Msg(String),
    Io(std::io::Error),
}

impl std::fmt::Display for OpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpError::Msg(m) => f.write_str(m),
            OpError::Io(e) => write!(f, "failed to persist graph: {e}"),
        }
    }
}

impl From<std::io::Error> for OpError {
    fn from(e: std::io::Error) -> Self {
        OpError::Io(e)
    }
}

impl From<String> for OpError {
    fn from(m: String) -> Self {
        OpError::Msg(m)
    }
}

/// FastMCP-compatible in-band tool error.
fn tool_error(tool: &str, msg: impl std::fmt::Display) -> CallToolResult {
    CallToolResult::error(vec![Content::text(format!(
        "Error calling tool '{tool}': {msg}"
    ))])
}

/// Wrap a tool body: Ok(value) becomes a structured result (compact JSON
/// text + structuredContent, matching FastMCP), Err becomes a tool error.
fn run(tool: &str, f: impl FnOnce() -> Result<Value, OpError>) -> CallToolResult {
    match f() {
        Ok(value) => CallToolResult::structured(value),
        Err(e) => tool_error(tool, e),
    }
}

/// Deserialize a list of loose JSON objects into typed items, stripping any
/// "type" discriminator a client might include (Python would let it clobber
/// the line type on disk and silently lose the row; we refuse to store it).
fn parse_items<T: serde::de::DeserializeOwned>(items: Vec<Value>) -> Result<Vec<T>, OpError> {
    items
        .into_iter()
        .map(|mut v| {
            if let Some(obj) = v.as_object_mut() {
                obj.remove("type");
            }
            serde_json::from_value(v).map_err(|e| OpError::Msg(e.to_string()))
        })
        .collect()
}

// ---- input schemas, copied verbatim from the Python server's tools/list ----

fn objects_param_schema(param: &str) -> JsonObject {
    rmcp::model::object(json!({
        "additionalProperties": false,
        "properties": {param: {"items": {"additionalProperties": true, "type": "object"}, "type": "array"}},
        "required": [param],
        "type": "object",
    }))
}

fn strings_param_schema(param: &str) -> JsonObject {
    rmcp::model::object(json!({
        "additionalProperties": false,
        "properties": {param: {"items": {"type": "string"}, "type": "array"}},
        "required": [param],
        "type": "object",
    }))
}

fn two_strings_schema(a: &str, b: &str) -> JsonObject {
    rmcp::model::object(json!({
        "additionalProperties": false,
        "properties": {a: {"type": "string"}, b: {"type": "string"}},
        "required": [a, b],
        "type": "object",
    }))
}

fn one_string_schema(a: &str) -> JsonObject {
    rmcp::model::object(json!({
        "additionalProperties": false,
        "properties": {a: {"type": "string"}},
        "required": [a],
        "type": "object",
    }))
}

fn no_params_schema() -> JsonObject {
    rmcp::model::object(json!({
        "additionalProperties": false,
        "properties": {},
        "type": "object",
    }))
}

fn output_schema() -> Arc<JsonObject> {
    Arc::new(rmcp::model::object(json!({
        "additionalProperties": true,
        "type": "object",
    })))
}

// ---- typed parameter wrappers (deserialization only; schemas above) ----

#[derive(Deserialize)]
struct EntitiesParams {
    entities: Vec<Value>,
}

#[derive(Deserialize)]
struct RelationsParams {
    relations: Vec<Value>,
}

#[derive(Deserialize)]
struct ObservationsParams {
    observations: Vec<Value>,
}

#[derive(Deserialize)]
struct DeletionsParams {
    deletions: Vec<Value>,
}

#[derive(Deserialize)]
struct EntityNamesParams {
    #[serde(rename = "entityNames")]
    entity_names: Vec<String>,
}

#[derive(Deserialize)]
struct RenameParams {
    name: String,
    new_name: String,
}

#[derive(Deserialize)]
struct MergeParams {
    source: String,
    target: String,
}

#[derive(Deserialize)]
struct QueryParams {
    query: String,
}

#[derive(Deserialize)]
struct NamesParams {
    names: Vec<String>,
}

#[derive(Deserialize)]
struct ObservationInput {
    #[serde(rename = "entityName")]
    entity_name: String,
    contents: Vec<String>,
}

#[derive(Deserialize)]
struct DeletionInput {
    #[serde(rename = "entityName")]
    entity_name: String,
    observations: Vec<String>,
}

#[derive(Deserialize)]
struct RelationTriple {
    from: String,
    to: String,
    #[serde(rename = "relationType")]
    relation_type: String,
}

#[tool_router]
impl MemoryServer {
    pub fn new(state: Arc<AppState>) -> Self {
        MemoryServer { state, tool_router: Self::tool_router() }
    }

    /// Resolve the Bearer token from the Authorization header. Messages
    /// match Python's `_get_token`; there is no env-var fallback.
    fn auth(&self, parts: &Parts) -> Result<&TokenEntry, OpError> {
        let header = parts
            .headers
            .get(http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let token = if header.len() > 7 && header[..7].eq_ignore_ascii_case("bearer ") {
            &header[7..]
        } else {
            ""
        };
        if token.is_empty() {
            return Err(OpError::Msg(
                "No authentication token provided. Send a Bearer token in the Authorization header."
                    .to_string(),
            ));
        }
        self.state.tokens.get(token).map_err(OpError::Msg)
    }

    /// Authenticate and fetch the token's store; enforce write permission
    /// when `write` is set.
    fn authed_store(&self, parts: &Parts, write: bool) -> Result<Arc<Store>, OpError> {
        let entry = self.auth(parts)?;
        if write && entry.is_read_only() {
            return Err(OpError::Msg(
                "This token has read-only access. Write operations are not allowed.".to_string(),
            ));
        }
        self.state
            .stores
            .get(&entry.file)
            .map_err(|e| OpError::Msg(e.to_string()))
    }

    #[tool(
        description = "Create multiple new entities in the knowledge graph.\n\nEach entity should have 'name', 'entityType', and 'observations' fields.\nDeduplicates by entity name - existing entities are skipped.\nNew entities receive 'createdAt' and 'lastUpdated' ISO 8601 UTC timestamps.",
        input_schema = objects_param_schema("entities")
    )]
    async fn create_entities(
        &self,
        Parameters(p): Parameters<EntitiesParams>,
        Extension(parts): Extension<Parts>,
    ) -> CallToolResult {
        run("create_entities", || {
            let store = self.authed_store(&parts, true)?;
            let entities: Vec<Entity> = parse_items(p.entities)?;
            let created =
                store.mutate(|g| Ok::<_, OpError>(g.create_entities(entities, &self.state.aliases)))?;
            Ok(json!({"created": created}))
        })
    }

    #[tool(
        description = "Create multiple new relations between entities.\n\nEach relation should have 'from', 'to', and 'relationType' fields.\nDeduplicates by the (from, to, relationType) tuple.\nNew relations receive 'createdAt' and 'lastUpdated' ISO 8601 UTC timestamps.",
        input_schema = objects_param_schema("relations")
    )]
    async fn create_relations(
        &self,
        Parameters(p): Parameters<RelationsParams>,
        Extension(parts): Extension<Parts>,
    ) -> CallToolResult {
        run("create_relations", || {
            let store = self.authed_store(&parts, true)?;
            let relations: Vec<Relation> = parse_items(p.relations)?;
            let created = store.mutate(|g| Ok::<_, OpError>(g.create_relations(relations)))?;
            Ok(json!({"created": created}))
        })
    }

    #[tool(
        description = "Add new observations to existing entities.\n\nEach observation should have 'entityName' and 'contents' (list of strings).\nReturns error if entity doesn't exist. Deduplicates observations.",
        input_schema = objects_param_schema("observations")
    )]
    async fn add_observations(
        &self,
        Parameters(p): Parameters<ObservationsParams>,
        Extension(parts): Extension<Parts>,
    ) -> CallToolResult {
        run("add_observations", || {
            let store = self.authed_store(&parts, true)?;
            let inputs: Vec<ObservationInput> = parse_items(p.observations)?;
            let pairs = inputs
                .into_iter()
                .map(|o| (o.entity_name, o.contents))
                .collect();
            let results = store.mutate(|g| g.add_observations(pairs).map_err(OpError::Msg))?;
            Ok(json!({"results": results}))
        })
    }

    #[tool(
        description = "Delete entities and their associated relations from the knowledge graph.\n\nTakes a list of entity names to delete. Relations involving deleted entities\nare also removed (cascade delete).",
        input_schema = strings_param_schema("entityNames")
    )]
    async fn delete_entities(
        &self,
        Parameters(p): Parameters<EntityNamesParams>,
        Extension(parts): Extension<Parts>,
    ) -> CallToolResult {
        run("delete_entities", || {
            let store = self.authed_store(&parts, true)?;
            store.mutate(|g| {
                g.delete_entities(&p.entity_names);
                Ok::<_, OpError>(())
            })?;
            Ok(json!({"deleted": p.entity_names}))
        })
    }

    #[tool(
        description = "Delete specific observations from entities.\n\nEach deletion should have 'entityName' and 'observations' (list of strings to remove).\nSilently ignores missing entities or observations.",
        input_schema = objects_param_schema("deletions")
    )]
    async fn delete_observations(
        &self,
        Parameters(p): Parameters<DeletionsParams>,
        Extension(parts): Extension<Parts>,
    ) -> CallToolResult {
        run("delete_observations", || {
            let store = self.authed_store(&parts, true)?;
            // Echo the raw deletions back like Python does.
            let echo = p.deletions.clone();
            let inputs: Vec<DeletionInput> = parse_items(p.deletions)?;
            let pairs = inputs
                .into_iter()
                .map(|d| (d.entity_name, d.observations))
                .collect();
            store.mutate(|g| {
                g.delete_observations(pairs);
                Ok::<_, OpError>(())
            })?;
            Ok(json!({"deleted": echo}))
        })
    }

    #[tool(
        description = "Delete specific relations from the knowledge graph.\n\nEach relation should have 'from', 'to', and 'relationType' fields.\nAll three fields must match for deletion.",
        input_schema = objects_param_schema("relations")
    )]
    async fn delete_relations(
        &self,
        Parameters(p): Parameters<RelationsParams>,
        Extension(parts): Extension<Parts>,
    ) -> CallToolResult {
        run("delete_relations", || {
            let store = self.authed_store(&parts, true)?;
            let echo = p.relations.clone();
            let triples: Vec<RelationTriple> = parse_items(p.relations)?;
            let keys = triples
                .into_iter()
                .map(|t| (t.from, t.to, t.relation_type))
                .collect();
            store.mutate(|g| {
                g.delete_relations(keys);
                Ok::<_, OpError>(())
            })?;
            Ok(json!({"deleted": echo}))
        })
    }

    #[tool(
        description = "Rename an entity in the knowledge graph.\n\nUpdates the entity's name and rewrites every relation that references it\n(both 'from' and 'to' endpoints). Errors if the source entity is not found,\nif an entity with 'new_name' already exists (use merge_entities to combine\nthem), or if name == new_name. Bumps lastUpdated on the entity and on every\ntouched relation. Returns {renamed: {from, to}, relationsUpdated: N}.",
        input_schema = two_strings_schema("name", "new_name")
    )]
    async fn rename_entity(
        &self,
        Parameters(p): Parameters<RenameParams>,
        Extension(parts): Extension<Parts>,
    ) -> CallToolResult {
        run("rename_entity", || {
            let store = self.authed_store(&parts, true)?;
            let result =
                store.mutate(|g| g.rename_entity(&p.name, &p.new_name).map_err(OpError::Msg))?;
            Ok(result)
        })
    }

    #[tool(
        description = "Merge the source entity into the target entity.\n\nThe source is removed; the target absorbs the source's observations\n(unioned, target order preserved) and relations (re-pointed, self-loops\ndropped, duplicates collapsed keeping the earliest createdAt). Target's\nentityType is kept; if source's entityType differed it is returned as\n'discardedType' in the response. createdAt becomes the earliest non-null\nof the two. Errors if either entity is missing or source == target.",
        input_schema = two_strings_schema("source", "target")
    )]
    async fn merge_entities(
        &self,
        Parameters(p): Parameters<MergeParams>,
        Extension(parts): Extension<Parts>,
    ) -> CallToolResult {
        run("merge_entities", || {
            let store = self.authed_store(&parts, true)?;
            let result =
                store.mutate(|g| g.merge_entities(&p.source, &p.target).map_err(OpError::Msg))?;
            Ok(result)
        })
    }

    #[tool(
        description = "Read the entire knowledge graph.\n\nReturns all entities and relations for the authenticated user.\nEntities include 'createdAt' and 'lastUpdated' timestamps (null for legacy data).",
        input_schema = no_params_schema()
    )]
    async fn read_graph(&self, Extension(parts): Extension<Parts>) -> CallToolResult {
        run("read_graph", || {
            let store = self.authed_store(&parts, false)?;
            Ok(store.read(|g| serde_json::to_value(g).expect("graph serializes")))
        })
    }

    #[tool(
        description = "Search for nodes in the knowledge graph.\n\nPerforms case-insensitive search across entity names, types, and observations.\nReturns matching entities (with 'createdAt'/'lastUpdated' timestamps) and any\nrelations where at least one endpoint matches.",
        input_schema = one_string_schema("query")
    )]
    async fn search_nodes(
        &self,
        Parameters(p): Parameters<QueryParams>,
        Extension(parts): Extension<Parts>,
    ) -> CallToolResult {
        run("search_nodes", || {
            let store = self.authed_store(&parts, false)?;
            Ok(store.read(|g| {
                serde_json::to_value(g.search_nodes(&p.query)).expect("graph serializes")
            }))
        })
    }

    #[tool(
        description = "Open specific nodes by name from the knowledge graph.\n\nReturns the requested entities (with 'createdAt'/'lastUpdated' timestamps) and\nany relations where at least one endpoint is in the requested set.",
        input_schema = strings_param_schema("names")
    )]
    async fn open_nodes(
        &self,
        Parameters(p): Parameters<NamesParams>,
        Extension(parts): Extension<Parts>,
    ) -> CallToolResult {
        run("open_nodes", || {
            let store = self.authed_store(&parts, false)?;
            Ok(store.read(|g| {
                serde_json::to_value(g.open_nodes(&p.names)).expect("graph serializes")
            }))
        })
    }

    #[tool(
        description = "Normalize all entity types in the knowledge graph using configured aliases.\n\nApplies type alias mappings and Title Case normalization to all existing entities.\nReturns a summary of changes made. Read-only tokens are rejected.",
        input_schema = no_params_schema()
    )]
    async fn normalize_entity_types(&self, Extension(parts): Extension<Parts>) -> CallToolResult {
        run("normalize_entity_types", || {
            let store = self.authed_store(&parts, true)?;
            let changes = store.mutate(|g| {
                Ok::<_, OpError>(g.normalize_all_entity_types(&self.state.aliases))
            })?;
            let total = changes.len();
            Ok(json!({"changes": changes, "total": total}))
        })
    }
}

impl ServerHandler for MemoryServer {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.server_info = Implementation::from_build_env();
        info.server_info.name = SERVER_NAME.into();
        info.server_info.version = env!("CARGO_PKG_VERSION").into();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        // The Python server advertises a permissive outputSchema on every
        // tool; the #[tool] macro can't set one, so add it here.
        let mut tools: Vec<Tool> = self.tool_router.list_all();
        for tool in &mut tools {
            tool.output_schema = Some(output_schema());
        }
        Ok(ListToolsResult { tools, meta: None, next_cursor: None })
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tool_router.get(name).cloned()
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let tcc = ToolCallContext::new(self, request, context);
        self.tool_router.call(tcc).await
    }
}
