//! htmx mutation endpoints and JSON API. Port of `webui/api.py`.

use super::views::{parse_depth, ui_entity};
use super::{WebUi, current_user_or_401, html, render};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use minijinja::context;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashSet;
use std::sync::Arc;

fn forbidden_if_read_only(user: &super::auth::CurrentUser) -> Result<(), Response> {
    if user.entry.is_read_only() {
        Err((StatusCode::FORBIDDEN, Json(json!({"detail": "Read-only token."}))).into_response())
    } else {
        Ok(())
    }
}

#[derive(Deserialize)]
pub(super) struct GraphDataParams {
    center: Option<String>,
    depth: Option<String>,
    #[allow(dead_code)]
    all: Option<String>,
}

/// Graph JSON for cytoscape: full graph, or a BFS neighborhood of `center`
/// out to `depth` hops (clamped 1..=10).
pub(super) async fn graph_data(
    State(ui): State<Arc<WebUi>>,
    Query(params): Query<GraphDataParams>,
    headers: HeaderMap,
) -> Response {
    let user = match current_user_or_401(&ui, &headers) {
        Ok(user) => user,
        Err(resp) => return resp,
    };
    let store = match ui.store_for(&user) {
        Ok(s) => s,
        Err(resp) => return resp,
    };

    store.read(|g| {
        let (nodes, edges): (Vec<_>, Vec<_>) = if let Some(center) = &params.center {
            if !g.entities.iter().any(|e| &e.name == center) {
                return StatusCode::NOT_FOUND.into_response();
            }
            let depth = parse_depth(params.depth.as_deref());
            let mut included: HashSet<&str> = HashSet::from([center.as_str()]);
            let mut frontier: HashSet<&str> = included.clone();
            for _ in 0..depth {
                let mut next_frontier: HashSet<&str> = HashSet::new();
                for r in &g.relations {
                    if frontier.contains(r.from.as_str()) && !included.contains(r.to.as_str()) {
                        next_frontier.insert(r.to.as_str());
                    }
                    if frontier.contains(r.to.as_str()) && !included.contains(r.from.as_str()) {
                        next_frontier.insert(r.from.as_str());
                    }
                }
                included.extend(&next_frontier);
                frontier = next_frontier;
                if frontier.is_empty() {
                    break;
                }
            }
            (
                g.entities
                    .iter()
                    .filter(|e| included.contains(e.name.as_str()))
                    .collect(),
                g.relations
                    .iter()
                    .filter(|r| {
                        included.contains(r.from.as_str()) && included.contains(r.to.as_str())
                    })
                    .collect(),
            )
        } else {
            (g.entities.iter().collect(), g.relations.iter().collect())
        };

        Json(json!({
            "nodes": nodes.iter().map(|e| json!({
                "data": {"id": e.name, "type": e.entity_type_str()}
            })).collect::<Vec<_>>(),
            "edges": edges.iter().map(|r| json!({
                "data": {
                    "id": format!("{}|{}|{}", r.from, r.relation_type, r.to),
                    "source": r.from,
                    "target": r.to,
                    "label": r.relation_type,
                }
            })).collect::<Vec<_>>(),
            "count": nodes.len(),
        }))
        .into_response()
    })
}

#[derive(Deserialize)]
pub(super) struct UpdateObservation {
    entity: String,
    original_text: String,
    new_text: String,
}

pub(super) async fn update_observation(
    State(ui): State<Arc<WebUi>>,
    headers: HeaderMap,
    Json(payload): Json<UpdateObservation>,
) -> Response {
    let user = match current_user_or_401(&ui, &headers) {
        Ok(user) => user,
        Err(resp) => return resp,
    };
    if let Err(resp) = forbidden_if_read_only(&user) {
        return resp;
    }
    let store = match ui.store_for(&user) {
        Ok(s) => s,
        Err(resp) => return resp,
    };

    let ok = match store.mutate(|g| {
        Ok::<_, std::io::Error>(g.update_observation(
            &payload.entity,
            &payload.original_text,
            &payload.new_text,
        ))
    }) {
        Ok(ok) => ok,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    };
    if !ok {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"detail": "Entity or observation not found."})),
        )
            .into_response();
    }

    html(render(
        &ui,
        "observation_row.html",
        context! {
            obs => payload.new_text,
            entity => context! { name => payload.entity },
            read_only => false,
        },
    ))
}

#[derive(Deserialize)]
pub(super) struct DeleteObservation {
    entity: String,
    text: String,
}

pub(super) async fn delete_observation(
    State(ui): State<Arc<WebUi>>,
    headers: HeaderMap,
    Json(payload): Json<DeleteObservation>,
) -> Response {
    let user = match current_user_or_401(&ui, &headers) {
        Ok(user) => user,
        Err(resp) => return resp,
    };
    if let Err(resp) = forbidden_if_read_only(&user) {
        return resp;
    }
    let store = match ui.store_for(&user) {
        Ok(s) => s,
        Err(resp) => return resp,
    };

    if let Err(e) = store.mutate(|g| {
        g.delete_observations(vec![(payload.entity.clone(), vec![payload.text.clone()])]);
        Ok::<_, std::io::Error>(())
    }) {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    // htmx swaps the row out with the empty response.
    html(String::new())
}

pub(super) async fn delete_entity(
    State(ui): State<Arc<WebUi>>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Response {
    let user = match current_user_or_401(&ui, &headers) {
        Ok(user) => user,
        Err(resp) => return resp,
    };
    if let Err(resp) = forbidden_if_read_only(&user) {
        return resp;
    }
    let store = match ui.store_for(&user) {
        Ok(s) => s,
        Err(resp) => return resp,
    };

    let existed = store.read(|g| g.entities.iter().any(|e| e.name == name));
    if !existed {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"detail": "Entity not found."})),
        )
            .into_response();
    }
    if let Err(e) = store.mutate(|g| {
        g.delete_entities(std::slice::from_ref(&name));
        Ok::<_, std::io::Error>(())
    }) {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }

    let mut response = Json(json!({"deleted": name})).into_response();
    response
        .headers_mut()
        .insert("HX-Redirect", "/ui/".parse().expect("valid header"));
    response
}
