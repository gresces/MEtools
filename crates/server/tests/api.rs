//! HTTP integration tests for me-server.

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::response::Response;
use http_body_util::BodyExt;
use me_server::config::Config;
use me_server::data::init_all;
use me_server::routes;
use me_server::state::AppState;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tower::ServiceExt;

const PASSWORD: &str = "test-password-1234567890";

struct TestDb {
    db_path: PathBuf,
    store: Arc<me_store::SqliteStore>,
}

impl TestDb {
    fn new(label: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let db_path = PathBuf::from("target")
            .join("test-tmp")
            .join(format!("me-server-{label}-{nanos}"))
            .join("me.db");
        let store = Arc::new(me_store::SqliteStore::open(&db_path).expect("open store"));
        init_all(store.as_ref()).expect("initialize test data");
        Self { db_path, store }
    }
}

impl Drop for TestDb {
    fn drop(&mut self) {
        if let Some(parent) = self.db_path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }
}

fn state(db: &TestDb) -> Arc<AppState> {
    let config = Config {
        bind: "127.0.0.1:0".parse().expect("valid bind"),
        db_path: db.db_path.clone(),
        password: Some(PASSWORD.to_string()),
        no_auth: false,
        allow_insecure: false,
        utc_offset_hours: 8,
        session_ttl_secs: 3600,
        secure_cookie: false,
    };
    Arc::new(AppState::new(config, Arc::clone(&db.store)))
}

const CHECKPOINT_ID: &str = "checkpoint-server-test";

fn seed_checkpoint(db: &TestDb) {
    let store = db.store.as_ref();
    let seeds: [(&str, &str, u64, Value); 3] = [
        (
            "goal.create",
            "seed-group",
            1,
            json!({
                "id": "goal-server-test",
                "group": true,
                "title": "Server test group",
                "success_criteria": ["seed"]
            }),
        ),
        (
            "milestone.create",
            "seed-milestone",
            2,
            json!({
                "id": "milestone-server-test",
                "goal_id": "goal-server-test",
                "title": "Server test milestone",
                "exit_criteria": ["seed"]
            }),
        ),
        (
            "checkpoint.create",
            "seed-checkpoint",
            3,
            json!({
                "id": CHECKPOINT_ID,
                "milestone_id": "milestone-server-test",
                "title": "Server test checkpoint"
            }),
        ),
    ];
    for (name, request_id, expected_version, input) in seeds {
        let command = goal::program()
            .commands
            .iter()
            .find(|command| command.name == name)
            .expect("goal command");
        let request = shared::Request {
            request_id: request_id.to_string(),
            expected_version,
            input: me_server::json::to_shared(&input)
                .expect("domain JSON")
                .as_object()
                .cloned()
                .expect("input object"),
        };
        (command.handler)(store, &request).expect("seed goal hierarchy");
    }
}

fn auth(request: axum::http::request::Builder) -> axum::http::request::Builder {
    request.header(header::AUTHORIZATION, format!("Bearer {PASSWORD}"))
}

async fn login_cookie(app: &axum::Router) -> String {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"password": PASSWORD}).to_string()))
                .expect("request"),
        )
        .await
        .expect("login");
    assert_eq!(response.status(), StatusCode::OK);
    response
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .expect("set-cookie")
        .split(';')
        .next()
        .expect("cookie pair")
        .to_string()
}

async fn body_json(response: Response) -> Value {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    serde_json::from_slice(&bytes).unwrap_or_else(|error| {
        panic!(
            "response is not JSON: {error}; body={}",
            String::from_utf8_lossy(&bytes)
        )
    })
}

async fn body_text(response: Response) -> String {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    String::from_utf8_lossy(&bytes).into_owned()
}

#[tokio::test]
async fn healthz_is_public_and_api_requires_auth() {
    let dir = TestDb::new("auth");
    let app = routes::app(state(&dir));

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("healthz");
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["status"], "ok");

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/programs")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("unauthorized");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body = body_json(response).await;
    assert_eq!(body["error"]["code"], "UNAUTHORIZED");

    let response = app
        .oneshot(
            auth(Request::builder())
                .uri("/api/v1/programs")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("authorized");
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    let programs = body["programs"].as_array().expect("programs array");
    assert!(programs.iter().any(|program| program["name"] == "personal"));
    assert!(programs.iter().any(|program| program["name"] == "goal"));
}

