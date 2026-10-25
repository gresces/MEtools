//! HTTP routes for the data API, dashboard, and self-description endpoints.

use axum::{
    Json, Router,
    extract::{
        Path, Query, State,
        rejection::{JsonRejection, QueryRejection},
    },
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header},
    middleware,
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use shared::{DocumentStore, FunctionFailure, Request};
use std::hash::{Hash, Hasher};
use std::sync::OnceLock;
use tower_http::{
    compression::CompressionLayer, set_header::SetResponseHeaderLayer, trace::TraceLayer,
};
use uuid::Uuid;

use crate::auth;
use crate::dashboard;
use crate::data::{self, ActionFilter, DataError, Resource};
use crate::json;
use crate::state::SharedState;

const DASHBOARD_HTML: &str = include_str!("../assets/dashboard.html");
const DASHBOARD_CSS: &str = include_str!("../assets/dashboard.css");
const DASHBOARD_JS: &str = include_str!("../assets/dashboard.js");
const DASHBOARD_CORE_JS: &str = include_str!("../assets/dashboard-core.js");

/// Build the complete axum application.
pub fn app(state: SharedState) -> Router {
    let protected = Router::new()
        .route("/api/v1/auth/session", get(auth::session))
        .route("/api/v1/status", get(status))
        .route("/api/v1/programs", get(list_programs))
        .route("/api/v1/programs/{program}", get(describe_program))
        .route(
            "/api/v1/programs/{program}/commands/{command}",
            get(describe_command).post(write_command),
        )
        .route("/api/v1/data/{resource}", get(read_resource))
        .route("/api/v1/versions/{resource}", get(read_versions))
        .route("/api/v1/dashboard/summary", get(dashboard_summary))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_auth,
        ));

    Router::new()
        .route("/", get(dashboard_page))
        .route("/assets/dashboard.css", get(dashboard_css))
        .route("/assets/dashboard.js", get(dashboard_js))
        .route("/assets/dashboard-core.js", get(dashboard_core_js))
        .route("/healthz", get(healthz))
        .route("/favicon.ico", get(favicon))
        .route("/api/v1/auth/login", post(auth::login))
        .route("/api/v1/auth/logout", post(auth::logout))
        .merge(protected)
        .fallback(fallback)
        .layer(TraceLayer::new_for_http())
        .layer(SetResponseHeaderLayer::overriding(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        ))
        .layer(CompressionLayer::new())
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("content-security-policy"),
            HeaderValue::from_static(
                "default-src 'self'; style-src 'self' 'unsafe-inline'; script-src 'self'; img-src 'self' data:; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
            ),
        ))
        .with_state(state)
}

async fn favicon() -> Response {
    StatusCode::NO_CONTENT.into_response()
}

async fn healthz() -> Response {
    json_response(
        StatusCode::OK,
        json!({
            "status": "ok",
            "service": "me-server",
            "version": env!("CARGO_PKG_VERSION"),
        }),
    )
}

fn asset_version() -> &'static str {
    static VERSION: OnceLock<String> = OnceLock::new();
    VERSION
        .get_or_init(|| {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            DASHBOARD_HTML.hash(&mut hasher);
            DASHBOARD_CSS.hash(&mut hasher);
            DASHBOARD_JS.hash(&mut hasher);
            DASHBOARD_CORE_JS.hash(&mut hasher);
            format!("{:x}", hasher.finish())
        })
        .as_str()
}

fn dashboard_page_html() -> &'static str {
    static PAGE: OnceLock<String> = OnceLock::new();
    PAGE.get_or_init(|| {
        let version = asset_version();
        DASHBOARD_HTML
            .replace("{{ASSET_VERSION}}", version)
            .replace(
                "/assets/dashboard.css",
                &format!("/assets/dashboard.css?v={version}"),
            )
            .replace(
                "/assets/dashboard.js",
                &format!("/assets/dashboard.js?v={version}"),
            )
    })
    .as_str()
}

async fn dashboard_page() -> Response {
    (
        [
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            ),
        ],
        Html(dashboard_page_html()),
    )
        .into_response()
}

async fn dashboard_css() -> Response {
    static_asset(DASHBOARD_CSS, "text/css; charset=utf-8")
}

async fn dashboard_js() -> Response {
    static_asset(DASHBOARD_JS, "text/javascript; charset=utf-8")
}

