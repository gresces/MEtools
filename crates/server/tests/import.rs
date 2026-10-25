//! Import and backup command tests.

use me_server::import::import_data_dir;
use me_store::{MemoryStore, SqliteStore};
use shared::{Document, DocumentStore, JsonValue};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

fn fixture_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let dir = PathBuf::from("target")
        .join("test-tmp")
        .join(format!("import-{label}-{nanos}"));
    std::fs::create_dir_all(&dir).expect("fixture dir");
    dir
}

fn write_json(path: &Path, value: serde_json::Value) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("parent");
    std::fs::write(path, value.to_string()).expect("write fixture");
}

#[test]
fn imports_v1_actions_and_goal_documents_from_data_dir() {
    let dir = fixture_dir("v1-actions");
    write_json(
        &dir.join("Data/personal/tasks.json"),
        serde_json::json!({
            "version": 1,
            "updated_at": "2026-10-12T20:00:00+08:00",
            "updated_by": "seed",
            "items": [
                {"id": "action-001", "title": "one", "status": "open", "version": 1,
                 "created_at": "2026-10-12T20:00:00+08:00", "updated_at": "2026-10-12T20:00:00+08:00"},
                {"id": "action-002", "title": "two", "status": "done", "version": 1,
                 "created_at": "2026-10-12T20:00:00+08:00", "updated_at": "2026-10-12T20:00:00+08:00"}
            ]
        }),
    );
    write_json(
        &dir.join("Data/goal/goals.json"),
        serde_json::json!({
            "schema_version": 1,
            "version": 1,
            "updated_at": "2026-10-12T20:00:00+08:00",
            "updated_by": "seed",
            "timezone": "Asia/Shanghai",
            "directions": [], "goals": [], "milestones": [], "action_links": [],
            "review": {"cadence": "weekly", "next_review_at": null}
        }),
    );

    let store = MemoryStore::new();
    let report = import_data_dir(&store, &dir, false).expect("import");

    assert!(!report.is_empty());
    let actions = store.load("actions").expect("actions");
    assert_eq!(
        actions.get("schema_version").and_then(JsonValue::as_u64),
        Some(7)
    );
    let first = actions
        .get("items")
        .and_then(JsonValue::as_array)
        .and_then(|items| items.first())
        .expect("imported action");
    assert_eq!(
        first.get("duration_minutes").and_then(JsonValue::as_u64),
        Some(30)
    );
    assert_eq!(
        first.get("buffer_minutes").and_then(JsonValue::as_u64),
        Some(30)
    );
    assert_eq!(
        actions
            .get("items")
            .and_then(JsonValue::as_array)
            .map(|items| items.len()),
        Some(2)
    );
    assert_eq!(
        actions
            .get("queue")
            .and_then(JsonValue::as_array)
            .map(|queue| queue
                .iter()
                .filter_map(JsonValue::as_str)
                .collect::<Vec<_>>()),
        Some(vec!["action-001"])
    );
    assert!(store.load_optional("goals").expect("goals").is_some());
}

#[test]
fn import_writes_nothing_when_any_collection_is_invalid() {
    let dir = fixture_dir("atomic");
    write_json(
        &dir.join("Data/personal/profile.json"),
        serde_json::json!({"version": 2, "display_name": "Test", "updated_at": "2026-10-12T20:00:00+08:00"}),
    );
    // v1 actions whose record is missing created_at/updated_at: migration fails validation.
    write_json(
        &dir.join("Data/personal/tasks.json"),
        serde_json::json!({
            "version": 1,
            "updated_at": "2026-10-12T20:00:00+08:00",
            "items": [{"id": "action-001", "title": "bad", "status": "open"}]
        }),
    );

    let store = MemoryStore::new();
    import_data_dir(&store, &dir, false).expect_err("invalid actions");

    assert!(
        store.collections().expect("collections").is_empty(),
        "a failed import must not leave partial data"
    );
}

#[test]
fn import_reads_project_directories_into_the_projects_collection() {
    let dir = fixture_dir("projects");
    write_json(
        &dir.join("Data/projects/alpha/project.json"),
        serde_json::json!({"title": "Alpha", "status": "active", "updated_at": "2026-10-12T20:00:00+08:00"}),
    );
    write_json(
        &dir.join("Data/projects/beta/project.json"),
        serde_json::json!({"title": "Beta", "status": "paused", "updated_at": "2026-10-11T20:00:00+08:00"}),
    );

    let store = MemoryStore::new();
    import_data_dir(&store, &dir, false).expect("import");

    let projects = store.load("projects").expect("projects");
    let items = projects
        .get("items")
        .and_then(JsonValue::as_array)
        .expect("items");
    assert_eq!(items.len(), 2);
    let ids: Vec<&str> = items
        .iter()
        .filter_map(|item| item.get("id").and_then(JsonValue::as_str))
        .collect();
    assert!(ids.contains(&"alpha"));
    assert!(ids.contains(&"beta"));
}

#[test]
fn import_refuses_to_overwrite_existing_collections_without_replace() {
    let dir = fixture_dir("replace");
    write_json(
        &dir.join("Data/personal/tasks.json"),
        serde_json::json!({
            "schema_version": 2, "version": 1,
            "updated_at": "2026-10-12T20:00:00+08:00", "updated_by": "seed",
            "items": []
        }),
    );
    let store = MemoryStore::new();
    let mut existing = Document::new();
    existing.insert("version".into(), JsonValue::from(9_u64));
    store.insert_initial("actions", &existing).expect("seed");

    let error = import_data_dir(&store, &dir, false).expect_err("refuse");
    assert!(error.to_string().contains("already exists"), "{error}");

    import_data_dir(&store, &dir, true).expect("replace");
    assert_eq!(
        store
            .load("actions")
            .unwrap()
            .get("version")
            .and_then(JsonValue::as_u64),
        Some(1)
    );
}

#[test]
fn backup_creates_openable_snapshot() {
    let dir = fixture_dir("backup");
    let store = SqliteStore::open(&dir.join("me.db")).expect("open");
    let mut doc = Document::new();
    doc.insert("version".into(), JsonValue::from(3_u64));
    store.insert_initial("actions", &doc).expect("insert");

    let target = dir.join("backup.db");
    store.backup_to(&target).expect("backup");
    assert!(store.backup_to(&target).is_err(), "existing target refused");
    drop(store);

    let restored = SqliteStore::open(&target).expect("reopen");
    assert_eq!(
        restored
            .load("actions")
            .unwrap()
            .get("version")
            .and_then(JsonValue::as_u64),
        Some(3)
    );
}
