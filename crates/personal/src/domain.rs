//! `personal` domain validation, persistence, and command handlers for
//! profile, the action queue (`actions`), and schema
//! migration.
//!
//! The `actions` collection holds the action records plus a document-level `queue`
//! array. The queue is the execution order of open, unarchived actions: index 0
//! is the current action, index 1 the next one, and so on.

use std::collections::{BTreeMap, BTreeSet};

use shared::{
    Document, DocumentStore, FunctionFailure, JsonValue, MutateOutcome, Request, check_fields,
    civil_from_days, days_from_civil, generate_id, now_iso8601, validate_instant,
    validate_iso_date,
};

pub const SCHEMA_VERSION: u64 = 7;
const DEFAULT_ACTION_MINUTES: u64 = 30;
const MAX_ACTION_MINUTES: u64 = 600;

const ACTION_FIELDS: &[&str] = &[
    "title",
    "notes",
    "status",
    "priority",
    "due_at",
    "due_date",
    "tags",
    "duration_minutes",
    "buffer_minutes",
    "recurrence",
    "recurrence_start_at",
    "recurrence_completed_count",
    "recurrence_last_completed_at",
];
const ACTION_PATCH_FIELDS: &[&str] = &[
    "title",
    "notes",
    "status",
    "priority",
    "due_at",
    "due_date",
    "tags",
    "duration_minutes",
    "buffer_minutes",
    "recurrence",
];
const ACTION_CREATE_ALLOWED: &[&str] = &[
    "title",
    "notes",
    "priority",
    "due_at",
    "due_date",
    "tags",
    "source",
    "duration_minutes",
    "buffer_minutes",
    "recurrence",
    "checkpoint_id",
];
const ACTION_STATUSES: &[&str] = &["open", "done", "cancelled"];
const ACTION_PRIORITIES: &[&str] = &["high", "normal", "low"];

const RECURRENCE_FREQUENCIES: &[&str] = &["daily", "weekly", "monthly", "yearly"];
const RECURRENCE_FIELDS: &[&str] = &["frequency", "interval", "count", "until", "by_weekday"];
const WEEKDAY_NAMES: &[&str] = &["mon", "tue", "wed", "thu", "fri", "sat", "sun"];
// A long-running daily series should still be expandable by the dashboard.
// 10,000 occurrences covers more than 27 years of daily events while keeping
// expansion bounded for malformed or unbounded rules.
const MAX_RECURRENCE_OCCURRENCES: usize = 10_000;

const PROFILE_RESERVED_FIELDS: &[&str] =
    &["version", "updated_at", "updated_by", "recent_requests"];
const PROFILE_RETAINED_FIELDS: &[&str] = &["display_name", "timezone"];
const PROFILE_MAX_KEY_CHARS: usize = 64;
const PROFILE_MAX_KEYS: usize = 256;
const PROFILE_MAX_VALUE_BYTES: usize = 16 * 1024;

// ---- common validators ------------------------------------------------------

fn require_non_empty_str<'a>(
    value: &'a JsonValue,
    field: &str,
) -> Result<&'a str, FunctionFailure> {
    value
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| FunctionFailure::invalid_input(format!("invalid {field}")))
}

fn require_string<'a>(value: &'a JsonValue, field: &str) -> Result<&'a str, FunctionFailure> {
    value
        .as_str()
        .ok_or_else(|| FunctionFailure::invalid_input(format!("invalid {field}")))
}

fn require_choice<'a>(
    value: &'a JsonValue,
    choices: &[&str],
    field: &str,
) -> Result<&'a str, FunctionFailure> {
    let s = value
        .as_str()
        .ok_or_else(|| FunctionFailure::invalid_input(format!("invalid {field}")))?;
    if !choices.contains(&s) {
        return Err(FunctionFailure::invalid_input(format!("invalid {field}")));
    }
    Ok(s)
}

fn validate_source(source: &JsonValue) -> Result<(), FunctionFailure> {
    let obj = source
        .as_object()
        .ok_or_else(|| FunctionFailure::invalid_input("source must be an object"))?;
    check_fields(obj, &["type", "reference"], &["type"])?;
    require_non_empty_str(obj.get("type").unwrap(), "source.type")?;
    if let Some(reference) = obj.get("reference") {
        require_non_empty_str(reference, "source.reference")?;
    }
    Ok(())
}

fn validate_metadata(record: &BTreeMap<String, JsonValue>) -> Result<(), FunctionFailure> {
    for key in [
        "id",
        "created_at",
        "updated_at",
        "source",
        "version",
        "archived_at",
    ] {
        if !record.contains_key(key) {
            return Err(FunctionFailure::invalid_input(format!(
                "record is missing {key}"
            )));
        }
    }
    require_non_empty_str(record.get("id").unwrap(), "id")?;
    let ver = record
        .get("version")
        .and_then(JsonValue::as_u64)
        .unwrap_or(0);
    if ver == 0 {
        return Err(FunctionFailure::invalid_input("invalid record version"));
    }

    let created_at = require_string(record.get("created_at").unwrap(), "created_at")?;
    validate_instant(created_at, "created_at")?;
    let updated_at = require_string(record.get("updated_at").unwrap(), "updated_at")?;
    validate_instant(updated_at, "updated_at")?;

    let archived_at = record.get("archived_at").unwrap();
    if !archived_at.is_null() {
        let s = require_string(archived_at, "archived_at")?;
        validate_instant(s, "archived_at")?;
    }

    validate_source(record.get("source").unwrap())?;
    Ok(())
}

pub fn validate_envelope(document: &BTreeMap<String, JsonValue>) -> Result<(), FunctionFailure> {
    let ver = document
        .get("version")
        .and_then(JsonValue::as_u64)
        .unwrap_or(0);
    if ver == 0 {
        return Err(FunctionFailure::new(
            "DATA_INVALID",
            "invalid collection version",
        ));
    }

    let items = document
        .get("items")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| FunctionFailure::new("DATA_INVALID", "items must be an array"))?;

    if !document
        .get("updated_at")
        .is_some_and(|v| v.as_str().is_some())
        || !document
            .get("updated_by")
            .is_some_and(|v| v.as_str().is_some())
    {
        return Err(FunctionFailure::new(
            "DATA_INVALID",
            "invalid collection metadata",
        ));
    }

    let mut identities = BTreeSet::new();
    for item in items {
        let obj = item
            .as_object()
            .ok_or_else(|| FunctionFailure::new("DATA_INVALID", "invalid record identity"))?;
        let id = obj
            .get("id")
            .and_then(JsonValue::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| FunctionFailure::new("DATA_INVALID", "invalid record identity"))?;
        if !identities.insert(id.to_string()) {
            return Err(FunctionFailure::new("DATA_INVALID", "duplicate record ID"));
        }
    }

    Ok(())
}

fn validate_records<F>(
    document: &BTreeMap<String, JsonValue>,
    validator: F,
) -> Result<(), FunctionFailure>
where
    F: Fn(&BTreeMap<String, JsonValue>) -> Result<(), FunctionFailure>,
{
    let items = document
        .get("items")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| FunctionFailure::new("DATA_INVALID", "items must be an array"))?;
    for item in items {
        let record = item
            .as_object()
            .ok_or_else(|| FunctionFailure::new("DATA_INVALID", "record must be an object"))?;
        let id = record
            .get("id")
            .and_then(JsonValue::as_str)
            .unwrap_or("unknown");
        if let Err(err) = validator(record) {
            return Err(FunctionFailure::new(
                "DATA_INVALID",
                format!("{id}: {}", err.message),
            ));
        }
    }
    Ok(())
}

fn check_version(
    record_or_doc: &BTreeMap<String, JsonValue>,
    expected_version: u64,
) -> Result<(), FunctionFailure> {
    let current = record_or_doc
        .get("version")
        .and_then(JsonValue::as_u64)
        .unwrap_or(0);
    if current != expected_version {
        return Err(FunctionFailure::version_conflict("version has changed"));
    }
    Ok(())
}

fn find_record_mut<'a>(
    document: &'a mut BTreeMap<String, JsonValue>,
    identity: &JsonValue,
) -> Result<&'a mut BTreeMap<String, JsonValue>, FunctionFailure> {
    let id_str = require_non_empty_str(identity, "record ID")?;
    let items = document
        .get_mut("items")
        .and_then(JsonValue::as_array_mut)
        .ok_or_else(|| FunctionFailure::new("DATA_INVALID", "items must be an array"))?;

    for item in items {
        let obj = item
            .as_object_mut()
            .ok_or_else(|| FunctionFailure::new("DATA_INVALID", "record must be an object"))?;
        if obj.get("id").and_then(JsonValue::as_str) == Some(id_str) {
            return Ok(obj);
        }
    }
    Err(FunctionFailure::not_found(format!(
        "record {id_str} was not found"
    )))
}

