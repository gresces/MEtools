//! Read-side data access, resource mapping, and filters.

use serde::Deserialize;
use serde_json::{Map, Value, json};
use shared::DocumentStore;

use crate::json;

/// A readable data resource exposed by the HTTP API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resource {
    Profile,
    Actions,
    Health,
    Life,
    Goals,
    Projects,
}

impl Resource {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "profile" => Some(Self::Profile),
            "actions" => Some(Self::Actions),
            "health" => Some(Self::Health),
            "life" => Some(Self::Life),
            "goals" => Some(Self::Goals),
            "projects" => Some(Self::Projects),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Profile => "profile",
            Self::Actions => "actions",
            Self::Health => "health",
            Self::Life => "life",
            Self::Goals => "goals",
            Self::Projects => "projects",
        }
    }
}

/// A loaded JSON resource with its top-level version and timestamp.
#[derive(Debug, Clone)]
pub struct LoadedResource {
    pub value: Value,
    pub version: Option<u64>,
    pub updated_at: Option<String>,
}

/// A collection loaded in the domain JSON representation.
pub struct LoadedDocument {
    pub document: shared::Document,
    pub version: Option<u64>,
    pub updated_at: Option<String>,
}

impl LoadedDocument {
    /// Convert to the HTTP representation, optionally dropping receipts.
    fn into_resource(mut self, include_receipts: bool) -> LoadedResource {
        if !include_receipts {
            self.document.remove("recent_requests");
        }
        LoadedResource {
            value: json::into_serde(shared::JsonValue::Object(self.document)),
            version: self.version,
            updated_at: self.updated_at,
        }
    }
}

#[derive(Debug)]
pub enum DataError {
    NotFound(String),
    Invalid(String),
    Io(String),
}

impl DataError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotFound(_) => "NOT_FOUND",
            Self::Invalid(_) => "DATA_INVALID",
            Self::Io(_) => "DATA_INVALID",
        }
    }

    pub fn message(&self) -> &str {
        match self {
            Self::NotFound(message) | Self::Invalid(message) | Self::Io(message) => message,
        }
    }
}

/// Load one collection from the document store without converting its JSON.
pub fn load_document(
    store: &dyn DocumentStore,
    resource: Resource,
) -> Result<LoadedDocument, DataError> {
    let document = store.load(resource.name()).map_err(|failure| {
        if failure.code == "NOT_FOUND" {
            DataError::NotFound(format!(
                "{} was not found; initialize the database first",
                resource.name()
            ))
        } else {
            DataError::Invalid(failure.message)
        }
    })?;

    let version = document.get("version").and_then(shared::JsonValue::as_u64);
    let updated_at = document
        .get("updated_at")
        .and_then(shared::JsonValue::as_str)
        .map(str::to_string);
    Ok(LoadedDocument {
        document,
        version,
        updated_at,
    })
}

/// Load one resource from the document store.
pub fn load_resource(
    store: &dyn DocumentStore,
    resource: Resource,
) -> Result<LoadedResource, DataError> {
    load_document(store, resource).map(|loaded| loaded.into_resource(true))
}

/// Load one resource for a read response. Receipts are dropped before the
/// conversion unless the caller explicitly asks for them.
pub fn load_resource_for_read(
    store: &dyn DocumentStore,
    resource: Resource,
    include_receipts: bool,
) -> Result<LoadedResource, DataError> {
    load_document(store, resource).map(|loaded| loaded.into_resource(include_receipts))
}