async fn dashboard_core_js() -> Response {
    static_asset(DASHBOARD_CORE_JS, "text/javascript; charset=utf-8")
}

fn static_asset(body: &'static str, content_type: &'static str) -> Response {
    (
        [
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=31536000, immutable"),
            ),
            (header::CONTENT_TYPE, HeaderValue::from_static(content_type)),
        ],
        body,
    )
        .into_response()
}

async fn fallback(uri: axum::http::Uri) -> Response {
    if uri.path().starts_with("/api/") {
        error_response(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("no route for {}", uri.path()),
        )
    } else {
        dashboard_page().await
    }
}

async fn status(State(state): State<SharedState>) -> Response {
    let programs: Vec<Value> = state
        .programs
        .all()
        .iter()
        .map(|program| {
            json!({
                "name": program.name,
                "summary": program.summary,
                "collections": program.collections,
                "commands": program.commands.len(),
            })
        })
        .collect();

    json_response(
        StatusCode::OK,
        json!({
            "status": "succeeded",
            "server": "me-server",
            "version": env!("CARGO_PKG_VERSION"),
            "started_at": state.started_at.to_rfc3339(),
            "db": state.config.db_path.display().to_string(),
            "auth": if state.config.auth_required() { "password" } else { "disabled" },
            "timezone_offset_hours": state.config.utc_offset_hours,
            "programs": programs,
            "resources": ["profile", "actions", "health", "life", "goals", "projects"],
        }),
    )
}

async fn list_programs(State(state): State<SharedState>) -> Response {
    let programs: Vec<Value> = state
        .programs
        .all()
        .iter()
        .map(|program| {
            let commands: Vec<Value> = program
                .commands
                .iter()
                .map(|command| json!({"name": command.name, "summary": command.summary}))
                .collect();
            json!({
                "name": program.name,
                "summary": program.summary,
                "collections": program.collections,
                "commands": commands,
            })
        })
        .collect();

    json_response(
        StatusCode::OK,
        json!({"status": "succeeded", "programs": programs}),
    )
}

async fn describe_program(
    State(state): State<SharedState>,
    Path(program_name): Path<String>,
) -> Response {
    let Some(program) = state.programs.get(&program_name) else {
        return error_response(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("unknown program '{program_name}'"),
        );
    };
    let value = shared::build_describe_overview(program);
    json_response(StatusCode::OK, json::into_serde(value))
}

async fn describe_command(
    State(state): State<SharedState>,
    Path((program_name, command_name)): Path<(String, String)>,
) -> Response {
    let Some(program) = state.programs.get(&program_name) else {
        return error_response(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("unknown program '{program_name}'"),
        );
    };
    match shared::build_describe_command(program, &command_name) {
        Ok(value) => json_response(StatusCode::OK, json::into_serde(value)),
        Err(failure) => failure_response(&failure),
    }
}