fn find_record<'a>(
    document: &'a BTreeMap<String, JsonValue>,
    identity: &str,
) -> Result<&'a BTreeMap<String, JsonValue>, FunctionFailure> {
    let items = document
        .get("items")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| FunctionFailure::new("DATA_INVALID", "items must be an array"))?;

    for item in items {
        let obj = item
            .as_object()
            .ok_or_else(|| FunctionFailure::new("DATA_INVALID", "record must be an object"))?;
        if obj.get("id").and_then(JsonValue::as_str) == Some(identity) {
            return Ok(obj);
        }
    }
    Err(FunctionFailure::not_found(format!(
        "record {identity} was not found"
    )))
}

fn new_record(
    prefix: &str,
    request_id: &str,
    seq: u64,
    defaults: BTreeMap<String, JsonValue>,
    payload: &BTreeMap<String, JsonValue>,
) -> BTreeMap<String, JsonValue> {
    let timestamp = now_iso8601();
    let mut merged = defaults;
    for (k, v) in payload {
        merged.insert(k.clone(), v.clone());
    }
    merged.insert(
        "id".to_string(),
        JsonValue::from(generate_id(prefix, request_id, seq)),
    );
    merged.insert("version".to_string(), JsonValue::from(1_u64));
    merged.insert("created_at".to_string(), JsonValue::from(timestamp.clone()));
    merged.insert("updated_at".to_string(), JsonValue::from(timestamp));
    merged.insert("archived_at".to_string(), JsonValue::Null);
    if !merged.contains_key("source") {
        merged.insert(
            "source".to_string(),
            JsonValue::object([("type", JsonValue::from("user_instruction"))]),
        );
    }
    merged
}

fn bump_document(document: &mut BTreeMap<String, JsonValue>, operation: &str) {
    let next = document
        .get("version")
        .and_then(JsonValue::as_u64)
        .unwrap_or(0)
        + 1;
    document.insert("version".to_string(), JsonValue::from(next));
    document.insert("updated_at".to_string(), JsonValue::from(now_iso8601()));
    document.insert("updated_by".to_string(), JsonValue::from(operation));
}

fn bump_record(record: &mut BTreeMap<String, JsonValue>, request: &Request) {
    let next_ver = record
        .get("version")
        .and_then(JsonValue::as_u64)
        .unwrap_or(1)
        + 1;
    record.insert("version".to_string(), JsonValue::from(next_ver));
    record.insert("updated_at".to_string(), JsonValue::from(now_iso8601()));
    record.insert(
        "last_request_id".to_string(),
        JsonValue::from(request.request_id.clone()),
    );
}

fn object_result(
    request: &Request,
    id: &str,
    operation: &str,
    new_versions: BTreeMap<String, JsonValue>,
) -> BTreeMap<String, JsonValue> {
    object_of_result([
        ("status", JsonValue::from("succeeded")),
        ("request_id", JsonValue::from(request.request_id.clone())),
        ("changed_records", JsonValue::string_array(&[id])),
        ("new_versions", JsonValue::Object(new_versions)),
        ("summary", JsonValue::from(format!("{operation}: {id}"))),
    ])
}

fn object_of_result<const N: usize>(
    entries: [(&str, JsonValue); N],
) -> BTreeMap<String, JsonValue> {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect()
}

fn mutation_result(
    request: &Request,
    record: &BTreeMap<String, JsonValue>,
    operation: &str,
) -> BTreeMap<String, JsonValue> {
    let id = record
        .get("id")
        .and_then(JsonValue::as_str)
        .unwrap_or("unknown");
    let ver = record
        .get("version")
        .and_then(JsonValue::as_u64)
        .unwrap_or(1);

    let JsonValue::Object(map) = JsonValue::object([
        ("status", JsonValue::from("succeeded")),
        ("request_id", JsonValue::from(request.request_id.clone())),
        ("changed_records", JsonValue::string_array(&[id])),
        (
            "new_versions",
            JsonValue::object([(id, JsonValue::from(ver))]),
        ),
        ("summary", JsonValue::from(format!("{operation}: {id}"))),
    ]) else {
        unreachable!()
    };
    map
}

fn run_collection_mutation<M, V>(
    store: &dyn DocumentStore,
    request: &Request,
    collection_name: &str,
    operation: &str,
    validator: V,
    mutate: M,
) -> Result<JsonValue, FunctionFailure>
where
    V: Fn(&BTreeMap<String, JsonValue>) -> Result<(), FunctionFailure>,
    M: Fn(
        &mut BTreeMap<String, JsonValue>,
        &Request,
    ) -> Result<BTreeMap<String, JsonValue>, FunctionFailure>,
{
    let result = store.transaction(&mut |transaction| {
        let outcome = transaction.mutate(collection_name, request, operation, &mut |document| {
            validate_envelope(document)?;
            let schema_version = document
                .get("schema_version")
                .and_then(JsonValue::as_u64)
                .unwrap_or(0);
            if schema_version != SCHEMA_VERSION {
                return Err(FunctionFailure::new(
                    "MIGRATION_REQUIRED",
                    format!("migrate {collection_name} to schema_version={SCHEMA_VERSION} first"),
                ));
            }
            validate_records(document, &validator)?;
            let result_map = mutate(document, request)?;
            Ok(JsonValue::Object(result_map))
        })?;

        Ok(match outcome {
            MutateOutcome::Replayed(value) => value,
            MutateOutcome::Applied { result, .. } => result,
        })
    })?;

    Ok(result)
}

// ---- Todo (actions.json) validation and mutations -----------------------------

fn require_minutes(value: &JsonValue, field: &str, min: u64) -> Result<u64, FunctionFailure> {
    value
        .as_u64()
        .filter(|minutes| (min..=MAX_ACTION_MINUTES).contains(minutes))
        .ok_or_else(|| {
            FunctionFailure::invalid_input(format!(
                "{field} must be an integer between {min} and {MAX_ACTION_MINUTES}"
            ))
        })
}

pub fn validate_action(action: &BTreeMap<String, JsonValue>) -> Result<(), FunctionFailure> {
    validate_metadata(action)?;
    for field in ACTION_FIELDS.iter().chain(std::iter::once(&"completed_at")) {
        if !action.contains_key(*field) {
            return Err(FunctionFailure::invalid_input("action fields are missing"));
        }
    }

    require_non_empty_str(action.get("title").unwrap(), "title")?;
    require_string(action.get("notes").unwrap(), "notes")?;
    let status = require_choice(action.get("status").unwrap(), ACTION_STATUSES, "status")?;
    require_choice(
        action.get("priority").unwrap(),
        ACTION_PRIORITIES,
        "priority",
    )?;

    let due_at = action.get("due_at").unwrap();
    let due_date = action.get("due_date").unwrap();
    if !due_at.is_null() && !due_date.is_null() {
        return Err(FunctionFailure::invalid_input(
            "due_at and due_date are mutually exclusive",
        ));
    }
    if !due_at.is_null() {
        let s = require_string(due_at, "due_at")?;
        validate_instant(s, "due_at")?;
    }
    if !due_date.is_null() {
        let s = require_string(due_date, "due_date")?;
        validate_iso_date(s, "due_date")?;
    }

    require_minutes(
        action.get("duration_minutes").unwrap(),
        "duration_minutes",
        1,
    )?;
    require_minutes(action.get("buffer_minutes").unwrap(), "buffer_minutes", 0)?;

    let recurrence = action.get("recurrence").unwrap();
    let recurrence_start = action.get("recurrence_start_at").unwrap();
    let recurrence_completed = action
        .get("recurrence_completed_count")
        .and_then(JsonValue::as_u64)
        .ok_or_else(|| {
            FunctionFailure::invalid_input(
                "recurrence_completed_count must be a non-negative integer",
            )
        })?;
    let recurrence_last = action.get("recurrence_last_completed_at").unwrap();
    if !recurrence_last.is_null() {
        validate_instant(
            require_string(recurrence_last, "recurrence_last_completed_at")?,
            "recurrence_last_completed_at",
        )?;
    }
    if recurrence.is_null() {
        if !recurrence_start.is_null() || recurrence_completed != 0 || !recurrence_last.is_null() {
            return Err(FunctionFailure::invalid_input(
                "non-recurring actions must not carry recurrence progress",
            ));
        }
    } else {
        let anchor = recurrence_start.as_str().ok_or_else(|| {
            FunctionFailure::invalid_input("recurring actions require recurrence_start_at")
        })?;
        if !due_at.is_null() {
            validate_instant(anchor, "recurrence_start_at")?;
        } else if !due_date.is_null() {
            validate_iso_date(anchor, "recurrence_start_at")?;
        } else {
            return Err(FunctionFailure::invalid_input(
                "recurring actions require due_at or due_date",
            ));
        }
        let first_date = anchor.get(..10).unwrap_or(anchor);
        let spec = parse_recurrence(recurrence, first_date)?
            .ok_or_else(|| FunctionFailure::invalid_input("recurrence must be an object"))?;
        if spec.count.is_some_and(|count| recurrence_completed > count) {
            return Err(FunctionFailure::invalid_input(
                "recurrence_completed_count exceeds recurrence.count",
            ));
        }
    }

    let tags = action
        .get("tags")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| FunctionFailure::invalid_input("tags must be an array"))?;
    for tag in tags {
        require_non_empty_str(tag, "tag")?;
    }

    let completed_at = action.get("completed_at").unwrap();
    if !completed_at.is_null() {
        let s = require_string(completed_at, "completed_at")?;
        validate_instant(s, "completed_at")?;
        if status != "done" {
            return Err(FunctionFailure::invalid_input(
                "only done actions can have completed_at",
            ));
        }
    }

    let archived_at = action.get("archived_at").unwrap();
    if !archived_at.is_null() && status != "done" && status != "cancelled" {
        return Err(FunctionFailure::new(
            "INVALID_TRANSITION",
            "only terminal actions can be archived",
        ));
    }

    Ok(())
}