/// Read collection metadata without returning the collection body.
///
/// When `id` is present, the record version at `path` (a dot-separated object
/// path, default `items`) is included. Used by clients that need an optimistic
/// concurrency version without downloading the whole collection.
pub fn resource_versions(
    store: &dyn DocumentStore,
    resource: Resource,
    path: Option<&str>,
    id: Option<&str>,
) -> Result<Value, DataError> {
    let loaded = load_document(store, resource)?;
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

    match (path, id) {
        (None, None) => {}
        (path, Some(id)) => {
            if id.is_empty() {
                return Err(DataError::Invalid("id must not be empty".to_string()));
            }
            let path = path.unwrap_or("items");
            if path.is_empty() || path.split('.').any(str::is_empty) {
                return Err(DataError::Invalid(
                    "path must be dot-separated object keys".to_string(),
                ));
            }
            let record = find_record(&loaded.document, path, id).ok_or_else(|| {
                DataError::NotFound(format!(
                    "record {id} was not found in {} at {path}",
                    resource.name()
                ))
            })?;
            let version = record
                .get("version")
                .and_then(shared::JsonValue::as_u64)
                .ok_or_else(|| DataError::Invalid(format!("record {id} has no integer version")))?;
            body.insert("record_id".to_string(), Value::String(id.to_string()));
            body.insert("record_version".to_string(), Value::from(version));
        }
        (Some(_), None) => {
            return Err(DataError::Invalid(
                "id is required when path is present".to_string(),
            ));
        }
    }

    Ok(Value::Object(body))
}

fn find_record<'a>(
    document: &'a shared::Document,
    path: &str,
    id: &str,
) -> Option<&'a shared::JsonValue> {
    let mut segments = path.split('.');
    let mut value = document.get(segments.next()?)?;
    for segment in segments {
        value = value.get(segment)?;
    }
    value
        .as_array()?
        .iter()
        .find(|record| record.get("id").and_then(shared::JsonValue::as_str) == Some(id))
}

/// Query parameters accepted by `GET /api/v1/data/actions`.
#[derive(Debug, Default, Deserialize)]
pub struct ActionFilter {
    pub status: Option<String>,
    pub tag: Option<String>,
    pub priority: Option<String>,
    #[serde(default)]
    pub include_archived: bool,
    pub limit: Option<usize>,
}

/// Apply action filters and return `(filtered document, total matching before limit)`.
pub fn filter_actions(mut value: Value, filter: &ActionFilter) -> (Value, usize) {
    let statuses: Option<Vec<String>> = filter.status.as_ref().map(|raw| {
        raw.split(',')
            .map(str::trim)
            .filter(|status| !status.is_empty())
            .map(str::to_string)
            .collect()
    });

    let limit = filter.limit.unwrap_or(200).clamp(1, 1000);
    let Some(items) = value.get_mut("items").and_then(Value::as_array_mut) else {
        return (value, 0);
    };

    let mut matching: Vec<Value> = std::mem::take(items)
        .into_iter()
        .filter(|item| {
            if !filter.include_archived
                && item
                    .get("archived_at")
                    .is_some_and(|archived| !archived.is_null())
            {
                return false;
            }
            if let Some(statuses) = &statuses
                && !statuses.is_empty()
            {
                let status = item.get("status").and_then(Value::as_str).unwrap_or("");
                if !statuses.iter().any(|wanted| wanted == status) {
                    return false;
                }
            }
            if let Some(priority) = &filter.priority {
                let current = item.get("priority").and_then(Value::as_str).unwrap_or("");
                if current != priority {
                    return false;
                }
            }
            if let Some(tag) = &filter.tag {
                let has_tag = item
                    .get("tags")
                    .and_then(Value::as_array)
                    .is_some_and(|tags| {
                        tags.iter()
                            .any(|value| value.as_str().is_some_and(|text| text == tag))
                    });
                if !has_tag {
                    return false;
                }
            }
            true
        })
        .collect();

    let total = matching.len();
    matching.truncate(limit);
    *items = matching;
    (value, total)
}

/// Read-only project summaries for the dashboard.
pub fn project_summaries(store: &dyn DocumentStore) -> Value {
    let document = store.load("projects").ok();
    let items = document
        .as_ref()
        .and_then(|document| document.get("items"))
        .and_then(shared::JsonValue::as_array);

    let mut summaries: Vec<Value> = items
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    let mut summary = Map::new();
                    if let Some(id) = item.get("id") {
                        summary.insert("id".to_string(), json::to_serde(id));
                    }
                    for field in ["title", "status", "updated_at", "next_action", "progress"] {
                        if let Some(value) = item.get(field) {
                            summary.insert(field.to_string(), json::to_serde(value));
                        }
                    }
                    Value::Object(summary)
                })
                .collect()
        })
        .unwrap_or_default();

    summaries.sort_by(|left, right| {
        let left = left.get("updated_at").and_then(Value::as_str).unwrap_or("");
        let right = right
            .get("updated_at")
            .and_then(Value::as_str)
            .unwrap_or("");
        right.cmp(left)
    });
    Value::Array(summaries)
}