async fn write_command(
    State(state): State<SharedState>,
    Path((program_name, command_name)): Path<(String, String)>,
    headers: HeaderMap,
    body: Result<Json<Value>, JsonRejection>,
) -> Response {
    if let Some(response) = require_csrf(&headers) {
        return response;
    }
    let Json(body) = match body {
        Ok(body) => body,
        Err(rejection) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "INVALID_INPUT",
                format!("invalid JSON request body: {}", rejection.body_text()),
            );
        }
    };

    let Some((_program, command)) = state.programs.command(&program_name, &command_name) else {
        return error_response(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("unknown command '{program_name}.{command_name}'"),
        );
    };

    let Value::Object(mut object) = body else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "INVALID_INPUT",
            "request body must be a JSON object",
        );
    };

    let request_id = object
        .get("request_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            headers
                .get("idempotency-key")
                .and_then(|value| value.to_str().ok())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
        .unwrap_or_else(|| format!("req-{}", Uuid::new_v4().simple()));

    let Some(expected_version) = object.get("expected_version").and_then(Value::as_u64) else {
        return error_response_with_request(
            StatusCode::BAD_REQUEST,
            "INVALID_INPUT",
            "expected_version is required and must be a non-negative integer",
            &request_id,
        );
    };

    let input_object = match object.remove("input") {
        Some(Value::Object(input)) => {
            let extra: Vec<&str> = object
                .keys()
                .map(String::as_str)
                .filter(|key| !matches!(*key, "request_id" | "expected_version"))
                .collect();
            if !extra.is_empty() {
                return error_response_with_request(
                    StatusCode::BAD_REQUEST,
                    "INVALID_INPUT",
                    format!(
                        "unexpected top-level fields when input is present: {}",
                        extra.join(", ")
                    ),
                    &request_id,
                );
            }
            input
        }
        Some(_) => {
            return error_response_with_request(
                StatusCode::BAD_REQUEST,
                "INVALID_INPUT",
                "input must be a JSON object",
                &request_id,
            );
        }
        None => {
            object.remove("request_id");
            object.remove("expected_version");
            object
        }
    };

    let shared_input = match json::into_shared(Value::Object(input_object)) {
        Ok(shared::JsonValue::Object(map)) => map,
        Ok(_) => {
            return error_response_with_request(
                StatusCode::BAD_REQUEST,
                "INVALID_INPUT",
                "input must be a JSON object",
                &request_id,
            );
        }
        Err(message) => {
            return error_response_with_request(
                StatusCode::BAD_REQUEST,
                "INVALID_INPUT",
                format!("input contains an unsupported value: {message}"),
                &request_id,
            );
        }
    };

    let request = Request {
        request_id: request_id.clone(),
        expected_version,
        input: shared_input,
    };
    let handler = command.handler;
    let store = std::sync::Arc::clone(&state.store);

    let result = match tokio::task::spawn_blocking(move || {
        let store: &dyn shared::DocumentStore = store.as_ref();
        handler(store, &request)
    })
    .await
    {
        Ok(result) => result,
        Err(error) => {
            return error_response_with_request(
                StatusCode::INTERNAL_SERVER_ERROR,
                "INTERNAL_ERROR",
                format!("command action failed: {error}"),
                &request_id,
            );
        }
    };

    match result {
        Ok(value) => {
            json_response_with_request(StatusCode::OK, json::into_serde(value), &request_id)
        }
        Err(mut failure) => {
            if failure.request_id.is_none() {
                failure.request_id = Some(request_id.clone());
            }
            failure_response_with_request(&failure, &request_id)
        }
    }
}

#[derive(Debug, Default, Deserialize, Hash)]
struct ReadQuery {
    status: Option<String>,
    tag: Option<String>,
    priority: Option<String>,
    #[serde(default)]
    include_archived: bool,
    #[serde(default)]
    include_receipts: bool,
    limit: Option<usize>,
}

fn require_csrf(headers: &HeaderMap) -> Option<Response> {
    let has_session_cookie = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .any(|part| part.trim().starts_with("me_session="));
    if !has_session_cookie {
        return None;
    }
    if headers
        .get("x-me-csrf")
        .and_then(|value| value.to_str().ok())
        != Some("1")
    {
        return Some(error_response(
            StatusCode::FORBIDDEN,
            "FORBIDDEN",
            "missing csrf header",
        ));
    }
    if let Some(origin) = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    {
        let host = headers
            .get(header::HOST)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        if !origin_matches_host(origin, host) {
            return Some(error_response(
                StatusCode::FORBIDDEN,
                "FORBIDDEN",
                "cross-origin write rejected",
            ));
        }
    }
    None
}

fn origin_matches_host(origin: &str, host: &str) -> bool {
    let Some((_scheme, authority)) = origin.split_once("://") else {
        return false;
    };
    let authority = authority.split('/').next().unwrap_or("");
    !host.is_empty() && authority.eq_ignore_ascii_case(host)
}

fn hashed_etag(parts: impl Hash) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    parts.hash(&mut hasher);
    format!("W/\"{:016x}\"", hasher.finish())
}

fn resource_etag(
    store: &dyn DocumentStore,
    resource: Resource,
    query: &ReadQuery,
) -> Result<Option<String>, FunctionFailure> {
    let Some(version) = store.version(resource.name())? else {
        return Ok(None);
    };
    Ok(Some(hashed_etag((resource.name(), version, query))))
}

fn summary_etag(
    store: &dyn DocumentStore,
    tz: chrono::FixedOffset,
) -> Result<String, FunctionFailure> {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    "dashboard.summary".hash(&mut hasher);
    for name in ["profile", "actions", "health", "life", "goals", "projects"] {
        store.version(name)?.unwrap_or(0).hash(&mut hasher);
    }
    let now = chrono::Utc::now().with_timezone(&tz);
    now.date_naive().hash(&mut hasher);
    (now.timestamp() / 3600).hash(&mut hasher);
    tz.local_minus_utc().hash(&mut hasher);
    Ok(format!("W/\"{:016x}\"", hasher.finish()))
}