/// Validate the document-level execution queue.
///
/// The queue must contain unique IDs of existing, open, unarchived actions.
pub fn validate_action_queue(
    document: &BTreeMap<String, JsonValue>,
) -> Result<(), FunctionFailure> {
    let queue = document
        .get("queue")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| FunctionFailure::new("DATA_INVALID", "queue must be an array"))?;
    let items = document
        .get("items")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| FunctionFailure::new("DATA_INVALID", "items must be an array"))?;

    let mut records: BTreeMap<&str, &BTreeMap<String, JsonValue>> = BTreeMap::new();
    for item in items {
        if let Some(record) = item.as_object()
            && let Some(id) = record.get("id").and_then(JsonValue::as_str)
        {
            records.insert(id, record);
        }
    }

    let mut seen = BTreeSet::new();
    for value in queue {
        let id = require_non_empty_str(value, "queue action id")?;
        if !seen.insert(id) {
            return Err(FunctionFailure::new(
                "DATA_INVALID",
                format!("queue contains duplicate action {id}"),
            ));
        }
        let Some(record) = records.get(id) else {
            return Err(FunctionFailure::new(
                "DATA_INVALID",
                format!("queue references missing action {id}"),
            ));
        };
        let open = record.get("status").and_then(JsonValue::as_str) == Some("open");
        let archived = record
            .get("archived_at")
            .is_some_and(|value| !value.is_null());
        if !open || archived {
            return Err(FunctionFailure::new(
                "DATA_INVALID",
                format!("queue may only contain open, unarchived actions: {id}"),
            ));
        }
    }
    Ok(())
}

/// Validate the complete `actions` document: envelope, schema, records, and queue.
pub fn validate_action_document(
    document: &BTreeMap<String, JsonValue>,
) -> Result<(), FunctionFailure> {
    validate_envelope(document)?;
    let schema_version = document
        .get("schema_version")
        .and_then(JsonValue::as_u64)
        .unwrap_or(0);
    if schema_version != SCHEMA_VERSION {
        return Err(FunctionFailure::new(
            "MIGRATION_REQUIRED",
            format!("migrate actions to schema_version={SCHEMA_VERSION} first"),
        ));
    }
    validate_records(document, validate_action)?;
    validate_action_queue(document)
}

fn queue_items_mut(
    document: &mut BTreeMap<String, JsonValue>,
) -> Result<&mut Vec<JsonValue>, FunctionFailure> {
    if !document.contains_key("queue") {
        document.insert("queue".to_string(), JsonValue::Array(Vec::new()));
    }
    document
        .get_mut("queue")
        .and_then(JsonValue::as_array_mut)
        .ok_or_else(|| FunctionFailure::new("DATA_INVALID", "queue must be an array"))
}

fn ensure_queued(
    document: &mut BTreeMap<String, JsonValue>,
    action_id: &str,
) -> Result<(), FunctionFailure> {
    let queue = queue_items_mut(document)?;
    if !queue.iter().any(|value| value.as_str() == Some(action_id)) {
        queue.push(JsonValue::from(action_id));
    }
    Ok(())
}

fn remove_from_queue(
    document: &mut BTreeMap<String, JsonValue>,
    action_id: &str,
) -> Result<(), FunctionFailure> {
    let queue = queue_items_mut(document)?;
    queue.retain(|value| value.as_str() != Some(action_id));
    Ok(())
}

fn priority_rank(value: Option<&str>) -> u8 {
    match value {
        Some("high") => 0,
        Some("low") => 2,
        _ => 1,
    }
}

fn document_item_ids(document: &Document) -> BTreeSet<String> {
    document
        .get("items")
        .and_then(JsonValue::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("id").and_then(JsonValue::as_str))
        .map(str::to_string)
        .collect()
}

/// Build a queue for documents migrated from schema v1/v2: open actions sorted by
/// due date, priority, then creation time.
fn derived_queue(document: &BTreeMap<String, JsonValue>) -> JsonValue {
    let mut open: Vec<(String, Option<String>, u8, String)> = Vec::new();
    if let Some(items) = document.get("items").and_then(JsonValue::as_array) {
        for item in items {
            let Some(record) = item.as_object() else {
                continue;
            };
            if record.get("status").and_then(JsonValue::as_str) != Some("open") {
                continue;
            }
            if record
                .get("archived_at")
                .is_some_and(|value| !value.is_null())
            {
                continue;
            }
            let Some(id) = record.get("id").and_then(JsonValue::as_str) else {
                continue;
            };
            let due = record
                .get("due_at")
                .and_then(JsonValue::as_str)
                .or_else(|| record.get("due_date").and_then(JsonValue::as_str))
                .map(str::to_string);
            let priority = priority_rank(record.get("priority").and_then(JsonValue::as_str));
            let created = record
                .get("created_at")
                .and_then(JsonValue::as_str)
                .unwrap_or("")
                .to_string();
            open.push((id.to_string(), due, priority, created));
        }
    }
    // Dated actions first (earliest due first), undated actions last.
    open.sort_by(|a, b| {
        a.1.is_none()
            .cmp(&b.1.is_none())
            .then_with(|| a.1.cmp(&b.1))
            .then(a.2.cmp(&b.2))
            .then(a.3.cmp(&b.3))
    });
    JsonValue::Array(
        open.into_iter()
            .map(|(id, ..)| JsonValue::from(id))
            .collect(),
    )
}

