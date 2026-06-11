//! HTML page handlers. Port of `webui/views.py`.

use super::auth::{
    clear_cookie_header, issue_cookie_header, safe_next, user_label,
};
use super::{WebUi, current_user_or_login_redirect, html, render};
use crate::graph::Entity;
use axum::extract::{Form, Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use minijinja::context;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::sync::Arc;

/// Entity as the templates expect it: JSON field names, observations always
/// present (a legacy entity without the key would otherwise break |length).
pub(super) fn ui_entity(e: &Entity) -> serde_json::Value {
    let mut v = serde_json::to_value(e).expect("entity serializes");
    let obj = v.as_object_mut().expect("entity is an object");
    obj.entry("observations")
        .or_insert_with(|| serde_json::Value::Array(vec![]));
    obj.entry("entityType")
        .or_insert_with(|| serde_json::Value::String(String::new()));
    v
}

/// Case-insensitive substring + exact-type filter. Port of `_filter_entities`.
fn filter_entities<'a>(
    entities: &'a [Entity],
    query: Option<&str>,
    type_filter: Option<&str>,
) -> Vec<&'a Entity> {
    let q = query.unwrap_or("").trim().to_lowercase();
    let t = type_filter.unwrap_or("").trim();
    entities
        .iter()
        .filter(|e| {
            if !t.is_empty() && e.entity_type_str() != t {
                return false;
            }
            if q.is_empty() {
                return true;
            }
            e.name.to_lowercase().contains(&q)
                || e.entity_type_str().to_lowercase().contains(&q)
                || e.observations_slice()
                    .iter()
                    .any(|f| f.text.to_lowercase().contains(&q))
        })
        .collect()
}

#[derive(Deserialize)]
pub(super) struct NextParam {
    next: Option<String>,
}

pub(super) async fn login_get(
    State(ui): State<Arc<WebUi>>,
    Query(params): Query<NextParam>,
) -> Response {
    let next_url = safe_next(params.next.as_deref());
    html(render(&ui, "login.html", context! { error => (), next_url => next_url }))
}

#[derive(Deserialize)]
pub(super) struct LoginForm {
    #[serde(default)]
    token: String,
}

pub(super) async fn login_post(
    State(ui): State<Arc<WebUi>>,
    Query(params): Query<NextParam>,
    headers: HeaderMap,
    Form(form): Form<LoginForm>,
) -> Response {
    let token = form.token.trim();
    let next_url = safe_next(params.next.as_deref());

    if ui.state.tokens.get(token).is_err() {
        return html(render(
            &ui,
            "login.html",
            context! { error => "Invalid token.", next_url => next_url },
        ));
    }

    // Match Python's `request.url.scheme == "https"`: trust the standard
    // proxy header since the server itself terminates plain HTTP.
    let secure = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("https"));

    (
        StatusCode::SEE_OTHER,
        [
            (header::LOCATION, next_url),
            (
                header::SET_COOKIE,
                issue_cookie_header(&ui.ui_secret, token, secure),
            ),
        ],
    )
        .into_response()
}

pub(super) async fn logout() -> Response {
    (
        StatusCode::SEE_OTHER,
        [
            (header::LOCATION, "/ui/login".to_string()),
            (header::SET_COOKIE, clear_cookie_header()),
        ],
    )
        .into_response()
}