fn request_not_modified(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value.split(',').any(|candidate| {
                let candidate = candidate.trim();
                candidate == "*" || candidate == etag
            })
        })
}

fn not_modified_response(etag: &str) -> Response {
    let mut response = StatusCode::NOT_MODIFIED.into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if let Ok(value) = HeaderValue::from_str(etag) {
        response.headers_mut().insert(header::ETAG, value);
    }
    response
}

fn json_response_with_etag(status: StatusCode, body: Value, etag: &str) -> Response {
    let mut response = json_response(status, body);
    if let Ok(value) = HeaderValue::from_str(etag) {
        response.headers_mut().insert(header::ETAG, value);
    }
    response
}

async fn read_resource(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(resource_name): Path<String>,
    query: Result<Query<ReadQuery>, QueryRejection>,
) -> Response {
    let Query(query) = match query {
        Ok(query) => query,
        Err(rejection) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "INVALID_INPUT",
                rejection.body_text(),
            );
        }
    };

    let Some(resource) = Resource::parse(&resource_name) else {
        return error_response(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("unknown resource '{resource_name}'"),
        );
    };

    let etag = resource_etag(state.store.as_ref(), resource, &query)
        .ok()
        .flatten();
    if let Some(etag) = &etag
        && request_not_modified(&headers, etag)
    {
        return not_modified_response(etag);
    }

    if resource == Resource::Projects {
        let store = std::sync::Arc::clone(&state.store);
        let projects = match tokio::task::spawn_blocking(move || {
            data::project_summaries(store.as_ref())
        })
        .await
        {
            Ok(projects) => projects,
            Err(error) => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "INTERNAL_ERROR",
                    format!("project summary failed: {error}"),
                );
            }
        };
        let count = projects.as_array().map_or(0, Vec::len);
        let body = json!({
            "status": "succeeded",
            "resource": "projects",
            "count": count,
            "data": projects,
        });
        return match etag {
            Some(etag) => json_response_with_etag(StatusCode::OK, body, &etag),
            None => json_response(StatusCode::OK, body),
        };
    }

    let store = std::sync::Arc::clone(&state.store);
    let include_receipts = query.include_receipts;
    let loaded = match tokio::task::spawn_blocking(move || {
        data::load_resource_for_read(store.as_ref(), resource, include_receipts)
    })
    .await
    {
        Ok(Ok(loaded)) => loaded,
        Ok(Err(error)) => return data_error_response(error),
        Err(error) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "INTERNAL_ERROR",
                format!("resource load failed: {error}"),
            );
        }
    };

    let mut value = loaded.value;
    let mut total = None;
    if resource == Resource::Actions {
        let filter = ActionFilter {
            status: query.status,
            tag: query.tag,
            priority: query.priority,
            include_archived: query.include_archived,
            limit: query.limit,
        };
        let (filtered, matching) = data::filter_actions(value, &filter);
        value = filtered;
        total = Some(matching);
    }

    let count = value
        .get("items")
        .and_then(Value::as_array)
        .map(Vec::len)
        .or_else(|| value.as_array().map(Vec::len));

    let mut body = Map::new();
    body.insert("status".to_string(), Value::String("succeeded".into()));
    body.insert(
        "resource".to_string(),
        Value::String(resource.name().into()),
    );
    body.insert(
        "version".to_string(),
        loaded.version.map(Value::from).unwrap_or(Value::Null),
    );
    body.insert(
        "updated_at".to_string(),
        loaded.updated_at.map(Value::String).unwrap_or(Value::Null),
    );
    if let Some(total) = total {
        body.insert("total".to_string(), Value::from(total));
    }
    if let Some(count) = count {
        body.insert("count".to_string(), Value::from(count));
    }
    body.insert("data".to_string(), value);

    match etag {
        Some(etag) => json_response_with_etag(StatusCode::OK, Value::Object(body), &etag),
        None => json_response(StatusCode::OK, Value::Object(body)),
    }
}

#[derive(Debug, Default, Deserialize)]
struct VersionQuery {
    path: Option<String>,
    id: Option<String>,
}