fn mutate_action(
    document: &mut BTreeMap<String, JsonValue>,
    request: &Request,
    operation: &str,
) -> Result<BTreeMap<String, JsonValue>, FunctionFailure> {
    let payload = &request.input;
    let required: &[&str] = if operation == "update_action" {
        &["action_id", "patch"]
    } else {
        &["action_id", "archived"]
    };
    check_fields(payload, required, required)?;

    let action_id_val = payload.get("action_id").unwrap();
    let target = find_record_mut(document, action_id_val)?;
    validate_action(target)?;
    check_version(target, request.expected_version)?;

    let action_id = target
        .get("id")
        .and_then(JsonValue::as_str)
        .unwrap()
        .to_string();
    if operation == "archive_action" {
        let archived = payload
            .get("archived")
            .and_then(JsonValue::as_bool)
            .ok_or_else(|| FunctionFailure::invalid_input("archived must be a boolean"))?;
        target.insert(
            "archived_at".to_string(),
            if archived {
                JsonValue::from(now_iso8601())
            } else {
                JsonValue::Null
            },
        );
    } else {
        if target.get("archived_at").is_some_and(|v| !v.is_null()) {
            return Err(FunctionFailure::new(
                "INVALID_TRANSITION",
                "restore action before updating",
            ));
        }
        let patch = payload
            .get("patch")
            .and_then(JsonValue::as_object)
            .filter(|m| !m.is_empty())
            .ok_or_else(|| FunctionFailure::invalid_input("patch must be a non-empty object"))?;
        check_fields(patch, ACTION_PATCH_FIELDS, &[])?;

        let previous_status = target
            .get("status")
            .and_then(JsonValue::as_str)
            .unwrap_or("open")
            .to_string();
        let previous_duration = target.get("duration_minutes").and_then(JsonValue::as_u64);
        let previous_buffer = target.get("buffer_minutes").and_then(JsonValue::as_u64);
        let previous_recurrence = target.get("recurrence").cloned().unwrap_or(JsonValue::Null);
        let previous_completed = target
            .get("recurrence_completed_count")
            .and_then(JsonValue::as_u64)
            .unwrap_or(0);
        let due_patched = patch.contains_key("due_at") || patch.contains_key("due_date");
        let recurrence_patched = patch.contains_key("recurrence");
        if due_patched
            && !recurrence_patched
            && !previous_recurrence.is_null()
            && previous_completed > 0
        {
            return Err(FunctionFailure::new(
                "INVALID_TRANSITION",
                "cannot reschedule a started recurring action; edit the recurrence or finish the series first",
            ));
        }

        for (k, v) in patch {
            target.insert(k.clone(), v.clone());
        }

        if recurrence_patched {
            let recurrence = target.get("recurrence").cloned().unwrap_or(JsonValue::Null);
            if recurrence.is_null() {
                target.insert("recurrence_start_at".to_string(), JsonValue::Null);
                target.insert(
                    "recurrence_completed_count".to_string(),
                    JsonValue::from(0_u64),
                );
                target.insert("recurrence_last_completed_at".to_string(), JsonValue::Null);
            } else {
                let due = target
                    .get("due_at")
                    .and_then(JsonValue::as_str)
                    .or_else(|| target.get("due_date").and_then(JsonValue::as_str))
                    .ok_or_else(|| {
                        FunctionFailure::invalid_input(
                            "recurring actions require due_at or due_date",
                        )
                    })?;
                target.insert("recurrence_start_at".to_string(), JsonValue::from(due));
                target.insert(
                    "recurrence_completed_count".to_string(),
                    JsonValue::from(0_u64),
                );
                target.insert("recurrence_last_completed_at".to_string(), JsonValue::Null);
            }
        } else if due_patched && !previous_recurrence.is_null() {
            let due = target
                .get("due_at")
                .and_then(JsonValue::as_str)
                .or_else(|| target.get("due_date").and_then(JsonValue::as_str))
                .ok_or_else(|| {
                    FunctionFailure::invalid_input("recurring actions require due_at or due_date")
                })?;
            target.insert("recurrence_start_at".to_string(), JsonValue::from(due));
        }

        if !patch.contains_key("buffer_minutes")
            && previous_duration.is_some()
            && previous_buffer == previous_duration
            && let Some(duration) = target.get("duration_minutes").and_then(JsonValue::as_u64)
        {
            target.insert("buffer_minutes".to_string(), JsonValue::from(duration));
        }

        let new_status =
            require_choice(target.get("status").unwrap(), ACTION_STATUSES, "status")?.to_string();
        if previous_status != new_status && previous_status != "open" && new_status != "open" {
            return Err(FunctionFailure::new(
                "INVALID_TRANSITION",
                "reopen action before changing terminal status",
            ));
        }
        if new_status != previous_status {
            target.insert(
                "completed_at".to_string(),
                if new_status == "done" {
                    JsonValue::from(now_iso8601())
                } else {
                    JsonValue::Null
                },
            );
        }
    }

    validate_action(target)?;
    bump_record(target, request);
    let res = mutation_result(request, target, operation);
    let active = target.get("status").and_then(JsonValue::as_str) == Some("open")
        && target.get("archived_at").is_some_and(JsonValue::is_null);
    if active {
        ensure_queued(document, &action_id)?;
    } else {
        remove_from_queue(document, &action_id)?;
    }
    validate_action_queue(document)?;
    Ok(res)
}

/// Compute the next due value for one recurrence occurrence.
///
/// The first occurrence is `recurrence_start_at`; `recurrence_completed_count`
/// indexes the current occurrence. Date-only actions keep their date format;
/// timed actions keep the anchor's time and offset.
fn next_action_recurrence_due(
    record: &BTreeMap<String, JsonValue>,
) -> Result<(Option<String>, u64), FunctionFailure> {
    let recurrence = record
        .get("recurrence")
        .ok_or_else(|| FunctionFailure::invalid_input("recurrence is missing"))?;
    if recurrence.is_null() {
        return Err(FunctionFailure::new(
            "INVALID_TRANSITION",
            "action is not recurring; use action.update to complete it",
        ));
    }
    let anchor = record
        .get("recurrence_start_at")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| {
            FunctionFailure::invalid_input("recurring actions require recurrence_start_at")
        })?;
    let first_date = anchor.get(..10).unwrap_or(anchor);
    let spec = parse_recurrence(recurrence, first_date)?
        .ok_or_else(|| FunctionFailure::invalid_input("recurrence must be an object"))?;
    let completed = record
        .get("recurrence_completed_count")
        .and_then(JsonValue::as_u64)
        .unwrap_or(0);
    let (dates, _) = recurring_start_dates(first_date, &spec, None)?;
    let next_index = usize::try_from(completed)
        .unwrap_or(usize::MAX)
        .saturating_add(1);
    let next_due = dates.get(next_index).map(|date| {
        if anchor.len() > 10 {
            format!("{date}{}", &anchor[10..])
        } else {
            date.clone()
        }
    });
    Ok((next_due, completed.saturating_add(1)))
}

fn complete_action_occurrence(
    document: &mut BTreeMap<String, JsonValue>,
    request: &Request,
) -> Result<BTreeMap<String, JsonValue>, FunctionFailure> {
    check_fields(&request.input, &["action_id"], &["action_id"])?;
    let action_id =
        require_non_empty_str(request.input.get("action_id").unwrap(), "action_id")?.to_string();
    let (next_due, completed) = {
        let target = find_record_mut(document, request.input.get("action_id").unwrap())?;
        validate_action(target)?;
        check_version(target, request.expected_version)?;
        if target
            .get("archived_at")
            .is_some_and(|value| !value.is_null())
        {
            return Err(FunctionFailure::new(
                "INVALID_TRANSITION",
                "restore action before completing an occurrence",
            ));
        }
        if target.get("status").and_then(JsonValue::as_str) != Some("open") {
            return Err(FunctionFailure::new(
                "INVALID_TRANSITION",
                "only open recurring actions can complete an occurrence",
            ));
        }

        let (next_due, completed) = next_action_recurrence_due(target)?;
        let now = now_iso8601();
        target.insert(
            "recurrence_completed_count".to_string(),
            JsonValue::from(completed),
        );
        target.insert(
            "recurrence_last_completed_at".to_string(),
            JsonValue::from(now.clone()),
        );
        if let Some(due) = next_due.clone() {
            if target.get("due_at").is_some_and(|value| !value.is_null()) {
                target.insert("due_at".to_string(), JsonValue::from(due));
            } else {
                target.insert("due_date".to_string(), JsonValue::from(due));
            }
            target.insert("completed_at".to_string(), JsonValue::Null);
            target.insert("status".to_string(), JsonValue::from("open"));
        } else {
            target.insert("status".to_string(), JsonValue::from("done"));
            target.insert("completed_at".to_string(), JsonValue::from(now));
        }
        (next_due, completed)
    };
    let series_completed = next_due.is_none();
    if series_completed {
        remove_from_queue(document, &action_id)?;
    } else {
        ensure_queued(document, &action_id)?;
    }
    validate_action_queue(document)?;
    let target = find_record_mut(document, request.input.get("action_id").unwrap())?;
    bump_record(target, request);
    validate_action(target)?;

    let mut result = mutation_result(request, target, "complete_action_occurrence");
    result.insert(
        "recurrence_completed_count".to_string(),
        JsonValue::from(completed),
    );
    result.insert(
        "recurrence_count".to_string(),
        target
            .get("recurrence")
            .and_then(JsonValue::as_object)
            .and_then(|recurrence| recurrence.get("count"))
            .cloned()
            .unwrap_or(JsonValue::Null),
    );
    result.insert(
        "series_completed".to_string(),
        JsonValue::Bool(series_completed),
    );
    result.insert(
        "next_due".to_string(),
        next_due.map(JsonValue::from).unwrap_or(JsonValue::Null),
    );
    Ok(result)
}

pub fn handle_action_complete(
    store: &dyn DocumentStore,
    request: &Request,
) -> Result<JsonValue, FunctionFailure> {
    run_collection_mutation(
        store,
        request,
        "actions",
        "complete_action_occurrence",
        validate_action,
        complete_action_occurrence,
    )
}

/// Find a record by ID in a non-actions collection of the goals document.
fn find_goal_record<'a>(
    document: &'a Document,
    collection: &str,
    identity: &str,
) -> Result<&'a BTreeMap<String, JsonValue>, FunctionFailure> {
    let items = document
        .get(collection)
        .and_then(JsonValue::as_array)
        .ok_or_else(|| {
            FunctionFailure::new("DATA_INVALID", format!("{collection} must be an array"))
        })?;
    for item in items {
        let record = item.as_object().ok_or_else(|| {
            FunctionFailure::new(
                "DATA_INVALID",
                format!("{collection} records must be objects"),
            )
        })?;
        if record.get("id").and_then(JsonValue::as_str) == Some(identity) {
            return Ok(record);
        }
    }
    Err(FunctionFailure::not_found(format!(
        "{collection} record {identity} was not found"
    )))
}