pub(super) async fn entity_list(
    State(ui): State<Arc<WebUi>>,
    headers: HeaderMap,
) -> Response {
    let user = match current_user_or_login_redirect(&ui, &headers, "/ui/", None) {
        Ok(user) => user,
        Err(redirect) => return redirect,
    };
    let store = match ui.store_for(&user) {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    let (entities, distinct_types) = store.read(|g| {
        let mut entities: Vec<&Entity> = g.entities.iter().collect();
        entities.sort_by_key(|e| e.name.to_lowercase());
        let types: BTreeSet<String> = g
            .entities
            .iter()
            .filter(|e| !e.entity_type_str().is_empty())
            .map(|e| e.entity_type_str().to_string())
            .collect();
        (
            entities.iter().map(|e| ui_entity(e)).collect::<Vec<_>>(),
            types.into_iter().collect::<Vec<_>>(),
        )
    });
    html(render(
        &ui,
        "entity_list.html",
        context! {
            entities => entities,
            distinct_types => distinct_types,
            user_label => user_label(&user.token),
            read_only => user.entry.is_read_only(),
        },
    ))
}

#[derive(Deserialize)]
pub(super) struct SearchParams {
    q: Option<String>,
    #[serde(rename = "type")]
    type_filter: Option<String>,
}

/// htmx fragment endpoint: returns just the tbody rows.
pub(super) async fn entity_list_rows(
    State(ui): State<Arc<WebUi>>,
    Query(params): Query<SearchParams>,
    headers: HeaderMap,
) -> Response {
    let user = match current_user_or_login_redirect(&ui, &headers, "/ui/search", None) {
        Ok(user) => user,
        Err(redirect) => return redirect,
    };
    let store = match ui.store_for(&user) {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    let entities = store.read(|g| {
        let mut filtered =
            filter_entities(&g.entities, params.q.as_deref(), params.type_filter.as_deref());
        filtered.sort_by_key(|e| e.name.to_lowercase());
        filtered.iter().map(|e| ui_entity(e)).collect::<Vec<_>>()
    });
    html(render(&ui, "entity_list_rows.html", context! { entities => entities }))
}

pub(super) async fn entity_detail(
    State(ui): State<Arc<WebUi>>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Response {
    let path = format!(
        "/ui/entity/{}",
        utf8_percent_encode(&name, NON_ALPHANUMERIC)
    );
    let user = match current_user_or_login_redirect(&ui, &headers, &path, None) {
        Ok(user) => user,
        Err(redirect) => return redirect,
    };
    let store = match ui.store_for(&user) {
        Ok(s) => s,
        Err(resp) => return resp,
    };

    let found = store.read(|g| {
        let result = g.open_nodes(std::slice::from_ref(&name));
        let entity = result.entities.iter().find(|e| e.name == name).cloned();
        entity.map(|e| (e, result.relations))
    });
    let Some((mut entity, relations)) = found else {
        return StatusCode::NOT_FOUND.into_response();
    };
    // Display order: active facts first, superseded after (stable within
    // each group).
    if let Some(obs) = &mut entity.observations {
        obs.sort_by_key(|f| !f.is_active());
    }

    let outgoing: Vec<_> = relations
        .iter()
        .filter(|r| r.from == name)
        .map(|r| serde_json::to_value(r).unwrap())
        .collect();
    let incoming: Vec<_> = relations
        .iter()
        .filter(|r| r.to == name && r.from != name)
        .map(|r| serde_json::to_value(r).unwrap())
        .collect();

    html(render(
        &ui,
        "entity_detail.html",
        context! {
            entity => ui_entity(&entity),
            outgoing => outgoing,
            incoming => incoming,
            user_label => user_label(&user.token),
            read_only => user.entry.is_read_only(),
        },
    ))
}

pub const FULL_GRAPH_WARN_THRESHOLD: usize = 500;

#[derive(Deserialize)]
pub(super) struct GraphParams {
    center: Option<String>,
    depth: Option<String>,
    all: Option<String>,
    confirm: Option<String>,
}

/// Depth parsing, ported quirk included: a non-numeric depth falls back to 2
/// without clamping; a numeric one is clamped to 1..=10.
pub(super) fn parse_depth(raw: Option<&str>) -> i64 {
    match raw.unwrap_or("2").parse::<i64>() {
        Ok(d) => d.clamp(1, 10),
        Err(_) => 2,
    }
}

pub(super) async fn graph_page(
    State(ui): State<Arc<WebUi>>,
    Query(params): Query<GraphParams>,
    headers: HeaderMap,
) -> Response {
    let user = match current_user_or_login_redirect(&ui, &headers, "/ui/graph", None) {
        Ok(user) => user,
        Err(redirect) => return redirect,
    };
    let store = match ui.store_for(&user) {
        Ok(s) => s,
        Err(resp) => return resp,
    };

    let depth = parse_depth(params.depth.as_deref());
    let is_full = params.all.as_deref() == Some("1");
    let confirmed = params.confirm.as_deref() == Some("1");

    let (entity_count, relation_count, center_exists) = store.read(|g| {
        (
            g.entities.len(),
            g.relations.len(),
            params
                .center
                .as_ref()
                .map(|c| g.entities.iter().any(|e| &e.name == c)),
        )
    });

    let mut warn_large = false;
    let api_query;
    if let Some(center) = &params.center {
        if center_exists != Some(true) {
            return StatusCode::NOT_FOUND.into_response();
        }
        // Unlike Python, the center value is URL-encoded here: the template
        // injects api_query with |safe, and an unencoded value is a
        // reflected-XSS vector.
        api_query = format!(
            "center={}&depth={depth}",
            utf8_percent_encode(center, NON_ALPHANUMERIC)
        );
    } else {
        // Default and all=1 both render the full graph, warning when large.
        let _ = is_full;
        if entity_count > FULL_GRAPH_WARN_THRESHOLD && !confirmed {
            warn_large = true;
        }
        api_query = "all=1".to_string();
    }

    html(render(
        &ui,
        "graph.html",
        context! {
            center => params.center,
            depth => depth,
            entity_count => entity_count,
            relation_count => relation_count,
            warn_large => warn_large,
            api_query => api_query,
            user_label => user_label(&user.token),
            read_only => user.entry.is_read_only(),
        },
    ))
}