async fn read_versions(
    State(state): State<SharedState>,
    Path(resource_name): Path<String>,
    query: Result<Query<VersionQuery>, QueryRejection>,
) -> Response {
    let Query(query) = match query {
        Ok(query) => query,
        Err(rejection) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "INVALID_INPUT",
                rejection.body_text(),
            );
        }
    };
    let Some(resource) = Resource::parse(&resource_name) else {
        return error_response(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            format!("unknown resource '{resource_name}'"),
        );
    };

    let store = std::sync::Arc::clone(&state.store);
    let path = query.path;
    let id = query.id;
    match tokio::task::spawn_blocking(move || {
        data::resource_versions(store.as_ref(), resource, path.as_deref(), id.as_deref())
    })
    .await
    {
        Ok(Ok(value)) => json_response(StatusCode::OK, value),
        Ok(Err(DataError::NotFound(message))) => {
            error_response(StatusCode::NOT_FOUND, "NOT_FOUND", message)
        }
        Ok(Err(DataError::Invalid(message))) => {
            error_response(StatusCode::BAD_REQUEST, "INVALID_INPUT", message)
        }
        Ok(Err(DataError::Io(message))) => {
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "DATA_INVALID", message)
        }
        Err(error) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "INTERNAL_ERROR",
            format!("version lookup failed: {error}"),
        ),
    }
}

async fn dashboard_summary(State(state): State<SharedState>, headers: HeaderMap) -> Response {
    let etag = summary_etag(state.store.as_ref(), state.config.tz()).ok();
    if let Some(etag) = &etag
        && request_not_modified(&headers, etag)
    {
        return not_modified_response(etag);
    }

    let store = std::sync::Arc::clone(&state.store);
    let tz = state.config.tz();
    match tokio::task::spawn_blocking(move || dashboard::summary(store.as_ref(), tz)).await {
        Ok(summary) => match etag {
            Some(etag) => json_response_with_etag(StatusCode::OK, summary, &etag),
            None => json_response(StatusCode::OK, summary),
        },
        Err(error) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "INTERNAL_ERROR",
            format!("dashboard summary failed: {error}"),
        ),
    }
}

fn failure_response(failure: &FunctionFailure) -> Response {
    failure_response_with_request(failure, failure.request_id.as_deref().unwrap_or("unknown"))
}

fn failure_response_with_request(failure: &FunctionFailure, request_id: &str) -> Response {
    let body = shared::build_error_response(failure);
    let status = match failure.code.as_str() {
        "INVALID_INPUT" => StatusCode::BAD_REQUEST,
        "NOT_IMPLEMENTED" => StatusCode::NOT_IMPLEMENTED,
        "NOT_FOUND" => StatusCode::NOT_FOUND,
        "VERSION_CONFLICT" | "TIME_CONFLICT" => StatusCode::CONFLICT,
        "WRITE_BUSY" => StatusCode::LOCKED,
        "UNAUTHORIZED" | "FORBIDDEN" => StatusCode::UNAUTHORIZED,
        "DATA_INVALID" | "INTERNAL_ERROR" => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::BAD_REQUEST,
    };
    json_response_with_request(status, json::into_serde(body), request_id)
}

fn data_error_response(error: DataError) -> Response {
    let status = match error {
        DataError::NotFound(_) => StatusCode::NOT_FOUND,
        DataError::Invalid(_) | DataError::Io(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    error_response(status, error.code(), error.message())
}

fn error_response(status: StatusCode, code: &str, message: impl Into<String>) -> Response {
    error_response_with_request(status, code, message, "unknown")
}

fn error_response_with_request(
    status: StatusCode,
    code: &str,
    message: impl Into<String>,
    request_id: &str,
) -> Response {
    json_response_with_request(
        status,
        json!({
            "status": "failed",
            "request_id": request_id,
            "error": {"code": code, "message": message.into()},
        }),
        request_id,
    )
}

fn json_response(status: StatusCode, body: Value) -> Response {
    (
        status,
        [
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json; charset=utf-8"),
            ),
        ],
        Json(body),
    )
        .into_response()
}

fn json_response_with_request(status: StatusCode, body: Value, request_id: &str) -> Response {
    let mut response = (
        status,
        [
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json; charset=utf-8"),
            ),
        ],
        Json(body),
    )
        .into_response();
    if let Ok(value) = HeaderValue::from_str(request_id) {
        response.headers_mut().insert("x-me-request-id", value);
    }
    response
}