#[tokio::test]
async fn action_write_read_and_version_conflict() {
    let dir = TestDb::new("actions");
    seed_checkpoint(&dir);
    let app = routes::app(state(&dir));

    let create = json!({
        "request_id": "req-create",
        "expected_version": 1,
        "input": {
            "title": "Integration action",
            "checkpoint_id": CHECKPOINT_ID,
            "priority": "high",
            "tags": ["test"]
        }
    });
    let response = app
        .clone()
        .oneshot(
            auth(Request::builder())
                .method("POST")
                .uri("/api/v1/programs/personal/commands/action.create")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(create.to_string()))
                .expect("request"),
        )
        .await
        .expect("create");
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["status"], "succeeded");
    let action_id = body["changed_records"][0]
        .as_str()
        .expect("action id")
        .to_string();

    let response = app
        .clone()
        .oneshot(
            auth(Request::builder())
                .uri("/api/v1/data/actions?status=open&tag=test&priority=high")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("read");
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["version"], 2);
    assert_eq!(body["count"], 1);
    assert_eq!(body["data"]["items"][0]["id"], action_id);
    assert_eq!(body["data"]["items"][0]["duration_minutes"], 30);
    assert_eq!(body["data"]["items"][0]["buffer_minutes"], 30);
    assert!(body["data"].get("recent_requests").is_none());

    let response = app
        .clone()
        .oneshot(
            auth(Request::builder())
                .uri("/api/v1/data/actions?include_receipts=true")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("receipts");
    let body = body_json(response).await;
    assert!(body["data"]["recent_requests"].is_array());

    // Replaying the same request_id with the same content returns the cached
    // receipt even though the collection version has advanced.
    let replay = json!({
        "request_id": "req-create",
        "expected_version": 1,
        "input": {
            "title": "Integration action",
            "checkpoint_id": CHECKPOINT_ID,
            "priority": "high",
            "tags": ["test"]
        }
    });
    let response = app
        .clone()
        .oneshot(
            auth(Request::builder())
                .method("POST")
                .uri("/api/v1/programs/personal/commands/action.create")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(replay.to_string()))
                .expect("request"),
        )
        .await
        .expect("replay");
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["changed_records"][0], action_id);

    let stale = json!({
        "request_id": "req-stale",
        "expected_version": 1,
        "input": {"title": "Stale", "checkpoint_id": CHECKPOINT_ID}
    });
    let response = app
        .oneshot(
            auth(Request::builder())
                .method("POST")
                .uri("/api/v1/programs/personal/commands/action.create")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(stale.to_string()))
                .expect("request"),
        )
        .await
        .expect("conflict");
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = body_json(response).await;
    assert_eq!(body["error"]["code"], "VERSION_CONFLICT");
}

#[tokio::test]
async fn action_create_requires_existing_checkpoint_and_writes_link() {
    let dir = TestDb::new("action-parent");
    seed_checkpoint(&dir);
    let app = routes::app(state(&dir));

    let missing = json!({
        "request_id": "req-action-no-checkpoint",
        "expected_version": 1,
        "input": {"title": "Orphan"}
    });
    let response = app
        .clone()
        .oneshot(
            auth(Request::builder())
                .method("POST")
                .uri("/api/v1/programs/personal/commands/action.create")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(missing.to_string()))
                .expect("request"),
        )
        .await
        .expect("missing checkpoint id");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(body_json(response).await["error"]["code"], "INVALID_INPUT");

    let unknown = json!({
        "request_id": "req-action-missing-checkpoint",
        "expected_version": 1,
        "input": {"title": "Orphan", "checkpoint_id": "checkpoint-missing"}
    });
    let response = app
        .clone()
        .oneshot(
            auth(Request::builder())
                .method("POST")
                .uri("/api/v1/programs/personal/commands/action.create")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(unknown.to_string()))
                .expect("request"),
        )
        .await
        .expect("unknown checkpoint");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(body_json(response).await["error"]["code"], "NOT_FOUND");

    let create = json!({
        "request_id": "req-action-linked",
        "expected_version": 1,
        "input": {"title": "Linked", "checkpoint_id": CHECKPOINT_ID}
    });
    let response = app
        .clone()
        .oneshot(
            auth(Request::builder())
                .method("POST")
                .uri("/api/v1/programs/personal/commands/action.create")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(create.to_string()))
                .expect("request"),
        )
        .await
        .expect("create");
    assert_eq!(response.status(), StatusCode::OK);
    let action_id = body_json(response).await["changed_records"][0]
        .as_str()
        .expect("action id")
        .to_string();

    let response = app
        .oneshot(
            auth(Request::builder())
                .uri("/api/v1/data/goals")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("goals read");
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    let link = body["data"]["action_links"]
        .as_array()
        .and_then(|links| {
            links
                .iter()
                .find(|link| link["action_id"] == action_id.as_str())
        })
        .expect("action link");
    assert_eq!(link["goal_id"], "goal-server-test");
    assert_eq!(link["milestone_id"], "milestone-server-test");
    assert_eq!(link["checkpoint_id"], CHECKPOINT_ID);
}