/// Resolve the goal and milestone that own a checkpoint.
fn checkpoint_chain(
    goals: &Document,
    checkpoint_id: &str,
) -> Result<(String, String), FunctionFailure> {
    let checkpoint = find_goal_record(goals, "checkpoints", checkpoint_id)?;
    let milestone_id = checkpoint
        .get("milestone_id")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| {
            FunctionFailure::new(
                "DATA_INVALID",
                format!("checkpoint {checkpoint_id} has no milestone_id"),
            )
        })?;
    let milestone = find_goal_record(goals, "milestones", milestone_id)?;
    let goal_id = milestone
        .get("goal_id")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| {
            FunctionFailure::new(
                "DATA_INVALID",
                format!("milestone {milestone_id} has no goal_id"),
            )
        })?;
    find_goal_record(goals, "goals", goal_id)?;
    Ok((goal_id.to_string(), milestone_id.to_string()))
}

/// Append the new action's link to its checkpoint and bump the goals version.
fn append_action_link(
    goals: &mut Document,
    action_id: &str,
    checkpoint_id: &str,
    goal_id: &str,
    milestone_id: &str,
) -> Result<u64, FunctionFailure> {
    let link = JsonValue::object([
        ("action_id", JsonValue::from(action_id)),
        ("goal_ids", JsonValue::Array(Vec::new())),
        ("goal_id", JsonValue::from(goal_id)),
        ("milestone_id", JsonValue::from(milestone_id)),
        ("checkpoint_id", JsonValue::from(checkpoint_id)),
        ("role", JsonValue::from("core")),
        ("contribution", JsonValue::from("")),
    ]);
    let links = goals
        .get_mut("action_links")
        .and_then(JsonValue::as_array_mut)
        .ok_or_else(|| FunctionFailure::new("DATA_INVALID", "action_links must be an array"))?;
    if links
        .iter()
        .any(|item| item.get("action_id").and_then(JsonValue::as_str) == Some(action_id))
    {
        return Err(FunctionFailure::new(
            "DATA_INVALID",
            format!("action {action_id} already has an action link"),
        ));
    }
    links.push(link);
    bump_document(goals, &format!("create_action:{action_id}"));
    goals
        .get("version")
        .and_then(JsonValue::as_u64)
        .ok_or_else(|| FunctionFailure::new("DATA_INVALID", "goals version must be an integer"))
}

fn create_action(
    document: &mut Document,
    request: &Request,
    goals: &mut Document,
) -> Result<BTreeMap<String, JsonValue>, FunctionFailure> {
    check_fields(
        &request.input,
        ACTION_CREATE_ALLOWED,
        &["title", "checkpoint_id"],
    )?;
    check_version(document, request.expected_version)?;
    let checkpoint_id =
        require_non_empty_str(request.input.get("checkpoint_id").unwrap(), "checkpoint_id")?;
    let (goal_id, milestone_id) = checkpoint_chain(goals, checkpoint_id)?;
    let duration_minutes = match request.input.get("duration_minutes") {
        Some(value) => value
            .as_u64()
            .ok_or_else(|| FunctionFailure::invalid_input("duration_minutes must be an integer"))?,
        None => DEFAULT_ACTION_MINUTES,
    };
    let buffer_minutes = match request.input.get("buffer_minutes") {
        Some(value) => value
            .as_u64()
            .ok_or_else(|| FunctionFailure::invalid_input("buffer_minutes must be an integer"))?,
        None => duration_minutes,
    };

    let seq = document
        .get("items")
        .and_then(JsonValue::as_array)
        .map_or(1, |a| (a.len() + 1) as u64);

    let JsonValue::Object(defaults) = JsonValue::object([
        ("status", JsonValue::from("open")),
        ("notes", JsonValue::from("")),
        ("priority", JsonValue::from("normal")),
        ("due_at", JsonValue::Null),
        ("due_date", JsonValue::Null),
        ("completed_at", JsonValue::Null),
        ("tags", JsonValue::Array(Vec::new())),
        ("duration_minutes", JsonValue::from(duration_minutes)),
        ("buffer_minutes", JsonValue::from(buffer_minutes)),
        ("recurrence", JsonValue::Null),
        ("recurrence_start_at", JsonValue::Null),
        ("recurrence_completed_count", JsonValue::from(0_u64)),
        ("recurrence_last_completed_at", JsonValue::Null),
    ]) else {
        unreachable!()
    };

    let mut action_payload = request.input.clone();
    action_payload.remove("checkpoint_id");
    let mut action = new_record(
        "action",
        &request.request_id,
        seq,
        defaults,
        &action_payload,
    );
    if !action.get("recurrence").is_some_and(JsonValue::is_null) {
        let due = if let Some(value) = action.get("due_at").and_then(JsonValue::as_str) {
            JsonValue::from(value)
        } else if let Some(value) = action.get("due_date").and_then(JsonValue::as_str) {
            JsonValue::from(value)
        } else {
            return Err(FunctionFailure::invalid_input(
                "recurring actions require due_at or due_date",
            ));
        };
        action.insert("recurrence_start_at".to_string(), due);
    }
    validate_action(&action)?;

    let mut res = mutation_result(request, &action, "create_action");
    let action_id = action
        .get("id")
        .and_then(JsonValue::as_str)
        .expect("new action has an id")
        .to_string();
    document
        .get_mut("items")
        .and_then(JsonValue::as_array_mut)
        .unwrap()
        .push(JsonValue::Object(action));
    ensure_queued(document, &action_id)?;
    validate_action_queue(document)?;

    let goals_version =
        append_action_link(goals, &action_id, checkpoint_id, &goal_id, &milestone_id)?;
    if let Some(versions) = res
        .get_mut("new_versions")
        .and_then(JsonValue::as_object_mut)
    {
        versions.insert("goals".to_string(), JsonValue::from(goals_version));
    }
    Ok(res)
}

pub fn handle_action_create(
    store: &dyn DocumentStore,
    request: &Request,
) -> Result<JsonValue, FunctionFailure> {
    store.transaction(&mut |transaction| {
        let mut goals = transaction.load_optional("goals")?;
        let outcome = transaction.mutate("actions", request, "create_action", &mut |document| {
            validate_envelope(document)?;
            let schema_version = document
                .get("schema_version")
                .and_then(JsonValue::as_u64)
                .unwrap_or(0);
            if schema_version != SCHEMA_VERSION {
                return Err(FunctionFailure::new(
                    "MIGRATION_REQUIRED",
                    format!("migrate actions to schema_version={SCHEMA_VERSION} first"),
                ));
            }
            validate_records(document, validate_action)?;
            let goals = goals.as_mut().ok_or_else(|| {
                FunctionFailure::not_found(
                    "goals collection was not initialized; create a goal, milestone, and checkpoint first",
                )
            })?;
            Ok(JsonValue::Object(create_action(document, request, goals)?))
        })?;

        if matches!(&outcome, MutateOutcome::Applied { .. }) {
            transaction.replace(
                "goals",
                goals
                    .as_ref()
                    .expect("created action link mutates goals"),
            )?;
        }

        Ok(match outcome {
            MutateOutcome::Replayed(value) => value,
            MutateOutcome::Applied { result, .. } => result,
        })
    })
}

pub fn handle_action_update(
    store: &dyn DocumentStore,
    request: &Request,
) -> Result<JsonValue, FunctionFailure> {
    run_collection_mutation(
        store,
        request,
        "actions",
        "update_action",
        validate_action,
        |doc, req| mutate_action(doc, req, "update_action"),
    )
}

pub fn handle_action_archive(
    store: &dyn DocumentStore,
    request: &Request,
) -> Result<JsonValue, FunctionFailure> {
    run_collection_mutation(
        store,
        request,
        "actions",
        "archive_action",
        validate_action,
        |doc, req| mutate_action(doc, req, "archive_action"),
    )
}