/// Initialize all known collections. Existing documents are validated, not replaced.
pub fn init_all(store: &dyn DocumentStore) -> anyhow::Result<Vec<Value>> {
    let programs = [
        personal::program(),
        health::program(),
        life::program(),
        goal::program(),
    ];
    let mut results = Vec::new();
    for program in programs {
        let result = (program.init_handler)(store).map_err(|failure| {
            anyhow::anyhow!(
                "{} init failed: {} ({})",
                program.name,
                failure.message,
                failure.code
            )
        })?;
        results.push(json::to_serde(&result));
    }

    let projects = shared::JsonValue::object([
        ("version", shared::JsonValue::from(1_u64)),
        ("updated_at", shared::JsonValue::from(shared::now_iso8601())),
        ("updated_by", shared::JsonValue::from("init")),
        ("items", shared::JsonValue::Array(Vec::new())),
    ]);
    if let shared::JsonValue::Object(document) = projects {
        store
            .insert_initial("projects", &document)
            .map_err(|failure| anyhow::anyhow!("projects init failed: {}", failure.message))?;
        results.push(serde_json::json!({
            "status": "succeeded",
            "changed_records": ["projects"],
            "new_versions": {"projects": 1},
        }));
    }

    Ok(results)
}

fn validate_loaded(
    store: &dyn DocumentStore,
    resource: Resource,
    value: &Value,
) -> Result<(), String> {
    let document = json::to_shared(value).map_err(|message| message.to_string())?;
    let document = document
        .as_object()
        .cloned()
        .ok_or_else(|| format!("{} must be a JSON object", resource.name()))?;

    match resource {
        Resource::Actions => {
            personal::validate_envelope(&document).map_err(|failure| failure.message)?;
            let schema_version = document
                .get("schema_version")
                .and_then(shared::JsonValue::as_u64)
                .unwrap_or(0);
            if schema_version != personal::SCHEMA_VERSION {
                return Err(format!(
                    "actions must be migrated to schema_version={}",
                    personal::SCHEMA_VERSION
                ));
            }
            for item in document
                .get("items")
                .and_then(shared::JsonValue::as_array)
                .into_iter()
                .flatten()
            {
                let record = item
                    .as_object()
                    .ok_or_else(|| "action record must be an object".to_string())?;
                personal::validate_action(record).map_err(|failure| failure.message)?;
            }
            personal::validate_action_queue(&document).map_err(|failure| failure.message)?;
        }
        Resource::Health => {
            health::validate_document(&document).map_err(|failure| failure.message)?;
        }
        Resource::Life => {
            life::validate_document(&document).map_err(|failure| failure.message)?;
        }
        Resource::Goals => {
            let actions = store
                .load_optional("actions")
                .map_err(|failure| failure.message)?;
            goal::validate_document(&document, actions.as_ref())
                .map_err(|failure| failure.message)?;
        }
        Resource::Profile | Resource::Projects => {}
    }

    Ok(())
}