#[tokio::test]
async fn version_probe_returns_collection_and_record_versions() {
    let dir = TestDb::new("versions");
    seed_checkpoint(&dir);
    let app = routes::app(state(&dir));

    let response = app
        .clone()
        .oneshot(
            auth(Request::builder())
                .uri("/api/v1/versions/actions")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("collection version");
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["version"], 1);
    assert!(
        body.get("data").is_none(),
        "version probe must not send data"
    );

    let create = json!({
        "request_id": "req-version-create",
        "expected_version": 1,
        "input": {"title": "Version probe action", "checkpoint_id": CHECKPOINT_ID}
    });
    let response = app
        .clone()
        .oneshot(
            auth(Request::builder())
                .method("POST")
                .uri("/api/v1/programs/personal/commands/action.create")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(create.to_string()))
                .expect("request"),
        )
        .await
        .expect("create");
    assert_eq!(response.status(), StatusCode::OK);
    let action_id = body_json(response).await["changed_records"][0]
        .as_str()
        .expect("action id")
        .to_string();

    let response = app
        .clone()
        .oneshot(
            auth(Request::builder())
                .uri(format!(
                    "/api/v1/versions/actions?path=items&id={action_id}"
                ))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("record version");
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["version"], 2);
    assert_eq!(body["record_id"], action_id);
    assert_eq!(body["record_version"], 1);

    let response = app
        .clone()
        .oneshot(
            auth(Request::builder())
                .uri("/api/v1/versions/actions?path=items&id=action-missing")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("missing record");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = body_json(response).await;
    assert_eq!(body["error"]["code"], "NOT_FOUND");

    let response = app
        .oneshot(
            auth(Request::builder())
                .uri("/api/v1/versions/actions?path=items")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("missing id");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = body_json(response).await;
    assert_eq!(body["error"]["code"], "INVALID_INPUT");
}

#[tokio::test]
async fn conditional_reads_return_304_and_new_etag_after_writes() {
    let dir = TestDb::new("etag");
    seed_checkpoint(&dir);
    let app = routes::app(state(&dir));

    let response = app
        .clone()
        .oneshot(
            auth(Request::builder())
                .uri("/api/v1/data/actions?include_receipts=true")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("read");
    assert_eq!(response.status(), StatusCode::OK);
    let etag = response
        .headers()
        .get(header::ETAG)
        .and_then(|value| value.to_str().ok())
        .expect("etag")
        .to_string();
    assert!(etag.starts_with("W/\""));
    let _ = body_json(response).await;

    let response = app
        .clone()
        .oneshot(
            auth(Request::builder())
                .uri("/api/v1/data/actions?include_receipts=true")
                .header(header::IF_NONE_MATCH, &etag)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("conditional read");
    assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
    assert!(
        response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes()
            .is_empty()
    );

    let create = json!({
        "request_id": "req-etag-create",
        "expected_version": 1,
        "input": {"title": "ETag action", "checkpoint_id": CHECKPOINT_ID}
    });
    let response = app
        .clone()
        .oneshot(
            auth(Request::builder())
                .method("POST")
                .uri("/api/v1/programs/personal/commands/action.create")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(create.to_string()))
                .expect("request"),
        )
        .await
        .expect("create");
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(
            auth(Request::builder())
                .uri("/api/v1/data/actions?include_receipts=true")
                .header(header::IF_NONE_MATCH, &etag)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("changed read");
    assert_eq!(response.status(), StatusCode::OK);
    let changed = response
        .headers()
        .get(header::ETAG)
        .and_then(|value| value.to_str().ok())
        .expect("new etag")
        .to_string();
    assert_ne!(changed, etag);

    let response = app
        .clone()
        .oneshot(
            auth(Request::builder())
                .uri("/api/v1/dashboard/summary")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("summary");
    assert_eq!(response.status(), StatusCode::OK);
    let summary_etag = response
        .headers()
        .get(header::ETAG)
        .and_then(|value| value.to_str().ok())
        .expect("summary etag")
        .to_string();

    let response = app
        .oneshot(
            auth(Request::builder())
                .uri("/api/v1/dashboard/summary")
                .header(header::IF_NONE_MATCH, &summary_etag)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("conditional summary");
    assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
}

#[tokio::test]
async fn gzip_compresses_dashboard_data() {
    let dir = TestDb::new("gzip");
    let app = routes::app(state(&dir));
    let response = app
        .oneshot(
            auth(Request::builder())
                .uri("/api/v1/dashboard/summary")
                .header(header::ACCEPT_ENCODING, "gzip")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("summary");
    assert_eq!(response.status(), StatusCode::OK);
    let encoding = response
        .headers()
        .get(header::CONTENT_ENCODING)
        .and_then(|value| value.to_str().ok());
    assert_eq!(encoding, Some("gzip"));
}

#[tokio::test]
async fn login_sets_cookie_that_authenticates_api() {
    let dir = TestDb::new("login");
    let app = routes::app(state(&dir));

    let login = json!({"password": PASSWORD});
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(login.to_string()))
                .expect("request"),
        )
        .await
        .expect("login");
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .expect("set-cookie")
        .split(';')
        .next()
        .expect("cookie pair")
        .to_string();
    assert!(cookie.starts_with("me_session="));

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/status")
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("cookie auth");
    assert_eq!(response.status(), StatusCode::OK);

    let bad = json!({"password": "wrong-password"});
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(bad.to_string()))
                .expect("request"),
        )
        .await
        .expect("bad login");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn dashboard_html_is_served() {
    let dir = TestDb::new("html");
    let app = routes::app(state(&dir));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("dashboard");
    assert_eq!(response.status(), StatusCode::OK);
    let text = body_text(response).await;
    assert!(text.contains("Personal OS"));
    assert!(text.contains("me-asset-version"));
    assert!(text.contains("/assets/dashboard.js?v="));
    assert!(text.contains("/assets/dashboard.css?v="));
}

#[tokio::test]
async fn dashboard_assets_are_served_without_auth() {
    let dir = TestDb::new("assets");
    let app = routes::app(state(&dir));

    let page = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("page");
    assert_eq!(page.status(), StatusCode::OK);
    let html = body_text(page).await;
    assert!(html.contains("/assets/dashboard.css"));
    assert!(html.contains("/assets/dashboard.js"));

    for (path, content_type) in [
        ("/assets/dashboard.css", "text/css"),
        ("/assets/dashboard.js", "text/javascript"),
        ("/assets/dashboard-core.js", "text/javascript"),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("asset");
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert!(
            response.headers()[header::CONTENT_TYPE]
                .to_str()
                .expect("content type")
                .contains(content_type),
            "{path}"
        );
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "public, max-age=31536000, immutable",
            "{path}"
        );
    }
}

#[tokio::test]
async fn cookie_writes_require_csrf_header() {
    let dir = TestDb::new("csrf");
    seed_checkpoint(&dir);
    let app = routes::app(state(&dir));
    let cookie = login_cookie(&app).await;
    let body = json!({
        "request_id": "csrf-1",
        "expected_version": 1,
        "input": {"title": "x", "checkpoint_id": CHECKPOINT_ID}
    })
    .to_string();

    let without = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/programs/personal/commands/action.create")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.clone()))
                .expect("request"),
        )
        .await
        .expect("without csrf");
    assert_eq!(without.status(), StatusCode::FORBIDDEN);

    let with = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/programs/personal/commands/action.create")
                .header(header::COOKIE, &cookie)
                .header("x-me-csrf", "1")
                .header(header::ORIGIN, "http://127.0.0.1:8787")
                .header(header::HOST, "127.0.0.1:8787")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .expect("request"),
        )
        .await
        .expect("with csrf");
    assert_eq!(with.status(), StatusCode::OK);

    let cross_origin = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/programs/personal/commands/action.create")
                .header(header::COOKIE, &cookie)
                .header("x-me-csrf", "1")
                .header(header::ORIGIN, "https://evil.example")
                .header(header::HOST, "127.0.0.1:8787")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "request_id": "csrf-evil",
                        "expected_version": 2,
                        "input": {"title": "evil", "checkpoint_id": CHECKPOINT_ID}
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("cross origin");
    assert_eq!(cross_origin.status(), StatusCode::FORBIDDEN);

    let bearer = app
        .oneshot(
            auth(Request::builder())
                .method("POST")
                .uri("/api/v1/programs/personal/commands/action.create")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "request_id": "csrf-bearer",
                        "expected_version": 2,
                        "input": {"title": "y", "checkpoint_id": CHECKPOINT_ID}
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("bearer");
    assert_eq!(bearer.status(), StatusCode::OK);
}

#[tokio::test]
async fn profile_update_persists_arbitrary_keys_and_summary_exposes_them() {
    let db = TestDb::new("profile-kv");
    let app = routes::app(state(&db));

    let response = app
        .clone()
        .oneshot(
            auth(Request::builder())
                .method("POST")
                .uri("/api/v1/programs/personal/commands/profile.update")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "request_id": "profile-kv-1",
                        "expected_version": 1,
                        "input": {
                            "patch": {
                                "occupation": "Engineer",
                                "skills": ["Rust", "SQLite"],
                                "age": 30,
                                "remote": true
                            }
                        }
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("profile update");
    assert_eq!(response.status(), StatusCode::OK);
    let update = body_json(response).await;
    assert_eq!(update["status"], "succeeded");

    let response = app
        .clone()
        .oneshot(
            auth(Request::builder())
                .uri("/api/v1/dashboard/summary")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("summary");
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["profile"]["occupation"], "Engineer");
    assert_eq!(body["profile"]["skills"], json!(["Rust", "SQLite"]));
    assert_eq!(body["profile"]["age"], 30);
    assert_eq!(body["profile"]["remote"], true);
    assert!(body["profile"].get("recent_requests").is_none());
}

#[tokio::test]
async fn status_reports_db_path_and_program_collections() {
    let db = TestDb::new("status");
    let app = routes::app(state(&db));

    let response = app
        .oneshot(
            auth(Request::builder())
                .uri("/api/v1/status")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("status");
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(
        body["db"].as_str(),
        Some(db.db_path.to_string_lossy().as_ref())
    );
    assert!(body.get("data_dir").is_none());
    let personal = body["programs"]
        .as_array()
        .expect("programs")
        .iter()
        .find(|program| program["name"] == "personal")
        .expect("personal program");
    assert_eq!(personal["collections"][0], "profile");
    assert!(personal.get("data_dir").is_none());
}

#[tokio::test]
async fn unknown_resource_and_program_return_not_found() {
    let db = TestDb::new("not-found");
    let app = routes::app(state(&db));

    let response = app
        .clone()
        .oneshot(
            auth(Request::builder())
                .uri("/api/v1/data/unknown")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("unknown resource");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(body_json(response).await["status"], "failed");

    let response = app
        .oneshot(
            auth(Request::builder())
                .uri("/api/v1/programs/unknown")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("unknown program");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn write_without_expected_version_returns_bad_request() {
    let db = TestDb::new("bad-request");
    let app = routes::app(state(&db));

    let response = app
        .oneshot(
            auth(Request::builder())
                .method("POST")
                .uri("/api/v1/programs/personal/commands/action.create")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"request_id": "req-missing-version", "input": {"title": "x"}})
                        .to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("write");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(body_json(response).await["error"]["code"], "INVALID_INPUT");
}

#[tokio::test]
async fn concurrent_http_writers_serialize_without_server_errors() {
    let db = TestDb::new("concurrent");
    seed_checkpoint(&db);
    let app = routes::app(state(&db));

    let mut handles = Vec::new();
    for worker in 0..6 {
        let app = app.clone();
        handles.push(tokio::spawn(async move {
            for attempt in 0..50 {
                let response = app
                    .clone()
                    .oneshot(
                        auth(Request::builder())
                            .uri("/api/v1/data/actions")
                            .body(Body::empty())
                            .expect("read"),
                    )
                    .await
                    .expect("read response");
                let version = body_json(response).await["version"]
                    .as_u64()
                    .expect("version");

                let response = app
                    .clone()
                    .oneshot(
                        auth(Request::builder())
                            .method("POST")
                            .uri("/api/v1/programs/personal/commands/action.create")
                            .header(header::CONTENT_TYPE, "application/json")
                            .body(Body::from(
                                json!({
                                    "request_id": format!("req-{worker}-{attempt}"),
                                    "expected_version": version,
                                    "input": {
                                        "title": format!("worker {worker} attempt {attempt}"),
                                        "checkpoint_id": CHECKPOINT_ID
                                    }
                                })
                                .to_string(),
                            ))
                            .expect("write"),
                    )
                    .await
                    .expect("write response");

                match response.status() {
                    StatusCode::OK => break,
                    StatusCode::CONFLICT => continue,
                    status => panic!("unexpected status {status} for worker {worker}"),
                }
            }
        }));
    }
    for handle in handles {
        handle.await.expect("worker");
    }

    let response = app
        .oneshot(
            auth(Request::builder())
                .uri("/api/v1/data/actions")
                .body(Body::empty())
                .expect("read"),
        )
        .await
        .expect("final read");
    let body = body_json(response).await;
    assert_eq!(body["count"], 6);
}