pub fn handle_action_delete(
    store: &dyn DocumentStore,
    request: &Request,
) -> Result<JsonValue, FunctionFailure> {
    store.transaction(&mut |transaction| {
        let mut goals = transaction.load_optional("goals")?;
        let mut goals_changed = false;
        let mut new_versions = BTreeMap::new();

        let outcome = transaction.mutate("actions", request, "delete_action", &mut |document| {
            validate_envelope(document)?;
            let schema_version = document
                .get("schema_version")
                .and_then(JsonValue::as_u64)
                .unwrap_or(0);
            if schema_version != SCHEMA_VERSION {
                return Err(FunctionFailure::new(
                    "MIGRATION_REQUIRED",
                    format!("migrate actions to schema_version={SCHEMA_VERSION} first"),
                ));
            }
            validate_records(document, validate_action)?;
            check_fields(&request.input, &["action_id"], &["action_id"])?;
            let action_id =
                require_non_empty_str(request.input.get("action_id").unwrap(), "action_id")?;
            let target = find_record(document, action_id)?;
            validate_action(target)?;
            check_version(target, request.expected_version)?;

            document
                .get_mut("items")
                .and_then(JsonValue::as_array_mut)
                .ok_or_else(|| FunctionFailure::new("DATA_INVALID", "items must be an array"))?
                .retain(|item| item.get("id").and_then(JsonValue::as_str) != Some(action_id));
            remove_from_queue(document, action_id)?;
            validate_action_queue(document)?;

            if let Some(goals_doc) = goals.as_mut()
                && let Some(links) = goals_doc
                    .get_mut("action_links")
                    .and_then(JsonValue::as_array_mut)
            {
                let before = links.len();
                links.retain(|link| {
                    link.get("action_id").and_then(JsonValue::as_str) != Some(action_id)
                });
                if links.len() != before {
                    goals_changed = true;
                    bump_document(goals_doc, &format!("delete_action:{}", request.request_id));
                    if let Some(version) = goals_doc.get("version").and_then(JsonValue::as_u64) {
                        new_versions.insert("goals".to_string(), JsonValue::from(version));
                    }
                }
            }

            Ok(JsonValue::Object(object_result(
                request,
                action_id,
                "delete_action",
                new_versions.clone(),
            )))
        })?;

        if matches!(&outcome, MutateOutcome::Applied { .. }) && goals_changed {
            transaction.replace("goals", goals.as_ref().expect("goals changed"))?;
        }

        Ok(match outcome {
            MutateOutcome::Replayed(value) => value,
            MutateOutcome::Applied { result, .. } => result,
        })
    })
}

/// Move an open action to `position` in the document-level queue.
fn mutate_action_queue(
    document: &mut BTreeMap<String, JsonValue>,
    request: &Request,
) -> Result<BTreeMap<String, JsonValue>, FunctionFailure> {
    check_fields(
        &request.input,
        &["action_id", "position"],
        &["action_id", "position"],
    )?;
    check_version(document, request.expected_version)?;

    let action_id = require_non_empty_str(request.input.get("action_id").unwrap(), "action_id")?;
    let position = request
        .input
        .get("position")
        .and_then(JsonValue::as_u64)
        .ok_or_else(|| FunctionFailure::invalid_input("position must be a non-negative integer"))?
        as usize;

    let record = find_record(document, action_id)?;
    let open = record.get("status").and_then(JsonValue::as_str) == Some("open")
        && record.get("archived_at").is_some_and(JsonValue::is_null);
    if !open {
        return Err(FunctionFailure::new(
            "INVALID_TRANSITION",
            "only open, unarchived actions can be queued",
        ));
    }
    let action_version = record
        .get("version")
        .and_then(JsonValue::as_u64)
        .unwrap_or(1);

    let queue = queue_items_mut(document)?;
    queue.retain(|value| value.as_str() != Some(action_id));
    let index = position.min(queue.len());
    queue.insert(index, JsonValue::from(action_id));
    validate_action_queue(document)?;

    let JsonValue::Object(result) = JsonValue::object([
        ("status", JsonValue::from("succeeded")),
        ("request_id", JsonValue::from(request.request_id.clone())),
        ("changed_records", JsonValue::string_array(&[action_id])),
        (
            "new_versions",
            JsonValue::object([(action_id, JsonValue::from(action_version))]),
        ),
        (
            "summary",
            JsonValue::from(format!("queue_action: {action_id} at {index}")),
        ),
    ]) else {
        unreachable!()
    };
    Ok(result)
}

pub fn handle_action_queue(
    store: &dyn DocumentStore,
    request: &Request,
) -> Result<JsonValue, FunctionFailure> {
    run_collection_mutation(
        store,
        request,
        "actions",
        "queue_action",
        validate_action,
        mutate_action_queue,
    )
}

// ---- Recurrence support -----------------------------------------------------

/// Parsed and validated recurrence rule.
struct RecurrenceSpec {
    frequency: String,
    interval: i64,
    count: Option<u64>,
    until: Option<String>,
    by_weekday: Option<Vec<usize>>,
}

fn is_leap_year(year: u32) -> bool {
    (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
}

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 30,
    }
}

/// Weekday index of a calendar date (0 = Monday .. 6 = Sunday).
fn weekday_index_from_date(date: &str) -> Result<usize, FunctionFailure> {
    let (y, m, d) = validate_iso_date(date, "date")?;
    let days = days_from_civil(y as i64, m, d);
    Ok(((days + 3).rem_euclid(7)) as usize)
}

fn add_days_to_date(date: &str, days: i64) -> Result<String, FunctionFailure> {
    let (y, m, d) = validate_iso_date(date, "date")?;
    let base = days_from_civil(y as i64, m, d);
    let (ny, nm, nd) = civil_from_days(base + days);
    Ok(format!("{ny:04}-{nm:02}-{nd:02}"))
}

fn add_months_to_date(date: &str, months: i64) -> Result<String, FunctionFailure> {
    let (y, m, d) = validate_iso_date(date, "date")?;
    let total_months = y as i64 * 12 + (m as i64 - 1) + months;
    let ny = total_months.div_euclid(12);
    let nm = (total_months.rem_euclid(12) + 1) as u32;
    if !(1..=9999).contains(&ny) {
        return Err(FunctionFailure::invalid_input(
            "recurrence exceeds the supported date range",
        ));
    }
    let nd = d.min(days_in_month(ny as u32, nm));
    Ok(format!("{ny:04}-{nm:02}-{nd:02}"))
}

/// Validate an optional `recurrence` value and return its parsed form.
///
/// `first_date` is the local start date of the first occurrence (`start_date` for
/// all-day events, the date part of `starts_at` for timed events).
fn parse_recurrence(
    recurrence: &JsonValue,
    first_date: &str,
) -> Result<Option<RecurrenceSpec>, FunctionFailure> {
    if recurrence.is_null() {
        return Ok(None);
    }
    let obj = recurrence
        .as_object()
        .ok_or_else(|| FunctionFailure::invalid_input("recurrence must be an object"))?;
    check_fields(obj, RECURRENCE_FIELDS, &["frequency"])?;

    let frequency = require_choice(
        obj.get("frequency").unwrap(),
        RECURRENCE_FREQUENCIES,
        "recurrence.frequency",
    )?
    .to_string();

    let interval = match obj.get("interval") {
        Some(value) => value
            .as_u64()
            .filter(|n| (1..=99).contains(n))
            .ok_or_else(|| {
                FunctionFailure::invalid_input(
                    "recurrence.interval must be an integer between 1 and 99",
                )
            })? as i64,
        None => 1,
    };

    let count = match obj.get("count") {
        Some(value) => Some(
            value
                .as_u64()
                .filter(|n| (1..=1000).contains(n))
                .ok_or_else(|| {
                    FunctionFailure::invalid_input(
                        "recurrence.count must be an integer between 1 and 1000",
                    )
                })?,
        ),
        None => None,
    };

    let until = match obj.get("until") {
        Some(value) => {
            let s = value.as_str().ok_or_else(|| {
                FunctionFailure::invalid_input("recurrence.until must be YYYY-MM-DD")
            })?;
            validate_iso_date(s, "recurrence.until")?;
            if s < first_date {
                return Err(FunctionFailure::invalid_input(
                    "recurrence.until must not precede the event start date",
                ));
            }
            Some(s.to_string())
        }
        None => None,
    };

    if count.is_some() && until.is_some() {
        return Err(FunctionFailure::invalid_input(
            "recurrence.count and recurrence.until are mutually exclusive",
        ));
    }

    let by_weekday = match obj.get("by_weekday") {
        Some(value) => {
            if frequency != "weekly" {
                return Err(FunctionFailure::invalid_input(
                    "recurrence.by_weekday is only supported for weekly frequency",
                ));
            }
            let arr = value.as_array().filter(|a| !a.is_empty()).ok_or_else(|| {
                FunctionFailure::invalid_input(
                    "recurrence.by_weekday must be a non-empty array of weekday names",
                )
            })?;
            let mut offsets: Vec<usize> = Vec::new();
            for item in arr {
                let name = require_choice(item, WEEKDAY_NAMES, "recurrence.by_weekday")?;
                let offset = WEEKDAY_NAMES.iter().position(|w| *w == name).unwrap();
                if !offsets.contains(&offset) {
                    offsets.push(offset);
                }
            }
            offsets.sort_unstable();
            let start_weekday = weekday_index_from_date(first_date)?;
            if !offsets.contains(&start_weekday) {
                return Err(FunctionFailure::invalid_input(
                    "recurrence.by_weekday must include the event start weekday",
                ));
            }
            Some(offsets)
        }
        None => None,
    };

    Ok(Some(RecurrenceSpec {
        frequency,
        interval,
        count,
        until,
        by_weekday,
    }))
}