/// Validate every known collection without modifying it.
pub fn check_all(store: &dyn DocumentStore) -> Value {
    let mut checks = Vec::new();
    for resource in [
        Resource::Profile,
        Resource::Actions,
        Resource::Health,
        Resource::Life,
        Resource::Goals,
    ] {
        let status = match load_resource(store, resource) {
            Ok(loaded) => {
                if let Err(message) = validate_loaded(store, resource, &loaded.value) {
                    checks.push(json!({
                        "collection": resource.name(),
                        "ok": false,
                        "error": {"code": "DATA_INVALID", "message": message},
                    }));
                    continue;
                }
                let count = loaded
                    .value
                    .get("items")
                    .and_then(Value::as_array)
                    .map(|items| items.len());
                let mut check = json!({
                    "collection": resource.name(),
                    "ok": true,
                    "version": loaded.version,
                    "updated_at": loaded.updated_at,
                });
                if let (Some(count), Some(object)) = (count, check.as_object_mut()) {
                    object.insert("count".to_string(), json!(count));
                }
                check
            }
            Err(error) => json!({
                "collection": resource.name(),
                "ok": false,
                "error": {"code": error.code(), "message": error.message()},
            }),
        };
        checks.push(status);
    }

    let projects = project_summaries(store);
    checks.push(json!({
        "collection": "projects",
        "ok": store.load("projects").is_ok(),
        "count": projects.as_array().map_or(0, Vec::len),
    }));

    json!({"status": "succeeded", "checks": checks})
}

#[cfg(test)]
mod tests {
    use super::*;
    use me_store::{MemoryStore, SqliteStore};

    fn store() -> SqliteStore {
        SqliteStore::open_in_memory().expect("store")
    }

    #[test]
    fn load_resource_reads_the_collection_document() {
        let store = store();
        let document = shared::JsonValue::object([
            ("version", shared::JsonValue::from(3_u64)),
            (
                "updated_at",
                shared::JsonValue::from("2026-10-22T00:00:00+08:00"),
            ),
            ("items", shared::JsonValue::Array(Vec::new())),
        ]);
        store
            .insert_initial("actions", &document.as_object().cloned().unwrap())
            .expect("seed");

        let loaded = load_resource(&store, Resource::Actions).expect("load");
        assert_eq!(loaded.version, Some(3));
        assert_eq!(
            loaded.updated_at.as_deref(),
            Some("2026-10-22T00:00:00+08:00")
        );
        assert_eq!(loaded.value.get("version").and_then(Value::as_u64), Some(3));
    }

    #[test]
    fn missing_collection_reports_not_found() {
        let store = store();
        let error = load_resource(&store, Resource::Actions).expect_err("missing");
        assert_eq!(error.code(), "NOT_FOUND");
    }

    #[test]
    fn check_all_reports_every_collection_after_init() {
        let store = MemoryStore::new();
        init_all(&store).expect("init");
        let report = check_all(&store);
        let checks = report
            .get("checks")
            .and_then(Value::as_array)
            .expect("checks");
        assert_eq!(checks.len(), 6);
        assert!(
            checks
                .iter()
                .all(|check| check.get("ok") == Some(&Value::Bool(true)))
        );
    }

    #[test]
    fn check_all_flags_invalid_records() {
        let store = store();
        init_all(&store).expect("init");
        let mut actions = store.load("actions").expect("actions");
        actions.insert(
            "items".into(),
            shared::JsonValue::Array(vec![shared::JsonValue::object([(
                "id",
                shared::JsonValue::from("action-broken"),
            )])]),
        );
        store.replace("actions", &actions).expect("corrupt");

        let report = check_all(&store);
        let actions_check = report["checks"]
            .as_array()
            .expect("checks")
            .iter()
            .find(|check| check["collection"] == "actions")
            .expect("actions check");
        assert_eq!(actions_check["ok"], false);
        assert_eq!(actions_check["error"]["code"], "DATA_INVALID");
    }

    #[test]
    fn init_all_is_idempotent() {
        let store = store();
        init_all(&store).expect("first init");
        let versions = store
            .collections()
            .expect("collections")
            .into_iter()
            .map(|collection| {
                let version = store
                    .load(&collection)
                    .expect("load")
                    .get("version")
                    .and_then(shared::JsonValue::as_u64);
                (collection, version)
            })
            .collect::<Vec<_>>();

        init_all(&store).expect("second init");

        for (collection, version) in versions {
            assert_eq!(
                store
                    .load(&collection)
                    .expect("load")
                    .get("version")
                    .and_then(shared::JsonValue::as_u64),
                version,
                "{collection} version must not change on re-init"
            );
        }
    }
}