/// Generate recurring start dates, stopping at `count`, `until`, or the occurrence cap.
///
/// Returns the dates in ascending order and whether generation was truncated by the cap.
fn recurring_start_dates(
    first_date: &str,
    spec: &RecurrenceSpec,
    stop_before: Option<&str>,
) -> Result<(Vec<String>, bool), FunctionFailure> {
    let mut dates: Vec<String> = Vec::new();
    let mut truncated = false;

    if spec.frequency == "weekly"
        && let Some(offsets) = &spec.by_weekday
    {
        let start_weekday = weekday_index_from_date(first_date)?;
        let (y, m, d) = validate_iso_date(first_date, "date")?;
        let monday = days_from_civil(y as i64, m, d) - start_weekday as i64;
        let mut week: i64 = 0;
        'weeks: loop {
            let week_monday = monday + week * spec.interval * 7;
            for offset in offsets {
                if let Some(count) = spec.count
                    && dates.len() as u64 >= count
                {
                    break 'weeks;
                }
                if dates.len() >= MAX_RECURRENCE_OCCURRENCES {
                    truncated = true;
                    break 'weeks;
                }
                let (ny, nm, nd) = civil_from_days(week_monday + *offset as i64);
                let date = format!("{ny:04}-{nm:02}-{nd:02}");
                if date.as_str() < first_date {
                    continue;
                }
                if stop_before.is_some_and(|stop| date.as_str() >= stop) {
                    break 'weeks;
                }
                if let Some(until) = &spec.until
                    && date.as_str() > until.as_str()
                {
                    break 'weeks;
                }
                dates.push(date);
            }
            week += 1;
            if week > 100_000 {
                truncated = true;
                break;
            }
        }
        return Ok((dates, truncated));
    }

    let mut index: u64 = 0;
    loop {
        if let Some(count) = spec.count
            && index >= count
        {
            break;
        }
        if dates.len() >= MAX_RECURRENCE_OCCURRENCES {
            truncated = true;
            break;
        }
        let steps = index * spec.interval as u64;
        let date = match spec.frequency.as_str() {
            "daily" => add_days_to_date(first_date, steps as i64)?,
            "weekly" => add_days_to_date(first_date, (steps * 7) as i64)?,
            "monthly" => add_months_to_date(first_date, steps as i64)?,
            "yearly" => add_months_to_date(first_date, (steps * 12) as i64)?,
            _ => unreachable!(),
        };
        if stop_before.is_some_and(|stop| date.as_str() >= stop) {
            break;
        }
        if let Some(until) = &spec.until
            && date.as_str() > until.as_str()
        {
            break;
        }
        dates.push(date);
        index += 1;
    }
    Ok((dates, truncated))
}

// ---- Profile (profile.json) validation and mutations ------------------------

struct ProfileUpdate {
    patch: BTreeMap<String, JsonValue>,
    remove: Vec<String>,
}

fn validate_profile_key(key: &str) -> Result<(), FunctionFailure> {
    if key.trim().is_empty() || key != key.trim() {
        return Err(FunctionFailure::invalid_input(
            "profile keys must be non-empty and have no surrounding whitespace",
        ));
    }
    if key.chars().count() > PROFILE_MAX_KEY_CHARS {
        return Err(FunctionFailure::invalid_input(format!(
            "profile keys must be at most {PROFILE_MAX_KEY_CHARS} characters"
        )));
    }
    if key.chars().any(char::is_control) {
        return Err(FunctionFailure::invalid_input(
            "profile keys must not contain control characters",
        ));
    }
    if PROFILE_RESERVED_FIELDS.contains(&key) {
        return Err(FunctionFailure::invalid_input(format!(
            "profile key `{key}` is reserved"
        )));
    }
    Ok(())
}

fn validate_profile_update(request: &Request) -> Result<ProfileUpdate, FunctionFailure> {
    check_fields(&request.input, &["patch", "remove"], &[])?;

    let patch = match request.input.get("patch") {
        Some(JsonValue::Object(map)) => map.clone(),
        Some(_) => {
            return Err(FunctionFailure::invalid_input(
                "input.patch must be an object",
            ));
        }
        None => BTreeMap::new(),
    };
    let mut remove = Vec::new();
    let mut seen_remove = BTreeSet::new();
    if let Some(value) = request.input.get("remove") {
        let items = value
            .as_array()
            .ok_or_else(|| FunctionFailure::invalid_input("input.remove must be a string array"))?;
        for item in items {
            let key = item.as_str().ok_or_else(|| {
                FunctionFailure::invalid_input("input.remove must be a string array")
            })?;
            validate_profile_key(key)?;
            if PROFILE_RETAINED_FIELDS.contains(&key) {
                return Err(FunctionFailure::invalid_input(format!(
                    "profile key `{key}` cannot be removed"
                )));
            }
            if !seen_remove.insert(key.to_string()) {
                return Err(FunctionFailure::invalid_input(format!(
                    "input.remove contains duplicate key `{key}`"
                )));
            }
            if patch.contains_key(key) {
                return Err(FunctionFailure::invalid_input(format!(
                    "profile key `{key}` cannot be both patched and removed"
                )));
            }
            remove.push(key.to_string());
        }
    }

    if patch.is_empty() && remove.is_empty() {
        return Err(FunctionFailure::invalid_input(
            "input must patch or remove at least one profile key",
        ));
    }

    for (key, value) in &patch {
        validate_profile_key(key)?;
        if value.to_json_string().len() > PROFILE_MAX_VALUE_BYTES {
            return Err(FunctionFailure::invalid_input(format!(
                "patch.{key} exceeds {PROFILE_MAX_VALUE_BYTES} bytes"
            )));
        }
    }
    for key in PROFILE_RETAINED_FIELDS {
        if let Some(value) = patch.get(*key) {
            require_non_empty_str(value, &format!("patch.{key}"))?;
        }
    }

    Ok(ProfileUpdate { patch, remove })
}

pub fn handle_profile_update(
    store: &dyn DocumentStore,
    request: &Request,
) -> Result<JsonValue, FunctionFailure> {
    let update = validate_profile_update(request)?;
    let outcome =
        store.with_document_mut("profile", request, "update_profile", &mut |profile| {
            if profile
                .get("version")
                .and_then(JsonValue::as_u64)
                .filter(|version| *version >= 1)
                .is_none()
            {
                return Err(FunctionFailure::new(
                    "DATA_INVALID",
                    "invalid profile version",
                ));
            }
            check_version(profile, request.expected_version)?;

            for key in &update.remove {
                profile.remove(key);
            }
            for (key, value) in &update.patch {
                profile.insert(key.clone(), value.clone());
            }
            let user_key_count = profile
                .keys()
                .filter(|key| !PROFILE_RESERVED_FIELDS.contains(&key.as_str()))
                .count();
            if user_key_count > PROFILE_MAX_KEYS {
                return Err(FunctionFailure::invalid_input(format!(
                    "profile has more than {PROFILE_MAX_KEYS} keys"
                )));
            }

            Ok(JsonValue::object([
                ("status", JsonValue::from("succeeded")),
                ("request_id", JsonValue::from(request.request_id.clone())),
                ("changed_records", JsonValue::string_array(&["profile"])),
                (
                    "new_versions",
                    JsonValue::object([("profile", JsonValue::from(request.expected_version + 1))]),
                ),
                ("summary", JsonValue::from("updated personal profile")),
            ]))
        })?;

    match outcome {
        MutateOutcome::Replayed(value) => Ok(value),
        MutateOutcome::Applied { result, .. } => Ok(result),
    }
}

// ---- Migration (migrate) and initialization (init) --------------------------

fn migrate_records(document: &mut BTreeMap<String, JsonValue>) -> Result<(), FunctionFailure> {
    let items = document
        .get_mut("items")
        .and_then(JsonValue::as_array_mut)
        .ok_or_else(|| FunctionFailure::new("DATA_INVALID", "items must be an array"))?;

    for item in items {
        let record = item
            .as_object_mut()
            .ok_or_else(|| FunctionFailure::new("DATA_INVALID", "record must be an object"))?;

        record
            .entry("notes".to_string())
            .or_insert_with(|| JsonValue::from(""));
        record
            .entry("archived_at".to_string())
            .or_insert(JsonValue::Null);
        record
            .entry("source".to_string())
            .or_insert_with(|| JsonValue::object([("type", JsonValue::from("legacy"))]));
        record
            .entry("priority".to_string())
            .or_insert_with(|| JsonValue::from("normal"));
        record
            .entry("due_at".to_string())
            .or_insert(JsonValue::Null);
        record
            .entry("due_date".to_string())
            .or_insert(JsonValue::Null);
        record
            .entry("completed_at".to_string())
            .or_insert(JsonValue::Null);
        record
            .entry("tags".to_string())
            .or_insert_with(|| JsonValue::Array(Vec::new()));
        record
            .entry("duration_minutes".to_string())
            .or_insert_with(|| JsonValue::from(DEFAULT_ACTION_MINUTES));
        let duration_minutes = record
            .get("duration_minutes")
            .and_then(JsonValue::as_u64)
            .unwrap_or(DEFAULT_ACTION_MINUTES);
        record
            .entry("buffer_minutes".to_string())
            .or_insert_with(|| JsonValue::from(duration_minutes));
        record
            .entry("recurrence".to_string())
            .or_insert(JsonValue::Null);
        record
            .entry("recurrence_start_at".to_string())
            .or_insert(JsonValue::Null);
        record
            .entry("recurrence_completed_count".to_string())
            .or_insert_with(|| JsonValue::from(0_u64));
        record
            .entry("recurrence_last_completed_at".to_string())
            .or_insert(JsonValue::Null);
    }

    validate_records(document, validate_action)
}

/// Pure migration to the current schema used by the `migrate` command and import.
pub fn migrate_document(document: &Document) -> Result<Document, FunctionFailure> {
    let mut migrated = document.clone();
    if !migrated.contains_key("items") {
        migrated.insert("items".to_string(), JsonValue::Array(Vec::new()));
    }
    migrate_records(&mut migrated)?;
    migrated.insert(
        "schema_version".to_string(),
        JsonValue::from(SCHEMA_VERSION),
    );
    let needs_queue = !migrated.contains_key("queue");
    if needs_queue {
        let queue = derived_queue(&migrated);
        migrated.insert("queue".to_string(), queue);
    }
    validate_action_queue(&migrated)?;
    Ok(migrated)
}

pub fn handle_migrate(
    store: &dyn DocumentStore,
    request: &Request,
) -> Result<JsonValue, FunctionFailure> {
    check_fields(
        &request.input,
        &["collection", "target_schema_version"],
        &["collection", "target_schema_version"],
    )?;
    require_choice(
        request.input.get("collection").unwrap(),
        &["actions"],
        "collection",
    )?;
    let target_ver = request
        .input
        .get("target_schema_version")
        .and_then(JsonValue::as_u64)
        .unwrap_or(0);
    if target_ver != SCHEMA_VERSION {
        return Err(FunctionFailure::invalid_input(format!(
            "target_schema_version must be {SCHEMA_VERSION}"
        )));
    }

    store.transaction(&mut |transaction| {
        let legacy = transaction.load_optional("tasks")?;
        let existing = transaction.load_optional("actions")?;
        if let (Some(existing), Some(legacy)) = (&existing, &legacy) {
            let existing_ids = document_item_ids(existing);
            let legacy_ids = document_item_ids(legacy);
            if !legacy_ids.is_subset(&existing_ids) {
                return Err(FunctionFailure::new(
                    "DATA_INVALID",
                    "both actions and legacy tasks exist with different records; reconcile them before migrating",
                ));
            }
        }
        let document = existing.or(legacy.clone()).ok_or_else(|| {
            FunctionFailure::new(
                "DATA_INVALID",
                "actions collection is missing (no actions or legacy tasks document)",
            )
        })?;

        validate_envelope(&document)?;
        let schema_version = match document.get("schema_version") {
            Some(value) => value.as_u64().ok_or_else(|| {
                FunctionFailure::new("DATA_INVALID", "unsupported existing schema_version")
            })?,
            None => 1,
        };
        if !(1..=SCHEMA_VERSION).contains(&schema_version) {
            return Err(FunctionFailure::new(
                "DATA_INVALID",
                "unsupported existing schema_version",
            ));
        }
        if schema_version == SCHEMA_VERSION {
            validate_records(&document, validate_action)?;
            validate_action_queue(&document)?;
            if legacy.is_some() {
                transaction.remove("tasks")?;
            }
            let current_ver = document
                .get("version")
                .and_then(JsonValue::as_u64)
                .unwrap_or(1);
            return Ok(JsonValue::object([
                ("status", JsonValue::from("succeeded")),
                ("request_id", JsonValue::from(request.request_id.clone())),
                ("changed_records", JsonValue::Array(Vec::new())),
                (
                    "new_versions",
                    JsonValue::object([("actions".to_string(), JsonValue::from(current_ver))]),
                ),
                ("summary", JsonValue::from("already migrated")),
            ]));
        }

        let current_version = document
            .get("version")
            .and_then(JsonValue::as_u64)
            .unwrap_or(0);
        if current_version != request.expected_version {
            return Err(FunctionFailure::version_conflict(format!(
                "version conflict: expected {}, current is {current_version}",
                request.expected_version
            )));
        }
        let new_version = current_version + 1;
        let mut migrated = migrate_document(&document)?;
        migrated.insert("version".to_string(), JsonValue::from(new_version));
        migrated.insert("updated_at".to_string(), JsonValue::from(now_iso8601()));
        migrated.insert(
            "updated_by".to_string(),
            JsonValue::from(format!("migrate:{}", request.request_id)),
        );
        transaction.replace("actions", &migrated)?;
        if legacy.is_some() {
            transaction.remove("tasks")?;
        }

        Ok(JsonValue::object([
            ("status", JsonValue::from("succeeded")),
            ("request_id", JsonValue::from(request.request_id.clone())),
            ("changed_records", JsonValue::string_array(&["actions"])),
            (
                "new_versions",
                JsonValue::object([("actions".to_string(), JsonValue::from(new_version))]),
            ),
            (
                "summary",
                JsonValue::from(format!("migrated actions to schema v{SCHEMA_VERSION}")),
            ),
        ]))
    })
}

/// Initialize missing `profile` and `actions` collections (or verify existing ones).
pub fn handle_init(store: &dyn DocumentStore) -> Result<JsonValue, FunctionFailure> {
    let timestamp = now_iso8601();
    let mut changed = Vec::new();
    let mut new_versions = BTreeMap::new();

    if let Some(profile) = store.load_optional("profile")? {
        let ver = profile
            .get("version")
            .and_then(JsonValue::as_u64)
            .filter(|version| *version >= 1)
            .ok_or_else(|| FunctionFailure::new("DATA_INVALID", "invalid profile version"))?;
        new_versions.insert("profile".to_string(), JsonValue::from(ver));
    } else {
        let JsonValue::Object(profile) = JsonValue::object([
            ("version", JsonValue::from(1_u64)),
            ("display_name", JsonValue::from("User")),
            ("timezone", JsonValue::from("Asia/Shanghai")),
            ("updated_at", JsonValue::from(timestamp.clone())),
            ("updated_by", JsonValue::from("init")),
            ("recent_requests", JsonValue::Array(Vec::new())),
        ]) else {
            unreachable!()
        };
        store.insert_initial("profile", &profile)?;
        changed.push(JsonValue::from("profile"));
        new_versions.insert("profile".to_string(), JsonValue::from(1_u64));
    }

    if let Some(document) = store.load_optional("actions")? {
        validate_envelope(&document)?;
        let schema_version = document
            .get("schema_version")
            .and_then(JsonValue::as_u64)
            .unwrap_or(0);
        if schema_version != SCHEMA_VERSION {
            return Err(FunctionFailure::new(
                "MIGRATION_REQUIRED",
                format!("migrate actions to schema_version={SCHEMA_VERSION} first"),
            ));
        }
        validate_records(&document, validate_action)?;
        validate_action_queue(&document)?;
        let ver = document
            .get("version")
            .and_then(JsonValue::as_u64)
            .unwrap_or(1);
        new_versions.insert("actions".to_string(), JsonValue::from(ver));
    } else {
        let JsonValue::Object(mut document) = JsonValue::object([
            ("schema_version", JsonValue::from(SCHEMA_VERSION)),
            ("version", JsonValue::from(1_u64)),
            ("updated_at", JsonValue::from(timestamp.clone())),
            ("updated_by", JsonValue::from("init")),
            ("items", JsonValue::Array(Vec::new())),
            ("recent_requests", JsonValue::Array(Vec::new())),
        ]) else {
            unreachable!()
        };
        document.insert("queue".to_string(), JsonValue::Array(Vec::new()));
        store.insert_initial("actions", &document)?;
        changed.push(JsonValue::from("actions"));
        new_versions.insert("actions".to_string(), JsonValue::from(1_u64));
    }

    Ok(JsonValue::object([
        ("status", JsonValue::from("succeeded")),
        ("request_id", JsonValue::from("init")),
        ("changed_records", JsonValue::Array(changed)),
        ("new_versions", JsonValue::Object(new_versions)),
        (
            "summary",
            JsonValue::from("initialized personal collections"),
        ),
    ]))
}
