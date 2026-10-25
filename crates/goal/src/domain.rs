//! `goal` domain validation, persistence, and command handlers for life planning
//! (`goals` collection).
//!
//! Four levels are modelled: goals (all horizons, nested through parent_goal_ids),
//! milestones (weeks), checkpoints (hours), and actions (atomic <=30-minute
//! steps in the `actions` execution queue). `action_links` may attach an
//! action to its checkpoint in addition to a goal or milestone.

use std::collections::{BTreeMap, BTreeSet};

use shared::{
    Document, DocumentStore, FunctionFailure, JsonValue, MutateOutcome, Request, check_fields,
    generate_id, now_iso8601, validate_instant, validate_iso_date,
};

pub const SCHEMA_VERSION: u64 = 2;
const ACTIONS_COLLECTION: &str = "actions";
const TIMEZONE: &str = "Asia/Shanghai";

const GOAL_STATUSES: &[&str] = &["proposed", "active", "paused", "achieved", "abandoned"];
const MILESTONE_STATUSES: &[&str] = &["planned", "active", "blocked", "completed", "skipped"];
const CHECKPOINT_STATUSES: &[&str] = &["planned", "active", "done", "cancelled"];
const HEALTH_VALUES: &[&str] = &["on_track", "at_risk", "blocked", "unknown"];
const PRIORITIES: &[&str] = &["high", "normal", "low"];
const ACTION_ROLES: &[&str] = &["core", "supporting", "maintenance"];
const PROGRESS_BASES: &[&str] = &["manual", "milestone_weighted"];
const PROGRESS_TARGETS: &[&str] = &["goal", "milestone"];

const GOAL_REQUIRED_FIELDS: &[&str] = &[
    "id",
    "parent_goal_ids",
    "title",
    "description",
    "purpose",
    "horizon",
    "status",
    "health",
    "priority",
    "start_date",
    "deadline",
    "success_criteria",
    "constraints",
    "progress",
    "progress_basis",
    "milestone_ids",
    "last_progress_at",
    "next_review_at",
    "next_action",
    "blockers",
    "risks",
    "assumptions",
    "created_at",
    "updated_at",
];
const GOAL_ALLOWED_FIELDS: &[&str] = &[
    "id",
    "parent_goal_ids",
    "title",
    "description",
    "purpose",
    "horizon",
    "status",
    "health",
    "priority",
    "start_date",
    "deadline",
    "success_criteria",
    "constraints",
    "progress",
    "progress_basis",
    "milestone_ids",
    "last_progress_at",
    "next_review_at",
    "next_action",
    "blockers",
    "risks",
    "assumptions",
    "created_at",
    "updated_at",
    "progress_events",
];
const GOAL_CREATE_FIELDS: &[&str] = &[
    "id",
    "group",
    "parent_goal_ids",
    "title",
    "description",
    "purpose",
    "horizon",
    "status",
    "health",
    "priority",
    "start_date",
    "deadline",
    "success_criteria",
    "constraints",
    "progress",
    "progress_basis",
    "last_progress_at",
    "next_review_at",
    "next_action",
    "blockers",
    "risks",
    "assumptions",
];
const GOAL_PATCH_FIELDS: &[&str] = &[
    "parent_goal_ids",
    "title",
    "description",
    "purpose",
    "horizon",
    "status",
    "health",
    "priority",
    "start_date",
    "deadline",
    "success_criteria",
    "constraints",
    "progress",
    "progress_basis",
    "milestone_ids",
    "last_progress_at",
    "next_review_at",
    "next_action",
    "blockers",
    "risks",
    "assumptions",
];

const MILESTONE_REQUIRED_FIELDS: &[&str] = &[
    "id",
    "goal_id",
    "title",
    "sequence",
    "status",
    "health",
    "start_date",
    "due_at",
    "exit_criteria",
    "progress",
    "action_ids",
    "dependencies",
    "next_action",
    "blockers",
    "last_progress_at",
    "updated_at",
];
const MILESTONE_ALLOWED_FIELDS: &[&str] = &[
    "id",
    "goal_id",
    "title",
    "sequence",
    "status",
    "health",
    "start_date",
    "due_at",
    "exit_criteria",
    "progress",
    "action_ids",
    "dependencies",
    "next_action",
    "blockers",
    "last_progress_at",
    "updated_at",
    "progress_events",
];
const MILESTONE_CREATE_FIELDS: &[&str] = &[
    "id",
    "goal_id",
    "title",
    "sequence",
    "status",
    "health",
    "start_date",
    "due_at",
    "exit_criteria",
    "progress",
    "action_ids",
    "dependencies",
    "next_action",
    "blockers",
    "last_progress_at",
];
const MILESTONE_PATCH_FIELDS: &[&str] = &[
    "goal_id",
    "title",
    "sequence",
    "status",
    "health",
    "start_date",
    "due_at",
    "exit_criteria",
    "progress",
    "action_ids",
    "dependencies",
    "next_action",
    "blockers",
    "last_progress_at",
];

const CHECKPOINT_FIELDS: &[&str] = &[
    "id",
    "milestone_id",
    "title",
    "description",
    "status",
    "estimate_minutes",
    "created_at",
    "updated_at",
];
const CHECKPOINT_CREATE_FIELDS: &[&str] = &[
    "id",
    "milestone_id",
    "title",
    "description",
    "status",
    "estimate_minutes",
];
const CHECKPOINT_PATCH_FIELDS: &[&str] = &[
    "milestone_id",
    "title",
    "description",
    "status",
    "estimate_minutes",
];

const ACTION_LINK_FIELDS: &[&str] = &[
    "action_id",
    "goal_ids",
    "goal_id",
    "milestone_id",
    "checkpoint_id",
    "role",
    "contribution",
];
/// `checkpoint_id` is optional so v1 action links remain valid.
const ACTION_LINK_REQUIRED_FIELDS: &[&str] = &[
    "action_id",
    "goal_ids",
    "goal_id",
    "milestone_id",
    "role",
    "contribution",
];

const PROGRESS_EVENT_FIELDS: &[&str] = &["at", "kind", "summary", "evidence", "progress_after"];

const PROGRESS_RECORD_FIELDS: &[&str] = &[
    "target",
    "target_id",
    "at",
    "kind",
    "summary",
    "evidence",
    "progress_after",
];

const REVIEW_FIELDS: &[&str] = &["cadence", "next_review_at"];

// ---- default_document -------------------------------------------------------

/// Construct a fresh `goals.json` document with empty collections.
pub fn default_document(timestamp: &str) -> BTreeMap<String, JsonValue> {
    object_of([
        ("schema_version", JsonValue::from(SCHEMA_VERSION)),
        ("version", JsonValue::from(1_u64)),
        ("updated_at", JsonValue::from(timestamp)),
        ("updated_by", JsonValue::from("init")),
        ("timezone", JsonValue::from(TIMEZONE)),
        (
            "review",
            JsonValue::object([
                ("cadence", JsonValue::from("weekly")),
                ("next_review_at", JsonValue::Null),
            ]),
        ),
        ("goals", JsonValue::Array(Vec::new())),
        ("milestones", JsonValue::Array(Vec::new())),
        ("checkpoints", JsonValue::Array(Vec::new())),
        ("action_links", JsonValue::Array(Vec::new())),
        ("recent_requests", JsonValue::Array(Vec::new())),
    ])
}

// ---- general helpers --------------------------------------------------------

fn object_of<const N: usize>(entries: [(&str, JsonValue); N]) -> BTreeMap<String, JsonValue> {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect()
}

fn require_string<'a>(value: &'a JsonValue, field: &str) -> Result<&'a str, FunctionFailure> {
    value
        .as_str()
        .ok_or_else(|| FunctionFailure::invalid_input(format!("{field} must be a string")))
}

fn require_non_empty_str<'a>(
    value: &'a JsonValue,
    field: &str,
) -> Result<&'a str, FunctionFailure> {
    let text = require_string(value, field)?;
    if text.trim().is_empty() {
        return Err(FunctionFailure::invalid_input(format!(
            "{field} must be a non-empty string"
        )));
    }
    Ok(text)
}

fn require_choice<'a>(
    value: &'a JsonValue,
    choices: &[&str],
    field: &str,
) -> Result<&'a str, FunctionFailure> {
    let text = value
        .as_str()
        .ok_or_else(|| FunctionFailure::invalid_input(format!("invalid {field}")))?;
    if !choices.contains(&text) {
        return Err(FunctionFailure::invalid_input(format!(
            "invalid {field} '{text}'; expected one of: {}",
            choices.join(", ")
        )));
    }
    Ok(text)
}

fn validate_identifier(value: &JsonValue, field: &str) -> Result<String, FunctionFailure> {
    let text = require_non_empty_str(value, field)?;
    if text.len() > 100
        || !text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err(FunctionFailure::invalid_input(format!(
            "{field} must use 1..=100 ASCII letters, digits, '-', '_' or '.'"
        )));
    }
    Ok(text.to_string())
}

fn validate_progress(value: &JsonValue, field: &str) -> Result<u64, FunctionFailure> {
    value.as_u64().filter(|n| *n <= 100).ok_or_else(|| {
        FunctionFailure::invalid_input(format!("{field} must be an integer between 0 and 100"))
    })
}

fn validate_string_array(
    value: &JsonValue,
    field: &str,
    require_non_empty_array: bool,
) -> Result<(), FunctionFailure> {
    let items = value.as_array().ok_or_else(|| {
        FunctionFailure::invalid_input(format!("{field} must be an array of strings"))
    })?;
    if require_non_empty_array && items.is_empty() {
        return Err(FunctionFailure::invalid_input(format!(
            "{field} must not be empty"
        )));
    }
    for item in items {
        require_non_empty_str(item, field)?;
    }
    Ok(())
}

fn validate_unique_string_array(
    value: &JsonValue,
    field: &str,
    require_non_empty_array: bool,
) -> Result<(), FunctionFailure> {
    validate_string_array(value, field, require_non_empty_array)?;
    let items = value.as_array().expect("array validated above");
    let mut seen = BTreeSet::new();
    for item in items {
        let text = item.as_str().unwrap_or("");
        if !seen.insert(text) {
            return Err(FunctionFailure::invalid_input(format!(
                "{field} must not contain duplicates"
            )));
        }
    }
    Ok(())
}

fn validate_date_or_null(value: &JsonValue, field: &str) -> Result<(), FunctionFailure> {
    if value.is_null() {
        return Ok(());
    }
    validate_iso_date(require_string(value, field)?, field)?;
    Ok(())
}

fn validate_instant_or_null(value: &JsonValue, field: &str) -> Result<(), FunctionFailure> {
    if value.is_null() {
        return Ok(());
    }
    validate_instant(require_string(value, field)?, field)?;
    Ok(())
}

fn require_instant(value: &JsonValue, field: &str) -> Result<(), FunctionFailure> {
    validate_instant(require_string(value, field)?, field)
}

fn copy_payload(
    record: &mut BTreeMap<String, JsonValue>,
    payload: &BTreeMap<String, JsonValue>,
    skip: &[&str],
) {
    for (key, value) in payload {
        if skip.contains(&key.as_str()) {
            continue;
        }
        record.insert(key.clone(), value.clone());
    }
}

fn string_values(value: &JsonValue) -> &[JsonValue] {
    value.as_array().map(Vec::as_slice).unwrap_or(&[])
}

// ---- collection helpers -----------------------------------------------------

fn collection_items<'a>(
    document: &'a BTreeMap<String, JsonValue>,
    collection: &str,
) -> Result<&'a Vec<JsonValue>, FunctionFailure> {
    document
        .get(collection)
        .and_then(JsonValue::as_array)
        .ok_or_else(|| {
            FunctionFailure::new("DATA_INVALID", format!("{collection} must be an array"))
        })
}

fn collection_items_mut<'a>(
    document: &'a mut BTreeMap<String, JsonValue>,
    collection: &str,
) -> Result<&'a mut Vec<JsonValue>, FunctionFailure> {
    document
        .get_mut(collection)
        .and_then(JsonValue::as_array_mut)
        .ok_or_else(|| {
            FunctionFailure::new("DATA_INVALID", format!("{collection} must be an array"))
        })
}

fn find_record<'a>(
    document: &'a BTreeMap<String, JsonValue>,
    collection: &str,
    id: &str,
) -> Result<&'a BTreeMap<String, JsonValue>, FunctionFailure> {
    for item in collection_items(document, collection)? {
        let record = item.as_object().ok_or_else(|| {
            FunctionFailure::new(
                "DATA_INVALID",
                format!("{collection} entries must be objects"),
            )
        })?;
        if record.get("id").and_then(JsonValue::as_str) == Some(id) {
            return Ok(record);
        }
    }
    Err(FunctionFailure::not_found(format!(
        "{id} was not found in {collection}"
    )))
}

fn find_record_mut<'a>(
    document: &'a mut BTreeMap<String, JsonValue>,
    collection: &str,
    id: &str,
) -> Result<&'a mut BTreeMap<String, JsonValue>, FunctionFailure> {
    for item in collection_items_mut(document, collection)? {
        let record = item.as_object_mut().ok_or_else(|| {
            FunctionFailure::new(
                "DATA_INVALID",
                format!("{collection} entries must be objects"),
            )
        })?;
        if record.get("id").and_then(JsonValue::as_str) == Some(id) {
            return Ok(record);
        }
    }
    Err(FunctionFailure::not_found(format!(
        "{id} was not found in {collection}"
    )))
}

fn has_record(document: &BTreeMap<String, JsonValue>, collection: &str, id: &str) -> bool {
    collection_items(document, collection)
        .map(|items| {
            items
                .iter()
                .any(|item| item.get("id").and_then(JsonValue::as_str) == Some(id))
        })
        .unwrap_or(false)
}

fn action_ids_of(actions: Option<&Document>) -> Result<Option<BTreeSet<String>>, FunctionFailure> {
    let Some(document) = actions else {
        return Ok(None);
    };
    let items = document
        .get("items")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| {
            FunctionFailure::new(
                "DATA_INVALID",
                "the actions collection must contain an items array",
            )
        })?;
    let mut ids = BTreeSet::new();
    for item in items {
        if let Some(id) = item.get("id").and_then(JsonValue::as_str) {
            ids.insert(id.to_string());
        }
    }
    Ok(Some(ids))
}

fn ensure_actions_exist(
    actions: Option<&Document>,
    action_ids: &JsonValue,
) -> Result<(), FunctionFailure> {
    let Some(known) = action_ids_of(actions)? else {
        return Ok(());
    };
    for item in string_values(action_ids) {
        let action_id = item.as_str().unwrap_or("");
        if !known.contains(action_id) {
            return Err(FunctionFailure::not_found(format!(
                "action {action_id} was not found in the {ACTIONS_COLLECTION} collection"
            )));
        }
    }
    Ok(())
}

// ---- record validators ------------------------------------------------------

fn validate_goal(record: &BTreeMap<String, JsonValue>) -> Result<(), FunctionFailure> {
    check_fields(record, GOAL_ALLOWED_FIELDS, GOAL_REQUIRED_FIELDS)?;
    validate_identifier(record.get("id").unwrap(), "id")?;
    validate_unique_string_array(
        record.get("parent_goal_ids").unwrap(),
        "parent_goal_ids",
        false,
    )?;
    require_non_empty_str(record.get("title").unwrap(), "title")?;
    require_string(record.get("description").unwrap(), "description")?;
    require_string(record.get("purpose").unwrap(), "purpose")?;
    require_non_empty_str(record.get("horizon").unwrap(), "horizon")?;
    require_choice(record.get("status").unwrap(), GOAL_STATUSES, "status")?;
    require_choice(record.get("health").unwrap(), HEALTH_VALUES, "health")?;
    require_choice(record.get("priority").unwrap(), PRIORITIES, "priority")?;
    validate_date_or_null(record.get("start_date").unwrap(), "start_date")?;
    validate_date_or_null(record.get("deadline").unwrap(), "deadline")?;
    if let (Some(start), Some(deadline)) = (
        record.get("start_date").and_then(JsonValue::as_str),
        record.get("deadline").and_then(JsonValue::as_str),
    ) && deadline < start
    {
        return Err(FunctionFailure::invalid_input(
            "deadline must not precede start_date",
        ));
    }
    validate_string_array(
        record.get("success_criteria").unwrap(),
        "success_criteria",
        true,
    )?;
    validate_progress(record.get("progress").unwrap(), "progress")?;
    validate_string_array(record.get("constraints").unwrap(), "constraints", false)?;
    require_choice(
        record.get("progress_basis").unwrap(),
        PROGRESS_BASES,
        "progress_basis",
    )?;
    validate_unique_string_array(record.get("milestone_ids").unwrap(), "milestone_ids", false)?;
    validate_instant_or_null(record.get("last_progress_at").unwrap(), "last_progress_at")?;
    validate_instant_or_null(record.get("next_review_at").unwrap(), "next_review_at")?;
    require_string(record.get("next_action").unwrap(), "next_action")?;
    validate_string_array(record.get("blockers").unwrap(), "blockers", false)?;
    validate_string_array(record.get("risks").unwrap(), "risks", false)?;
    validate_string_array(record.get("assumptions").unwrap(), "assumptions", false)?;
    require_instant(record.get("created_at").unwrap(), "created_at")?;
    require_instant(record.get("updated_at").unwrap(), "updated_at")?;
    if let Some(events) = record.get("progress_events") {
        validate_progress_events(events)?;
    }
    Ok(())
}

fn milestone_sort_date(record: &JsonValue) -> &str {
    record
        .get("start_date")
        .and_then(JsonValue::as_str)
        .or_else(|| record.get("due_at").and_then(JsonValue::as_str))
        .unwrap_or("9999-12-31")
}

fn milestone_sort_sequence(record: &JsonValue) -> u64 {
    record
        .get("sequence")
        .and_then(JsonValue::as_u64)
        .unwrap_or(0)
}

fn sort_milestones_by_start(
    document: &mut BTreeMap<String, JsonValue>,
) -> Result<(), FunctionFailure> {
    // Keep milestones grouped in goal order, but sort each goal by start date.
    let goal_order: BTreeMap<String, usize> = document
        .get("goals")
        .and_then(JsonValue::as_array)
        .map(|goals| {
            goals
                .iter()
                .enumerate()
                .filter_map(|(index, goal)| Some((goal.get("id")?.as_str()?.to_string(), index)))
                .collect()
        })
        .unwrap_or_default();
    let items = document
        .get_mut("milestones")
        .and_then(JsonValue::as_array_mut)
        .ok_or_else(|| FunctionFailure::new("DATA_INVALID", "milestones must be an array"))?;

    items.sort_by(|left, right| {
        let left_goal = left
            .get("goal_id")
            .and_then(JsonValue::as_str)
            .unwrap_or("");
        let right_goal = right
            .get("goal_id")
            .and_then(JsonValue::as_str)
            .unwrap_or("");
        let left_group = goal_order.get(left_goal).copied().unwrap_or(usize::MAX);
        let right_group = goal_order.get(right_goal).copied().unwrap_or(usize::MAX);
        let left_id = left.get("id").and_then(JsonValue::as_str).unwrap_or("");
        let right_id = right.get("id").and_then(JsonValue::as_str).unwrap_or("");
        left_group
            .cmp(&right_group)
            .then_with(|| milestone_sort_date(left).cmp(milestone_sort_date(right)))
            .then_with(|| milestone_sort_sequence(left).cmp(&milestone_sort_sequence(right)))
            .then_with(|| left_id.cmp(right_id))
    });
    Ok(())
}

fn validate_milestone(record: &BTreeMap<String, JsonValue>) -> Result<(), FunctionFailure> {
    check_fields(record, MILESTONE_ALLOWED_FIELDS, MILESTONE_REQUIRED_FIELDS)?;
    validate_identifier(record.get("id").unwrap(), "id")?;
    validate_identifier(record.get("goal_id").unwrap(), "goal_id")?;
    require_non_empty_str(record.get("title").unwrap(), "title")?;
    record
        .get("sequence")
        .and_then(JsonValue::as_u64)
        .filter(|n| *n >= 1)
        .ok_or_else(|| FunctionFailure::invalid_input("sequence must be a positive integer"))?;
    require_choice(record.get("status").unwrap(), MILESTONE_STATUSES, "status")?;
    require_choice(record.get("health").unwrap(), HEALTH_VALUES, "health")?;
    validate_date_or_null(record.get("start_date").unwrap(), "start_date")?;
    validate_date_or_null(record.get("due_at").unwrap(), "due_at")?;
    if let (Some(start), Some(due)) = (
        record.get("start_date").and_then(JsonValue::as_str),
        record.get("due_at").and_then(JsonValue::as_str),
    ) && due < start
    {
        return Err(FunctionFailure::invalid_input(
            "due_at must not precede start_date",
        ));
    }
    validate_string_array(record.get("exit_criteria").unwrap(), "exit_criteria", true)?;
    validate_progress(record.get("progress").unwrap(), "progress")?;
    validate_unique_string_array(record.get("action_ids").unwrap(), "action_ids", false)?;
    validate_unique_string_array(record.get("dependencies").unwrap(), "dependencies", false)?;
    require_string(record.get("next_action").unwrap(), "next_action")?;
    validate_string_array(record.get("blockers").unwrap(), "blockers", false)?;
    validate_instant_or_null(record.get("last_progress_at").unwrap(), "last_progress_at")?;
    require_instant(record.get("updated_at").unwrap(), "updated_at")?;
    if let Some(events) = record.get("progress_events") {
        validate_progress_events(events)?;
    }
    Ok(())
}

fn validate_checkpoint(record: &BTreeMap<String, JsonValue>) -> Result<(), FunctionFailure> {
    check_fields(record, CHECKPOINT_FIELDS, CHECKPOINT_FIELDS)?;
    validate_identifier(record.get("id").unwrap(), "id")?;
    validate_identifier(record.get("milestone_id").unwrap(), "milestone_id")?;
    require_non_empty_str(record.get("title").unwrap(), "title")?;
    require_string(record.get("description").unwrap(), "description")?;

    require_choice(record.get("status").unwrap(), CHECKPOINT_STATUSES, "status")?;
    record
        .get("estimate_minutes")
        .and_then(JsonValue::as_u64)
        .filter(|minutes| (1..=600).contains(minutes))
        .ok_or_else(|| {
            FunctionFailure::invalid_input("estimate_minutes must be an integer between 1 and 600")
        })?;
    require_instant(record.get("created_at").unwrap(), "created_at")?;
    require_instant(record.get("updated_at").unwrap(), "updated_at")?;
    Ok(())
}

fn validate_action_link(record: &BTreeMap<String, JsonValue>) -> Result<(), FunctionFailure> {
    check_fields(record, ACTION_LINK_FIELDS, ACTION_LINK_REQUIRED_FIELDS)?;
    validate_identifier(record.get("action_id").unwrap(), "action_id")?;
    validate_unique_string_array(record.get("goal_ids").unwrap(), "goal_ids", false)?;
    for field in ["goal_id", "milestone_id", "checkpoint_id"] {
        if let Some(value) = record.get(field)
            && !value.is_null()
        {
            validate_identifier(value, field)?;
        }
    }
    require_choice(record.get("role").unwrap(), ACTION_ROLES, "role")?;
    require_string(record.get("contribution").unwrap(), "contribution")?;
    Ok(())
}

fn validate_progress_event(event: &BTreeMap<String, JsonValue>) -> Result<(), FunctionFailure> {
    check_fields(event, PROGRESS_EVENT_FIELDS, PROGRESS_EVENT_FIELDS)?;
    require_instant(event.get("at").unwrap(), "at")?;
    require_non_empty_str(event.get("kind").unwrap(), "kind")?;
    require_non_empty_str(event.get("summary").unwrap(), "summary")?;
    require_string(event.get("evidence").unwrap(), "evidence")?;
    validate_progress(event.get("progress_after").unwrap(), "progress_after")?;
    Ok(())
}

fn validate_progress_events(value: &JsonValue) -> Result<(), FunctionFailure> {
    let events = value
        .as_array()
        .ok_or_else(|| FunctionFailure::invalid_input("progress_events must be an array"))?;
    for event in events {
        let record = event.as_object().ok_or_else(|| {
            FunctionFailure::invalid_input("progress_events entries must be objects")
        })?;
        validate_progress_event(record)?;
    }
    Ok(())
}

fn validate_review(review: &BTreeMap<String, JsonValue>) -> Result<(), FunctionFailure> {
    check_fields(review, REVIEW_FIELDS, REVIEW_FIELDS)?;
    require_non_empty_str(review.get("cadence").unwrap(), "review.cadence")?;
    validate_instant_or_null(
        review.get("next_review_at").unwrap(),
        "review.next_review_at",
    )?;
    Ok(())
}

// ---- document validation ----------------------------------------------------

fn envelope_version(document: &BTreeMap<String, JsonValue>) -> Result<u64, FunctionFailure> {
    let schema = document
        .get("schema_version")
        .and_then(JsonValue::as_u64)
        .ok_or_else(|| FunctionFailure::new("DATA_INVALID", "invalid or missing schema_version"))?;
    if schema != SCHEMA_VERSION {
        return Err(FunctionFailure::new(
            "DATA_INVALID",
            format!("unsupported schema_version={schema}; expected {SCHEMA_VERSION}"),
        ));
    }
    let version = document
        .get("version")
        .and_then(JsonValue::as_u64)
        .filter(|value| *value >= 1)
        .ok_or_else(|| FunctionFailure::new("DATA_INVALID", "invalid document version"))?;
    let timezone = document
        .get("timezone")
        .and_then(JsonValue::as_str)
        .unwrap_or("");
    if timezone != TIMEZONE {
        return Err(FunctionFailure::new(
            "DATA_INVALID",
            format!("timezone must be {TIMEZONE}"),
        ));
    }
    let review = document
        .get("review")
        .and_then(JsonValue::as_object)
        .ok_or_else(|| FunctionFailure::new("DATA_INVALID", "review must be an object"))?;
    validate_review(review)
        .map_err(|err| FunctionFailure::new("DATA_INVALID", format!("review: {}", err.message)))?;
    for collection in ["goals", "milestones", "checkpoints", "action_links"] {
        if !document
            .get(collection)
            .is_some_and(|value| value.as_array().is_some())
        {
            return Err(FunctionFailure::new(
                "DATA_INVALID",
                format!("{collection} must be an array"),
            ));
        }
    }
    Ok(version)
}

fn validate_collection<'a, F>(
    document: &'a BTreeMap<String, JsonValue>,
    collection: &str,
    validator: F,
) -> Result<&'a Vec<JsonValue>, FunctionFailure>
where
    F: Fn(&BTreeMap<String, JsonValue>) -> Result<(), FunctionFailure>,
{
    let items = collection_items(document, collection)?;
    let mut seen = BTreeSet::new();
    for item in items {
        let record = item.as_object().ok_or_else(|| {
            FunctionFailure::new(
                "DATA_INVALID",
                format!("{collection} entries must be objects"),
            )
        })?;
        let id = record
            .get("id")
            .or_else(|| record.get("action_id"))
            .and_then(JsonValue::as_str)
            .unwrap_or("");
        if id.is_empty() {
            return Err(FunctionFailure::new(
                "DATA_INVALID",
                format!("{collection} entry is missing its identifier"),
            ));
        }
        if !seen.insert(id.to_string()) {
            return Err(FunctionFailure::new(
                "DATA_INVALID",
                format!("{collection}: duplicate identifier {id}"),
            ));
        }
        if let Err(err) = validator(record) {
            return Err(FunctionFailure::new(
                "DATA_INVALID",
                format!("{collection}: {id}: {}", err.message),
            ));
        }
    }
    Ok(items)
}

/// Validate the complete document, including cross references, and return its
/// top-level version.
///
/// When the `actions` collection exists, every referenced action ID must exist
/// there; otherwise action references are accepted without cross-checking.
pub fn validate_document(
    document: &BTreeMap<String, JsonValue>,
    actions: Option<&Document>,
) -> Result<u64, FunctionFailure> {
    let version = envelope_version(document)?;

    let goals = validate_collection(document, "goals", validate_goal)?;
    let milestones = validate_collection(document, "milestones", validate_milestone)?;
    let checkpoints = validate_collection(document, "checkpoints", validate_checkpoint)?;
    let action_links = validate_collection(document, "action_links", validate_action_link)?;

    let goal_ids: BTreeSet<String> = goals
        .iter()
        .filter_map(|item| item.get("id").and_then(JsonValue::as_str))
        .map(str::to_string)
        .collect();
    let mut milestone_owners: BTreeMap<String, String> = BTreeMap::new();
    for item in milestones {
        if let (Some(id), Some(goal_id)) = (
            item.get("id").and_then(JsonValue::as_str),
            item.get("goal_id").and_then(JsonValue::as_str),
        ) {
            milestone_owners.insert(id.to_string(), goal_id.to_string());
        }
    }
    let mut checkpoint_owners: BTreeMap<String, String> = BTreeMap::new();
    for item in checkpoints {
        let record = item.as_object().expect("validated checkpoint");
        let id = record.get("id").and_then(JsonValue::as_str).unwrap_or("");
        let milestone_id = record
            .get("milestone_id")
            .and_then(JsonValue::as_str)
            .unwrap_or("");
        if !milestone_owners.contains_key(milestone_id) {
            return Err(FunctionFailure::new(
                "DATA_INVALID",
                format!("checkpoint {id} references missing milestone {milestone_id}"),
            ));
        }
        checkpoint_owners.insert(id.to_string(), milestone_id.to_string());
    }

    for item in goals {
        let record = item.as_object().expect("validated goal");
        let goal_id = record.get("id").and_then(JsonValue::as_str).unwrap_or("");
        for value in string_values(record.get("parent_goal_ids").unwrap()) {
            let parent_id = value.as_str().unwrap_or("");
            if parent_id == goal_id {
                return Err(FunctionFailure::new(
                    "DATA_INVALID",
                    format!("goal {goal_id} must not be its own parent"),
                ));
            }
            if !goal_ids.contains(parent_id) {
                return Err(FunctionFailure::new(
                    "DATA_INVALID",
                    format!("goal {goal_id} references missing parent goal {parent_id}"),
                ));
            }
        }
        for value in string_values(record.get("milestone_ids").unwrap()) {
            let milestone_id = value.as_str().unwrap_or("");
            match milestone_owners.get(milestone_id) {
                Some(owner) if owner == goal_id => {}
                Some(owner) => {
                    return Err(FunctionFailure::new(
                        "DATA_INVALID",
                        format!(
                            "goal {goal_id} references milestone {milestone_id} owned by {owner}"
                        ),
                    ));
                }
                None => {
                    return Err(FunctionFailure::new(
                        "DATA_INVALID",
                        format!("goal {goal_id} references missing milestone {milestone_id}"),
                    ));
                }
            }
        }
    }
    ensure_goal_hierarchy_is_acyclic(goals)?;

    for item in milestones {
        let record = item.as_object().expect("validated milestone");
        let milestone_id = record.get("id").and_then(JsonValue::as_str).unwrap_or("");
        let goal_id = record
            .get("goal_id")
            .and_then(JsonValue::as_str)
            .unwrap_or("");
        if !goal_ids.contains(goal_id) {
            return Err(FunctionFailure::new(
                "DATA_INVALID",
                format!("milestone {milestone_id} references missing goal {goal_id}"),
            ));
        }
        for value in string_values(record.get("dependencies").unwrap()) {
            let dependency = value.as_str().unwrap_or("");
            if dependency == milestone_id {
                return Err(FunctionFailure::new(
                    "DATA_INVALID",
                    format!("milestone {milestone_id} must not depend on itself"),
                ));
            }
            if !milestone_owners.contains_key(dependency) {
                return Err(FunctionFailure::new(
                    "DATA_INVALID",
                    format!("milestone {milestone_id} references missing dependency {dependency}"),
                ));
            }
        }
    }

    let known_actions = action_ids_of(actions)?;
    if let Some(known_actions) = &known_actions {
        for item in milestones {
            let record = item.as_object().expect("validated milestone");
            let milestone_id = record.get("id").and_then(JsonValue::as_str).unwrap_or("");
            for value in string_values(record.get("action_ids").unwrap()) {
                let action_id = value.as_str().unwrap_or("");
                if !known_actions.contains(action_id) {
                    return Err(FunctionFailure::new(
                        "DATA_INVALID",
                        format!(
                            "milestone {milestone_id} references missing action {action_id} in the {ACTIONS_COLLECTION} collection"
                        ),
                    ));
                }
            }
        }
    }

    for item in action_links {
        let record = item.as_object().expect("validated action link");
        let action_id = record
            .get("action_id")
            .and_then(JsonValue::as_str)
            .unwrap_or("");
        for value in string_values(record.get("goal_ids").unwrap()) {
            let linked_goal = value.as_str().unwrap_or("");
            if !goal_ids.contains(linked_goal) {
                return Err(FunctionFailure::new(
                    "DATA_INVALID",
                    format!("action link {action_id} references missing goal {linked_goal}"),
                ));
            }
        }
        let goal_id = record.get("goal_id").and_then(JsonValue::as_str);
        let milestone_id = record.get("milestone_id").and_then(JsonValue::as_str);
        if let Some(goal_id) = goal_id
            && !goal_ids.contains(goal_id)
        {
            return Err(FunctionFailure::new(
                "DATA_INVALID",
                format!("action link {action_id} references missing goal {goal_id}"),
            ));
        }
        if let Some(milestone_id) = milestone_id {
            match milestone_owners.get(milestone_id) {
                Some(owner) => {
                    if let Some(goal_id) = goal_id
                        && owner != goal_id
                    {
                        return Err(FunctionFailure::new(
                            "DATA_INVALID",
                            format!(
                                "action link {action_id} goal {goal_id} does not own milestone {milestone_id}"
                            ),
                        ));
                    }
                }
                None => {
                    return Err(FunctionFailure::new(
                        "DATA_INVALID",
                        format!(
                            "action link {action_id} references missing milestone {milestone_id}"
                        ),
                    ));
                }
            }
        }
        if let Some(checkpoint_id) = record.get("checkpoint_id").and_then(JsonValue::as_str) {
            let Some(owner_milestone) = checkpoint_owners.get(checkpoint_id) else {
                return Err(FunctionFailure::new(
                    "DATA_INVALID",
                    format!(
                        "action link {action_id} references missing checkpoint {checkpoint_id}"
                    ),
                ));
            };
            if let Some(milestone_id) = milestone_id
                && owner_milestone != milestone_id
            {
                return Err(FunctionFailure::new(
                    "DATA_INVALID",
                    format!(
                        "action link {action_id} checkpoint {checkpoint_id} belongs to another milestone"
                    ),
                ));
            }
            if let Some(goal_id) = goal_id {
                match milestone_owners.get(owner_milestone) {
                    Some(owner_goal) if owner_goal == goal_id => {}
                    _ => {
                        return Err(FunctionFailure::new(
                            "DATA_INVALID",
                            format!(
                                "action link {action_id} checkpoint {checkpoint_id} does not belong to goal {goal_id}"
                            ),
                        ));
                    }
                }
            }
        }
        if let Some(known_actions) = &known_actions
            && !known_actions.contains(action_id)
        {
            return Err(FunctionFailure::new(
                "DATA_INVALID",
                format!(
                    "action link {action_id} references missing action in the {ACTIONS_COLLECTION} collection"
                ),
            ));
        }
    }

    Ok(version)
}

fn ensure_goal_hierarchy_is_acyclic(goals: &[JsonValue]) -> Result<(), FunctionFailure> {
    let parents: BTreeMap<String, Vec<String>> = goals
        .iter()
        .filter_map(|item| {
            let record = item.as_object()?;
            let id = record.get("id")?.as_str()?.to_string();
            let values = string_values(record.get("parent_goal_ids")?)
                .iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect();
            Some((id, values))
        })
        .collect();

    for start in parents.keys() {
        let mut stack = vec![start.clone()];
        let mut visited = BTreeSet::new();
        while let Some(current) = stack.pop() {
            if !visited.insert(current.clone()) {
                continue;
            }
            for parent in parents.get(&current).into_iter().flatten() {
                if parent == start {
                    return Err(FunctionFailure::new(
                        "DATA_INVALID",
                        format!("goal hierarchy cycle detected at {start}"),
                    ));
                }
                stack.push(parent.clone());
            }
        }
    }
    Ok(())
}

// ---- mutation pipeline ------------------------------------------------------

fn build_result(
    request: &Request,
    changed_records: Vec<String>,
    summary: String,
) -> BTreeMap<String, JsonValue> {
    object_of([
        ("status", JsonValue::from("succeeded")),
        ("request_id", JsonValue::from(request.request_id.clone())),
        (
            "changed_records",
            JsonValue::Array(changed_records.into_iter().map(JsonValue::from).collect()),
        ),
        ("summary", JsonValue::from(summary)),
    ])
}

/// Execute a transactional, replay-aware mutation on `goals.json`.
///
/// The actions snapshot is read inside the same transaction so action links cannot
/// race with a concurrent action update.
fn run_goal_mutation<F>(
    store: &dyn DocumentStore,
    request: &Request,
    operation: &str,
    mutate: F,
) -> Result<JsonValue, FunctionFailure>
where
    F: Fn(
        &mut BTreeMap<String, JsonValue>,
        &Request,
        Option<&Document>,
    ) -> Result<BTreeMap<String, JsonValue>, FunctionFailure>,
{
    store.transaction(&mut |transaction| {
        let actions = transaction.load_optional(ACTIONS_COLLECTION)?;
        let outcome = transaction.mutate("goals", request, operation, &mut |document| {
            let current_version = envelope_version(document)?;
            if current_version != request.expected_version {
                return Err(FunctionFailure::version_conflict(format!(
                    "goals version conflict: expected {}, current is {current_version}",
                    request.expected_version
                )));
            }
            let mut result = mutate(document, request, actions.as_ref())?;
            validate_document(document, actions.as_ref())?;
            result.insert(
                "new_versions".to_string(),
                JsonValue::object([("goals", JsonValue::from(request.expected_version + 1))]),
            );
            Ok(JsonValue::Object(result))
        })?;

        Ok(match outcome {
            MutateOutcome::Replayed(value) => value,
            MutateOutcome::Applied { result, .. } => result,
        })
    })
}

// ---- init -------------------------------------------------------------------

/// Initialize the `goals` collection if missing, or verify it if already present.
pub fn handle_init(store: &dyn DocumentStore) -> Result<JsonValue, FunctionFailure> {
    let actions = store.load_optional(ACTIONS_COLLECTION)?;

    if let Some(document) = store.load_optional("goals")? {
        let version = validate_document(&document, actions.as_ref())?;
        return Ok(JsonValue::object([
            ("status", JsonValue::from("succeeded")),
            ("request_id", JsonValue::from("init")),
            ("changed_records", JsonValue::Array(Vec::new())),
            (
                "new_versions",
                JsonValue::object([("goals", JsonValue::from(version))]),
            ),
            ("collection_version", JsonValue::from(version)),
            ("summary", JsonValue::from("goals.json already initialized")),
        ]));
    }

    let document = default_document(&now_iso8601());
    validate_document(&document, actions.as_ref())?;
    store.insert_initial("goals", &document)?;

    Ok(JsonValue::object([
        ("status", JsonValue::from("succeeded")),
        ("request_id", JsonValue::from("init")),
        ("changed_records", JsonValue::string_array(&["goals"])),
        (
            "new_versions",
            JsonValue::object([("goals", JsonValue::from(1_u64))]),
        ),
        ("collection_version", JsonValue::from(1_u64)),
        ("summary", JsonValue::from("initialized goals.json")),
    ]))
}

// ---- migration --------------------------------------------------------------

/// Migrate a v1 goals document in place.
///
/// Directions become top-level goals; existing goals keep their IDs and their
/// `direction_ids` become `parent_goal_ids`. Existing task links become action
/// links. The operation is idempotent and only moves the collection version.
pub fn handle_migrate(
    store: &dyn DocumentStore,
    request: &Request,
) -> Result<JsonValue, FunctionFailure> {
    store.transaction(&mut |transaction| {
        let actions = transaction.load_optional(ACTIONS_COLLECTION)?;
        let Some(mut document) = transaction.load_optional("goals")? else {
            return Err(FunctionFailure::not_found(
                "the goals collection does not exist",
            ));
        };
        if let Some(target) = request.input.get("target_schema_version") {
            let target = target.as_u64().ok_or_else(|| {
                FunctionFailure::invalid_input("target_schema_version must be an integer")
            })?;
            if target != SCHEMA_VERSION {
                return Err(FunctionFailure::invalid_input(format!(
                    "unsupported target_schema_version={target}; expected {SCHEMA_VERSION}"
                )));
            }
        }
        let schema = document
            .get("schema_version")
            .and_then(JsonValue::as_u64)
            .ok_or_else(|| FunctionFailure::new("DATA_INVALID", "missing schema_version"))?;
        if schema == SCHEMA_VERSION {
            let version = validate_document(&document, actions.as_ref())?;
            return Ok(JsonValue::object([
                ("status", JsonValue::from("succeeded")),
                ("request_id", JsonValue::from(request.request_id.clone())),
                ("changed_records", JsonValue::Array(Vec::new())),
                (
                    "new_versions",
                    JsonValue::object([("goals", JsonValue::from(version))]),
                ),
                ("collection_version", JsonValue::from(version)),
                (
                    "summary",
                    JsonValue::from("goals document already uses schema v2"),
                ),
            ]));
        }
        if schema != 1 {
            return Err(FunctionFailure::new(
                "DATA_INVALID",
                format!("unsupported schema_version={schema}; expected 1 or {SCHEMA_VERSION}"),
            ));
        }
        let current_version = envelope_version_for_migration(&document)?;
        if current_version != request.expected_version {
            return Err(FunctionFailure::version_conflict(format!(
                "goals version conflict: expected {}, current is {current_version}",
                request.expected_version
            )));
        }

        migrate_v1_document(&mut document)?;
        validate_document(&document, actions.as_ref())?;
        let new_version = current_version + 1;
        document.insert("version".into(), JsonValue::from(new_version));
        document.insert("updated_at".into(), JsonValue::from(now_iso8601()));
        document.insert(
            "updated_by".into(),
            JsonValue::from(format!("migrate:{}", request.request_id)),
        );
        transaction.replace("goals", &document)?;

        Ok(JsonValue::object([
            ("status", JsonValue::from("succeeded")),
            ("request_id", JsonValue::from(request.request_id.clone())),
            (
                "changed_records",
                JsonValue::string_array(&["directions", "checkpoints", "action_links", "goals"]),
            ),
            (
                "new_versions",
                JsonValue::object([("goals", JsonValue::from(new_version))]),
            ),
            ("collection_version", JsonValue::from(new_version)),
            (
                "summary",
                JsonValue::from("migrated goals document to schema v2"),
            ),
        ]))
    })
}

fn envelope_version_for_migration(
    document: &BTreeMap<String, JsonValue>,
) -> Result<u64, FunctionFailure> {
    document
        .get("version")
        .and_then(JsonValue::as_u64)
        .filter(|version| *version >= 1)
        .ok_or_else(|| FunctionFailure::new("DATA_INVALID", "invalid document version"))
}

/// Migrate an imported legacy document without changing its collection version.
pub fn migrate_legacy_document(document: &Document) -> Result<Document, FunctionFailure> {
    let schema = document
        .get("schema_version")
        .and_then(JsonValue::as_u64)
        .unwrap_or(1);
    match schema {
        1 => {
            let mut migrated = document.clone();
            migrate_v1_document(&mut migrated)?;
            Ok(migrated)
        }
        2 => Ok(document.clone()),
        other => Err(FunctionFailure::new(
            "DATA_INVALID",
            format!("unsupported schema_version={other}; expected 1 or 2"),
        )),
    }
}

fn migrate_v1_document(document: &mut BTreeMap<String, JsonValue>) -> Result<(), FunctionFailure> {
    let directions = document
        .remove("directions")
        .unwrap_or_else(|| JsonValue::Array(Vec::new()));
    let directions = directions
        .as_array()
        .cloned()
        .ok_or_else(|| FunctionFailure::new("DATA_INVALID", "directions must be an array"))?;
    let goals = document
        .remove("goals")
        .unwrap_or_else(|| JsonValue::Array(Vec::new()));
    let goals = goals
        .as_array()
        .cloned()
        .ok_or_else(|| FunctionFailure::new("DATA_INVALID", "goals must be an array"))?;
    let milestones = document
        .remove("milestones")
        .unwrap_or_else(|| JsonValue::Array(Vec::new()));
    let milestones = milestones
        .as_array()
        .cloned()
        .ok_or_else(|| FunctionFailure::new("DATA_INVALID", "milestones must be an array"))?;
    let current_goals = document
        .remove("current_goals")
        .unwrap_or_else(|| JsonValue::Array(Vec::new()));
    let checkpoints = current_goals
        .as_array()
        .cloned()
        .ok_or_else(|| FunctionFailure::new("DATA_INVALID", "current_goals must be an array"))?;
    let task_links = document
        .remove("task_links")
        .unwrap_or_else(|| JsonValue::Array(Vec::new()));
    let task_links = task_links
        .as_array()
        .cloned()
        .ok_or_else(|| FunctionFailure::new("DATA_INVALID", "task_links must be an array"))?;

    let mut migrated_goals = Vec::with_capacity(directions.len() + goals.len());
    for item in directions {
        let direction = item.as_object().ok_or_else(|| {
            FunctionFailure::new("DATA_INVALID", "directions entries must be objects")
        })?;
        migrated_goals.push(migrate_direction_to_goal(direction)?);
    }
    for item in goals {
        let mut goal = item
            .as_object()
            .cloned()
            .ok_or_else(|| FunctionFailure::new("DATA_INVALID", "goals entries must be objects"))?;
        let direction_ids = goal
            .remove("direction_ids")
            .unwrap_or_else(|| JsonValue::Array(Vec::new()));
        goal.insert("parent_goal_ids".into(), direction_ids);
        goal.entry("purpose".into())
            .or_insert_with(|| JsonValue::from(""));
        goal.entry("constraints".into())
            .or_insert_with(|| JsonValue::Array(Vec::new()));
        if !goal.contains_key("created_at") {
            let created_at = goal.get("updated_at").cloned().unwrap_or(JsonValue::Null);
            goal.insert("created_at".into(), created_at);
        }
        migrated_goals.push(JsonValue::Object(goal));
    }

    let mut migrated_milestones = Vec::with_capacity(milestones.len());
    for item in milestones {
        let mut milestone = item.as_object().cloned().ok_or_else(|| {
            FunctionFailure::new("DATA_INVALID", "milestones entries must be objects")
        })?;
        if let Some(task_ids) = milestone.remove("task_ids") {
            milestone.insert("action_ids".into(), task_ids);
        } else {
            milestone
                .entry("action_ids".into())
                .or_insert_with(|| JsonValue::Array(Vec::new()));
        }
        migrated_milestones.push(JsonValue::Object(milestone));
    }

    let mut migrated_links = Vec::with_capacity(task_links.len());
    for item in task_links {
        let mut link = item.as_object().cloned().ok_or_else(|| {
            FunctionFailure::new("DATA_INVALID", "task_links entries must be objects")
        })?;
        if let Some(action_id) = link.remove("task_id") {
            link.insert("action_id".into(), action_id);
        }
        if let Some(goal_ids) = link.remove("direction_ids") {
            link.insert("goal_ids".into(), goal_ids);
        } else {
            link.entry("goal_ids".into())
                .or_insert_with(|| JsonValue::Array(Vec::new()));
        }
        if let Some(checkpoint_id) = link.remove("current_goal_id") {
            link.insert("checkpoint_id".into(), checkpoint_id);
        }
        migrated_links.push(JsonValue::Object(link));
    }

    document.insert("goals".into(), JsonValue::Array(migrated_goals));
    document.insert("milestones".into(), JsonValue::Array(migrated_milestones));
    document.insert("checkpoints".into(), JsonValue::Array(checkpoints));
    document.insert("action_links".into(), JsonValue::Array(migrated_links));
    document.insert("schema_version".into(), JsonValue::from(SCHEMA_VERSION));
    sort_milestones_by_start(document)?;
    Ok(())
}

fn migrate_direction_to_goal(
    direction: &BTreeMap<String, JsonValue>,
) -> Result<JsonValue, FunctionFailure> {
    let id = direction
        .get("id")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| FunctionFailure::new("DATA_INVALID", "direction is missing id"))?;
    let success_definition = direction
        .get("success_definition")
        .cloned()
        .unwrap_or_else(|| JsonValue::Array(Vec::new()));
    if success_definition
        .as_array()
        .is_none_or(|criteria| criteria.is_empty())
    {
        return Err(FunctionFailure::new(
            "DATA_INVALID",
            format!(
                "direction {id} has empty success_definition; add success criteria before migrating"
            ),
        ));
    }
    let status = match direction.get("status").and_then(JsonValue::as_str) {
        Some("retired") => "abandoned",
        Some(value) => value,
        None => "active",
    };
    let next_review_at = direction
        .get("review_at")
        .and_then(JsonValue::as_str)
        .map(|date| JsonValue::from(format!("{date}T20:00:00+08:00")))
        .unwrap_or(JsonValue::Null);
    Ok(JsonValue::object([
        ("id", JsonValue::from(id)),
        ("parent_goal_ids", JsonValue::Array(Vec::new())),
        (
            "title",
            direction
                .get("title")
                .cloned()
                .unwrap_or_else(|| JsonValue::from("")),
        ),
        (
            "description",
            direction
                .get("description")
                .cloned()
                .unwrap_or_else(|| JsonValue::from("")),
        ),
        (
            "purpose",
            direction
                .get("purpose")
                .cloned()
                .unwrap_or_else(|| JsonValue::from("")),
        ),
        (
            "horizon",
            direction
                .get("horizon")
                .cloned()
                .unwrap_or_else(|| JsonValue::from("years")),
        ),
        ("status", JsonValue::from(status)),
        (
            "health",
            direction
                .get("health")
                .cloned()
                .unwrap_or_else(|| JsonValue::from("unknown")),
        ),
        (
            "priority",
            direction
                .get("priority")
                .cloned()
                .unwrap_or_else(|| JsonValue::from("normal")),
        ),
        ("start_date", JsonValue::Null),
        ("deadline", JsonValue::Null),
        ("success_criteria", success_definition),
        (
            "constraints",
            direction
                .get("constraints")
                .cloned()
                .unwrap_or_else(|| JsonValue::Array(Vec::new())),
        ),
        ("progress", JsonValue::from(0_u64)),
        ("progress_basis", JsonValue::from("manual")),
        ("milestone_ids", JsonValue::Array(Vec::new())),
        ("last_progress_at", JsonValue::Null),
        ("next_review_at", next_review_at),
        ("next_action", JsonValue::from("")),
        ("blockers", JsonValue::Array(Vec::new())),
        ("risks", JsonValue::Array(Vec::new())),
        ("assumptions", JsonValue::Array(Vec::new())),
        (
            "created_at",
            direction
                .get("created_at")
                .cloned()
                .or_else(|| direction.get("updated_at").cloned())
                .unwrap_or_else(|| JsonValue::from(now_iso8601())),
        ),
        (
            "updated_at",
            direction
                .get("updated_at")
                .cloned()
                .unwrap_or_else(|| JsonValue::from(now_iso8601())),
        ),
    ]))
}

// ---- goals ------------------------------------------------------------------

pub fn handle_goal_create(
    store: &dyn DocumentStore,
    request: &Request,
) -> Result<JsonValue, FunctionFailure> {
    run_goal_mutation(store, request, "create_goal", |document, req, _actions| {
        check_fields(
            &req.input,
            GOAL_CREATE_FIELDS,
            &["title", "success_criteria"],
        )?;
        let is_group = match req.input.get("group") {
            Some(value) => value
                .as_bool()
                .ok_or_else(|| FunctionFailure::invalid_input("group must be a boolean"))?,
            None => false,
        };
        let timestamp = now_iso8601();
        let sequence = collection_items(document, "goals")?.len() as u64 + 1;
        let id = match req.input.get("id") {
            Some(value) => validate_identifier(value, "id")?,
            None => generate_id("goal", &req.request_id, sequence),
        };
        if has_record(document, "goals", &id) {
            return Err(FunctionFailure::invalid_input(format!(
                "goal id '{id}' already exists"
            )));
        }

        let mut record = object_of([
            ("id", JsonValue::from(id.clone())),
            ("parent_goal_ids", JsonValue::Array(Vec::new())),
            ("title", JsonValue::from("")),
            ("description", JsonValue::from("")),
            ("purpose", JsonValue::from("")),
            ("horizon", JsonValue::from("long")),
            ("status", JsonValue::from("active")),
            ("health", JsonValue::from("unknown")),
            ("priority", JsonValue::from("normal")),
            ("start_date", JsonValue::Null),
            ("deadline", JsonValue::Null),
            ("success_criteria", JsonValue::Array(Vec::new())),
            ("constraints", JsonValue::Array(Vec::new())),
            ("progress", JsonValue::from(0_u64)),
            ("progress_basis", JsonValue::from("manual")),
            ("milestone_ids", JsonValue::Array(Vec::new())),
            ("last_progress_at", JsonValue::Null),
            ("next_review_at", JsonValue::Null),
            ("next_action", JsonValue::from("")),
            ("blockers", JsonValue::Array(Vec::new())),
            ("risks", JsonValue::Array(Vec::new())),
            ("assumptions", JsonValue::Array(Vec::new())),
            ("created_at", JsonValue::from(timestamp.clone())),
            ("updated_at", JsonValue::from(timestamp)),
        ]);
        copy_payload(&mut record, &req.input, &["id", "group"]);
        validate_goal(&record)?;

        let parent_goal_ids = string_values(record.get("parent_goal_ids").unwrap());
        if is_group {
            if !parent_goal_ids.is_empty() {
                return Err(FunctionFailure::invalid_input(
                    "goal group must not have parent_goal_ids",
                ));
            }
        } else if parent_goal_ids.is_empty() {
            return Err(FunctionFailure::invalid_input(
                "goal must reference an existing parent goal; set group=true to create a goal group",
            ));
        }
        for value in parent_goal_ids {
            let parent_id = value.as_str().unwrap_or("");
            find_record(document, "goals", parent_id)?;
        }

        collection_items_mut(document, "goals")?.push(JsonValue::Object(record));
        Ok(build_result(
            req,
            vec![id.clone()],
            format!("created goal {id}"),
        ))
    })
}

pub fn handle_goal_update(
    store: &dyn DocumentStore,
    request: &Request,
) -> Result<JsonValue, FunctionFailure> {
    run_goal_mutation(store, request, "update_goal", |document, req, _actions| {
        check_fields(
            &req.input,
            &["goal_id", "patch", "recalculate_progress"],
            &["goal_id"],
        )?;
        let goal_id = validate_identifier(req.input.get("goal_id").unwrap(), "goal_id")?;
        let patch = req
            .input
            .get("patch")
            .and_then(JsonValue::as_object)
            .filter(|patch| !patch.is_empty());
        let recalculate = match req.input.get("recalculate_progress") {
            Some(value) => value.as_bool().ok_or_else(|| {
                FunctionFailure::invalid_input("recalculate_progress must be a boolean")
            })?,
            None => false,
        };
        if patch.is_none() && !recalculate {
            return Err(FunctionFailure::invalid_input(
                "provide a non-empty patch or recalculate_progress=true",
            ));
        }
        if let Some(patch) = patch {
            check_fields(patch, GOAL_PATCH_FIELDS, &[])?;
        }

        let (merged_parent_goal_ids, merged_milestone_ids) = {
            let current = find_record(document, "goals", &goal_id)?;
            let parent_goal_ids = patch
                .and_then(|patch| patch.get("parent_goal_ids"))
                .cloned()
                .unwrap_or_else(|| {
                    current
                        .get("parent_goal_ids")
                        .cloned()
                        .unwrap_or(JsonValue::Array(Vec::new()))
                });
            let milestone_ids = patch
                .and_then(|patch| patch.get("milestone_ids"))
                .cloned()
                .unwrap_or_else(|| {
                    current
                        .get("milestone_ids")
                        .cloned()
                        .unwrap_or(JsonValue::Array(Vec::new()))
                });
            (parent_goal_ids, milestone_ids)
        };

        for value in string_values(&merged_parent_goal_ids) {
            let parent_id = value.as_str().unwrap_or("");
            if parent_id == goal_id {
                return Err(FunctionFailure::invalid_input(
                    "goal must not be its own parent",
                ));
            }
            find_record(document, "goals", parent_id)?;
        }
        for value in string_values(&merged_milestone_ids) {
            let milestone_id = value.as_str().unwrap_or("");
            let milestone = find_record(document, "milestones", milestone_id)?;
            if milestone.get("goal_id").and_then(JsonValue::as_str) != Some(goal_id.as_str()) {
                return Err(FunctionFailure::invalid_input(format!(
                    "milestone {milestone_id} belongs to another goal"
                )));
            }
        }

        let recalculated = if recalculate {
            if !merged_milestone_ids
                .as_array()
                .is_some_and(|items| !items.is_empty())
            {
                return Err(FunctionFailure::invalid_input(
                    "goal has no milestones to recalculate from",
                ));
            }
            let mut sum = 0_u64;
            let mut count = 0_u64;
            for value in string_values(&merged_milestone_ids) {
                let milestone_id = value.as_str().unwrap_or("");
                let milestone = find_record(document, "milestones", milestone_id)?;
                sum += milestone
                    .get("progress")
                    .and_then(JsonValue::as_u64)
                    .unwrap_or(0);
                count += 1;
            }
            Some(((sum as f64) / (count as f64)).round() as u64)
        } else {
            None
        };

        let record = find_record_mut(document, "goals", &goal_id)?;
        if let Some(patch) = patch {
            copy_payload(record, patch, &[]);
        }
        if let Some(value) = recalculated {
            record.insert("progress".to_string(), JsonValue::from(value));
            record.insert(
                "progress_basis".to_string(),
                JsonValue::from("milestone_weighted"),
            );
        }
        record.insert("updated_at".to_string(), JsonValue::from(now_iso8601()));
        validate_goal(record)?;

        Ok(build_result(
            req,
            vec![goal_id.clone()],
            format!("updated goal {goal_id}"),
        ))
    })
}

// ---- milestones -------------------------------------------------------------

pub fn handle_milestone_create(
    store: &dyn DocumentStore,
    request: &Request,
) -> Result<JsonValue, FunctionFailure> {
    run_goal_mutation(
        store,
        request,
        "create_milestone",
        |document, req, actions| {
            check_fields(
                &req.input,
                MILESTONE_CREATE_FIELDS,
                &["goal_id", "title", "exit_criteria"],
            )?;
            let timestamp = now_iso8601();
            let goal_id = validate_identifier(req.input.get("goal_id").unwrap(), "goal_id")?;
            find_record(document, "goals", &goal_id)?;

            let next_sequence = collection_items(document, "milestones")?
                .iter()
                .filter_map(JsonValue::as_object)
                .filter(|record| {
                    record.get("goal_id").and_then(JsonValue::as_str) == Some(goal_id.as_str())
                })
                .filter_map(|record| record.get("sequence").and_then(JsonValue::as_u64))
                .max()
                .unwrap_or(0)
                + 1;

            let sequence = collection_items(document, "milestones")?.len() as u64 + 1;
            let id = match req.input.get("id") {
                Some(value) => validate_identifier(value, "id")?,
                None => generate_id("milestone", &req.request_id, sequence),
            };
            if has_record(document, "milestones", &id) {
                return Err(FunctionFailure::invalid_input(format!(
                    "milestone id '{id}' already exists"
                )));
            }

            let mut record = object_of([
                ("id", JsonValue::from(id.clone())),
                ("goal_id", JsonValue::from(goal_id.clone())),
                ("title", JsonValue::from("")),
                ("sequence", JsonValue::from(next_sequence)),
                ("status", JsonValue::from("planned")),
                ("health", JsonValue::from("unknown")),
                ("start_date", JsonValue::Null),
                ("due_at", JsonValue::Null),
                ("exit_criteria", JsonValue::Array(Vec::new())),
                ("progress", JsonValue::from(0_u64)),
                ("action_ids", JsonValue::Array(Vec::new())),
                ("dependencies", JsonValue::Array(Vec::new())),
                ("next_action", JsonValue::from("")),
                ("blockers", JsonValue::Array(Vec::new())),
                ("last_progress_at", JsonValue::Null),
                ("updated_at", JsonValue::from(timestamp.clone())),
            ]);
            copy_payload(&mut record, &req.input, &["id"]);
            validate_milestone(&record)?;
            ensure_actions_exist(actions, record.get("action_ids").unwrap())?;
            for value in string_values(record.get("dependencies").unwrap()) {
                let dependency = value.as_str().unwrap_or("");
                find_record(document, "milestones", dependency)?;
            }

            collection_items_mut(document, "milestones")?.push(JsonValue::Object(record));

            let goal = find_record_mut(document, "goals", &goal_id)?;
            let milestone_ids = goal
                .get_mut("milestone_ids")
                .and_then(JsonValue::as_array_mut)
                .ok_or_else(|| {
                    FunctionFailure::new("DATA_INVALID", "goal milestone_ids must be an array")
                })?;
            if !milestone_ids
                .iter()
                .any(|value| value.as_str() == Some(&id))
            {
                milestone_ids.push(JsonValue::from(id.clone()));
            }
            goal.insert("updated_at".to_string(), JsonValue::from(timestamp));
            sort_milestones_by_start(document)?;

            Ok(build_result(
                req,
                vec![id.clone()],
                format!("created milestone {id}"),
            ))
        },
    )
}

pub fn handle_milestone_update(
    store: &dyn DocumentStore,
    request: &Request,
) -> Result<JsonValue, FunctionFailure> {
    run_goal_mutation(
        store,
        request,
        "update_milestone",
        |document, req, actions| {
            check_fields(
                &req.input,
                &["milestone_id", "patch"],
                &["milestone_id", "patch"],
            )?;
            let milestone_id =
                validate_identifier(req.input.get("milestone_id").unwrap(), "milestone_id")?;
            let patch = req
                .input
                .get("patch")
                .and_then(JsonValue::as_object)
                .filter(|patch| !patch.is_empty())
                .ok_or_else(|| {
                    FunctionFailure::invalid_input("patch must be a non-empty JSON object")
                })?;
            check_fields(patch, MILESTONE_PATCH_FIELDS, &[])?;

            let old_goal_id = find_record(document, "milestones", &milestone_id)?
                .get("goal_id")
                .and_then(JsonValue::as_str)
                .unwrap_or("")
                .to_string();
            let new_goal_id = match patch.get("goal_id") {
                Some(value) => validate_identifier(value, "goal_id")?,
                None => old_goal_id.clone(),
            };
            find_record(document, "goals", &new_goal_id)?;

            if let Some(action_ids) = patch.get("action_ids") {
                validate_unique_string_array(action_ids, "action_ids", false)?;
                ensure_actions_exist(actions, action_ids)?;
            }
            if let Some(dependencies) = patch.get("dependencies") {
                validate_unique_string_array(dependencies, "dependencies", false)?;
                for value in string_values(dependencies) {
                    let dependency = value.as_str().unwrap_or("");
                    find_record(document, "milestones", dependency)?;
                }
            }

            let timestamp = now_iso8601();
            {
                let record = find_record_mut(document, "milestones", &milestone_id)?;
                copy_payload(record, patch, &[]);
                record.insert("updated_at".to_string(), JsonValue::from(timestamp.clone()));
                validate_milestone(record)?;
            }

            if new_goal_id != old_goal_id {
                let old_goal = find_record_mut(document, "goals", &old_goal_id)?;
                if let Some(milestone_ids) = old_goal
                    .get_mut("milestone_ids")
                    .and_then(JsonValue::as_array_mut)
                {
                    milestone_ids.retain(|value| value.as_str() != Some(milestone_id.as_str()));
                }
                old_goal.insert("updated_at".to_string(), JsonValue::from(timestamp.clone()));

                let new_goal = find_record_mut(document, "goals", &new_goal_id)?;
                let milestone_ids = new_goal
                    .get_mut("milestone_ids")
                    .and_then(JsonValue::as_array_mut)
                    .ok_or_else(|| {
                        FunctionFailure::new("DATA_INVALID", "goal milestone_ids must be an array")
                    })?;
                if !milestone_ids
                    .iter()
                    .any(|value| value.as_str() == Some(milestone_id.as_str()))
                {
                    milestone_ids.push(JsonValue::from(milestone_id.clone()));
                }
                new_goal.insert("updated_at".to_string(), JsonValue::from(timestamp));
            }
            sort_milestones_by_start(document)?;

            Ok(build_result(
                req,
                vec![milestone_id.clone()],
                format!("updated milestone {milestone_id}"),
            ))
        },
    )
}

// ---- checkpoints ----------------------------------------------------------

pub fn handle_checkpoint_create(
    store: &dyn DocumentStore,
    request: &Request,
) -> Result<JsonValue, FunctionFailure> {
    run_goal_mutation(
        store,
        request,
        "create_checkpoint",
        |document, req, _actions| {
            check_fields(
                &req.input,
                CHECKPOINT_CREATE_FIELDS,
                &["milestone_id", "title"],
            )?;
            let milestone_id =
                validate_identifier(req.input.get("milestone_id").unwrap(), "milestone_id")?;
            find_record(document, "milestones", &milestone_id)?;
            if !document.contains_key("checkpoints") {
                document.insert("checkpoints".to_string(), JsonValue::Array(Vec::new()));
            }

            let sequence = collection_items(document, "checkpoints")?.len() as u64 + 1;
            let id = match req.input.get("id") {
                Some(value) => validate_identifier(value, "id")?,
                None => generate_id("checkpoint", &req.request_id, sequence),
            };
            if has_record(document, "checkpoints", &id) {
                return Err(FunctionFailure::invalid_input(format!(
                    "checkpoint id '{id}' already exists"
                )));
            }

            let timestamp = now_iso8601();
            let mut record = object_of([
                ("id", JsonValue::from(id.clone())),
                ("milestone_id", JsonValue::from(milestone_id)),
                ("title", JsonValue::from("")),
                ("description", JsonValue::from("")),
                ("status", JsonValue::from("planned")),
                ("estimate_minutes", JsonValue::from(60_u64)),
                ("created_at", JsonValue::from(timestamp.clone())),
                ("updated_at", JsonValue::from(timestamp)),
            ]);
            copy_payload(&mut record, &req.input, &["id"]);
            validate_checkpoint(&record)?;

            collection_items_mut(document, "checkpoints")?.push(JsonValue::Object(record));

            Ok(build_result(
                req,
                vec![id.clone()],
                format!("created checkpoint {id}"),
            ))
        },
    )
}

pub fn handle_checkpoint_update(
    store: &dyn DocumentStore,
    request: &Request,
) -> Result<JsonValue, FunctionFailure> {
    run_goal_mutation(
        store,
        request,
        "update_checkpoint",
        |document, req, _actions| {
            check_fields(
                &req.input,
                &["checkpoint_id", "patch"],
                &["checkpoint_id", "patch"],
            )?;
            let checkpoint_id =
                validate_identifier(req.input.get("checkpoint_id").unwrap(), "checkpoint_id")?;
            let patch = req
                .input
                .get("patch")
                .and_then(JsonValue::as_object)
                .filter(|patch| !patch.is_empty())
                .ok_or_else(|| {
                    FunctionFailure::invalid_input("patch must be a non-empty JSON object")
                })?;
            check_fields(patch, CHECKPOINT_PATCH_FIELDS, &[])?;

            if let Some(value) = patch.get("milestone_id") {
                let milestone_id = validate_identifier(value, "milestone_id")?;
                find_record(document, "milestones", &milestone_id)?;
            }
            if !document.contains_key("checkpoints") {
                document.insert("checkpoints".to_string(), JsonValue::Array(Vec::new()));
            }

            let record = find_record_mut(document, "checkpoints", &checkpoint_id)?;
            copy_payload(record, patch, &[]);
            record.insert("updated_at".to_string(), JsonValue::from(now_iso8601()));
            validate_checkpoint(record)?;

            Ok(build_result(
                req,
                vec![checkpoint_id.clone()],
                format!("updated checkpoint {checkpoint_id}"),
            ))
        },
    )
}

pub fn handle_goal_delete(
    store: &dyn DocumentStore,
    request: &Request,
) -> Result<JsonValue, FunctionFailure> {
    run_goal_mutation(store, request, "delete_goal", |document, req, _actions| {
        check_fields(&req.input, &["goal_id"], &["goal_id"])?;
        let goal_id = validate_identifier(req.input.get("goal_id").unwrap(), "goal_id")?;
        find_record(document, "goals", &goal_id)?;

        let mut goal_ids: BTreeSet<String> = BTreeSet::new();
        goal_ids.insert(goal_id.clone());
        loop {
            let mut added = false;
            for item in collection_items(document, "goals")? {
                let record = item.as_object().ok_or_else(|| {
                    FunctionFailure::new("DATA_INVALID", "goals entries must be objects")
                })?;
                let id = record
                    .get("id")
                    .and_then(JsonValue::as_str)
                    .unwrap_or_default();
                if id.is_empty() || goal_ids.contains(id) {
                    continue;
                }
                let has_deleted_parent = record
                    .get("parent_goal_ids")
                    .and_then(JsonValue::as_array)
                    .map(|parents| {
                        parents
                            .iter()
                            .filter_map(JsonValue::as_str)
                            .any(|parent| goal_ids.contains(parent))
                    })
                    .unwrap_or(false);
                if has_deleted_parent {
                    goal_ids.insert(id.to_string());
                    added = true;
                }
            }
            if !added {
                break;
            }
        }

        let milestone_ids: BTreeSet<String> = collection_items(document, "milestones")?
            .iter()
            .filter_map(JsonValue::as_object)
            .filter(|record| {
                record
                    .get("goal_id")
                    .and_then(JsonValue::as_str)
                    .is_some_and(|goal| goal_ids.contains(goal))
            })
            .filter_map(|record| record.get("id").and_then(JsonValue::as_str))
            .map(str::to_string)
            .collect();
        let checkpoint_ids: BTreeSet<String> = collection_items(document, "checkpoints")?
            .iter()
            .filter_map(JsonValue::as_object)
            .filter(|record| {
                record
                    .get("milestone_id")
                    .and_then(JsonValue::as_str)
                    .is_some_and(|milestone| milestone_ids.contains(milestone))
            })
            .filter_map(|record| record.get("id").and_then(JsonValue::as_str))
            .map(str::to_string)
            .collect();

        let mut removed_links = 0usize;
        collection_items_mut(document, "action_links")?.retain(|item| {
            let remove = item.as_object().is_some_and(|record| {
                record
                    .get("goal_id")
                    .and_then(JsonValue::as_str)
                    .is_some_and(|goal| goal_ids.contains(goal))
                    || record
                        .get("goal_ids")
                        .and_then(JsonValue::as_array)
                        .is_some_and(|goals| {
                            goals
                                .iter()
                                .filter_map(JsonValue::as_str)
                                .any(|goal| goal_ids.contains(goal))
                        })
                    || record
                        .get("milestone_id")
                        .and_then(JsonValue::as_str)
                        .is_some_and(|milestone| milestone_ids.contains(milestone))
                    || record
                        .get("checkpoint_id")
                        .and_then(JsonValue::as_str)
                        .is_some_and(|checkpoint| checkpoint_ids.contains(checkpoint))
            });
            if remove {
                removed_links += 1;
            }
            !remove
        });

        let removed_checkpoints = remove_records_by_id(document, "checkpoints", &checkpoint_ids)?;
        let removed_milestones = remove_records_by_id(document, "milestones", &milestone_ids)?;
        let removed_goals = remove_records_by_id(document, "goals", &goal_ids)?;

        for item in collection_items_mut(document, "milestones")? {
            if let Some(record) = item.as_object_mut()
                && let Some(dependencies) = record
                    .get_mut("dependencies")
                    .and_then(JsonValue::as_array_mut)
            {
                dependencies.retain(|value| {
                    value
                        .as_str()
                        .is_none_or(|dependency| !milestone_ids.contains(dependency))
                });
            }
        }
        for item in collection_items_mut(document, "goals")? {
            if let Some(record) = item.as_object_mut() {
                if let Some(parents) = record
                    .get_mut("parent_goal_ids")
                    .and_then(JsonValue::as_array_mut)
                {
                    parents.retain(|value| {
                        value
                            .as_str()
                            .is_none_or(|parent| !goal_ids.contains(parent))
                    });
                }
                if let Some(milestones) = record
                    .get_mut("milestone_ids")
                    .and_then(JsonValue::as_array_mut)
                {
                    milestones.retain(|value| {
                        value
                            .as_str()
                            .is_none_or(|milestone| !milestone_ids.contains(milestone))
                    });
                }
            }
        }

        Ok(build_result(
            req,
            vec![goal_id.clone()],
            format!(
                "deleted goal subtree {goal_id}: {removed_goals} goals, {removed_milestones} milestones, {removed_checkpoints} checkpoints, {removed_links} action links"
            ),
        ))
    })
}

pub fn handle_milestone_delete(
    store: &dyn DocumentStore,
    request: &Request,
) -> Result<JsonValue, FunctionFailure> {
    run_goal_mutation(
        store,
        request,
        "delete_milestone",
        |document, req, _actions| {
            check_fields(&req.input, &["milestone_id"], &["milestone_id"])?;
            let milestone_id =
                validate_identifier(req.input.get("milestone_id").unwrap(), "milestone_id")?;
            find_record(document, "milestones", &milestone_id)?;

            let checkpoint_ids: BTreeSet<String> = collection_items(document, "checkpoints")?
                .iter()
                .filter_map(JsonValue::as_object)
                .filter(|record| {
                    record
                        .get("milestone_id")
                        .and_then(JsonValue::as_str)
                        .is_some_and(|milestone| milestone == milestone_id)
                })
                .filter_map(|record| record.get("id").and_then(JsonValue::as_str))
                .map(str::to_string)
                .collect();

            let mut removed_links = 0usize;
            collection_items_mut(document, "action_links")?.retain(|item| {
                let remove = item.as_object().is_some_and(|record| {
                    record
                        .get("milestone_id")
                        .and_then(JsonValue::as_str)
                        .is_some_and(|milestone| milestone == milestone_id)
                        || record
                            .get("checkpoint_id")
                            .and_then(JsonValue::as_str)
                            .is_some_and(|checkpoint| checkpoint_ids.contains(checkpoint))
                });
                if remove {
                    removed_links += 1;
                }
                !remove
            });

            let removed_checkpoints =
                remove_records_by_id(document, "checkpoints", &checkpoint_ids)?;
            let deleted = remove_records_by_id(
                document,
                "milestones",
                &BTreeSet::from([milestone_id.clone()]),
            )?;

            for item in collection_items_mut(document, "goals")? {
                if let Some(record) = item.as_object_mut()
                    && let Some(milestone_ids) = record
                        .get_mut("milestone_ids")
                        .and_then(JsonValue::as_array_mut)
                {
                    milestone_ids.retain(|value| value.as_str() != Some(milestone_id.as_str()));
                }
            }
            for item in collection_items_mut(document, "milestones")? {
                if let Some(record) = item.as_object_mut()
                    && let Some(dependencies) = record
                        .get_mut("dependencies")
                        .and_then(JsonValue::as_array_mut)
                {
                    dependencies.retain(|value| value.as_str() != Some(milestone_id.as_str()));
                }
            }
            sort_milestones_by_start(document)?;

            Ok(build_result(
                req,
                vec![milestone_id.clone()],
                format!(
                    "deleted milestone {milestone_id}: {deleted} milestone, {removed_checkpoints} checkpoints, {removed_links} action links"
                ),
            ))
        },
    )
}

pub fn handle_checkpoint_delete(
    store: &dyn DocumentStore,
    request: &Request,
) -> Result<JsonValue, FunctionFailure> {
    run_goal_mutation(
        store,
        request,
        "delete_checkpoint",
        |document, req, _actions| {
            check_fields(&req.input, &["checkpoint_id"], &["checkpoint_id"])?;
            let checkpoint_id =
                validate_identifier(req.input.get("checkpoint_id").unwrap(), "checkpoint_id")?;
            find_record(document, "checkpoints", &checkpoint_id)?;

            let mut removed_links = 0usize;
            collection_items_mut(document, "action_links")?.retain(|item| {
                let remove = item.as_object().is_some_and(|record| {
                    record
                        .get("checkpoint_id")
                        .and_then(JsonValue::as_str)
                        .is_some_and(|checkpoint| checkpoint == checkpoint_id)
                });
                if remove {
                    removed_links += 1;
                }
                !remove
            });
            remove_records_by_id(
                document,
                "checkpoints",
                &BTreeSet::from([checkpoint_id.clone()]),
            )?;

            Ok(build_result(
                req,
                vec![checkpoint_id.clone()],
                format!("deleted checkpoint {checkpoint_id}: {removed_links} action links"),
            ))
        },
    )
}

fn remove_records_by_id(
    document: &mut BTreeMap<String, JsonValue>,
    collection: &str,
    ids: &BTreeSet<String>,
) -> Result<usize, FunctionFailure> {
    let items = collection_items_mut(document, collection)?;
    let before = items.len();
    items.retain(|item| {
        item.get("id")
            .and_then(JsonValue::as_str)
            .is_none_or(|id| !ids.contains(id))
    });
    Ok(before - items.len())
}

// ---- action links -------------------------------------------------------------

pub fn handle_action_link_set(
    store: &dyn DocumentStore,
    request: &Request,
) -> Result<JsonValue, FunctionFailure> {
    run_goal_mutation(
        store,
        request,
        "set_action_link",
        |document, req, actions| {
            check_fields(&req.input, ACTION_LINK_FIELDS, &["action_id"])?;
            let action_id = validate_identifier(req.input.get("action_id").unwrap(), "action_id")?;
            ensure_actions_exist(
                actions,
                &JsonValue::Array(vec![JsonValue::from(action_id.clone())]),
            )?;

            let goal_ids = req
                .input
                .get("goal_ids")
                .cloned()
                .unwrap_or(JsonValue::Array(Vec::new()));
            let goal_id = req.input.get("goal_id").cloned().unwrap_or(JsonValue::Null);
            let milestone_id = req
                .input
                .get("milestone_id")
                .cloned()
                .unwrap_or(JsonValue::Null);
            let checkpoint_id = req
                .input
                .get("checkpoint_id")
                .cloned()
                .unwrap_or(JsonValue::Null);
            let role = req
                .input
                .get("role")
                .cloned()
                .unwrap_or_else(|| JsonValue::from("core"));
            let contribution = req
                .input
                .get("contribution")
                .cloned()
                .unwrap_or_else(|| JsonValue::from(""));

            let record = object_of([
                ("action_id", JsonValue::from(action_id.clone())),
                ("goal_ids", goal_ids),
                ("goal_id", goal_id),
                ("milestone_id", milestone_id),
                ("checkpoint_id", checkpoint_id),
                ("role", role),
                ("contribution", contribution),
            ]);
            validate_action_link(&record)?;

            let record_goal_ids = record.get("goal_ids").unwrap();
            if string_values(record_goal_ids).is_empty() {
                let goal_id = record.get("goal_id").unwrap();
                let milestone_id = record.get("milestone_id").unwrap();
                let checkpoint_id = record.get("checkpoint_id").unwrap();
                if goal_id.is_null() && milestone_id.is_null() && checkpoint_id.is_null() {
                    return Err(FunctionFailure::invalid_input(
                        "action link must reference a goal, milestone, or checkpoint",
                    ));
                }
            }

            for value in string_values(record_goal_ids) {
                let linked_goal_id = value.as_str().unwrap_or("");
                find_record(document, "goals", linked_goal_id)?;
            }
            if let Some(goal_id) = record.get("goal_id").and_then(JsonValue::as_str) {
                find_record(document, "goals", goal_id)?;
            }
            if let Some(milestone_id) = record.get("milestone_id").and_then(JsonValue::as_str) {
                let milestone = find_record(document, "milestones", milestone_id)?;
                if let Some(goal_id) = record.get("goal_id").and_then(JsonValue::as_str)
                    && milestone.get("goal_id").and_then(JsonValue::as_str) != Some(goal_id)
                {
                    return Err(FunctionFailure::invalid_input(format!(
                        "milestone {milestone_id} belongs to another goal"
                    )));
                }
            }
            if let Some(checkpoint_id) = record.get("checkpoint_id").and_then(JsonValue::as_str) {
                let current = find_record(document, "checkpoints", checkpoint_id)?;
                let owner_milestone = current
                    .get("milestone_id")
                    .and_then(JsonValue::as_str)
                    .unwrap_or("");
                if let Some(milestone_id) = record.get("milestone_id").and_then(JsonValue::as_str)
                    && milestone_id != owner_milestone
                {
                    return Err(FunctionFailure::invalid_input(format!(
                        "checkpoint {checkpoint_id} belongs to another milestone"
                    )));
                }
                if let Some(goal_id) = record.get("goal_id").and_then(JsonValue::as_str) {
                    let milestone = find_record(document, "milestones", owner_milestone)?;
                    if milestone.get("goal_id").and_then(JsonValue::as_str) != Some(goal_id) {
                        return Err(FunctionFailure::invalid_input(format!(
                            "checkpoint {checkpoint_id} belongs to another goal"
                        )));
                    }
                }
            }

            let links = collection_items_mut(document, "action_links")?;
            if let Some(existing) = links.iter_mut().find(|item| {
                item.get("action_id").and_then(JsonValue::as_str) == Some(action_id.as_str())
            }) {
                *existing = JsonValue::Object(record);
            } else {
                links.push(JsonValue::Object(record));
            }

            Ok(build_result(
                req,
                vec![action_id.clone()],
                format!("linked action {action_id} to life planning"),
            ))
        },
    )
}

pub fn handle_action_link_remove(
    store: &dyn DocumentStore,
    request: &Request,
) -> Result<JsonValue, FunctionFailure> {
    run_goal_mutation(
        store,
        request,
        "remove_action_link",
        |document, req, _actions| {
            check_fields(&req.input, &["action_id"], &["action_id"])?;
            let action_id = validate_identifier(req.input.get("action_id").unwrap(), "action_id")?;

            let links = collection_items_mut(document, "action_links")?;
            let position = links.iter().position(|item| {
                item.get("action_id").and_then(JsonValue::as_str) == Some(action_id.as_str())
            });
            match position {
                Some(index) => {
                    links.remove(index);
                }
                None => {
                    return Err(FunctionFailure::not_found(format!(
                        "action link {action_id} was not found"
                    )));
                }
            }

            Ok(build_result(
                req,
                vec![action_id.clone()],
                format!("removed action link {action_id}"),
            ))
        },
    )
}

// ---- review -----------------------------------------------------------------

pub fn handle_review_set(
    store: &dyn DocumentStore,
    request: &Request,
) -> Result<JsonValue, FunctionFailure> {
    run_goal_mutation(store, request, "set_review", |document, req, _actions| {
        check_fields(&req.input, REVIEW_FIELDS, &[])?;
        if req.input.is_empty() {
            return Err(FunctionFailure::invalid_input(
                "provide cadence, next_review_at, or both",
            ));
        }

        let review = document
            .get_mut("review")
            .and_then(JsonValue::as_object_mut)
            .ok_or_else(|| FunctionFailure::new("DATA_INVALID", "review must be an object"))?;
        copy_payload(review, &req.input, &[]);
        validate_review(review)?;

        Ok(build_result(
            req,
            vec!["review".to_string()],
            "updated review cadence".to_string(),
        ))
    })
}

// ---- progress evidence ------------------------------------------------------

pub fn handle_progress_record(
    store: &dyn DocumentStore,
    request: &Request,
) -> Result<JsonValue, FunctionFailure> {
    run_goal_mutation(
        store,
        request,
        "record_progress",
        |document, req, _actions| {
            check_fields(
                &req.input,
                PROGRESS_RECORD_FIELDS,
                &["target", "target_id", "kind", "summary", "progress_after"],
            )?;
            let target =
                require_choice(req.input.get("target").unwrap(), PROGRESS_TARGETS, "target")?;
            let target_id = validate_identifier(req.input.get("target_id").unwrap(), "target_id")?;
            let at = match req.input.get("at") {
                Some(value) => {
                    let text = require_non_empty_str(value, "at")?;
                    validate_instant(text, "at")?;
                    text.to_string()
                }
                None => now_iso8601(),
            };
            let kind = require_non_empty_str(req.input.get("kind").unwrap(), "kind")?.to_string();
            let summary =
                require_non_empty_str(req.input.get("summary").unwrap(), "summary")?.to_string();
            let evidence = match req.input.get("evidence") {
                Some(value) => require_string(value, "evidence")?.to_string(),
                None => String::new(),
            };
            let progress_after =
                validate_progress(req.input.get("progress_after").unwrap(), "progress_after")?;

            let event = object_of([
                ("at", JsonValue::from(at.clone())),
                ("kind", JsonValue::from(kind)),
                ("summary", JsonValue::from(summary)),
                ("evidence", JsonValue::from(evidence)),
                ("progress_after", JsonValue::from(progress_after)),
            ]);

            let collection = if target == "goal" {
                "goals"
            } else {
                "milestones"
            };
            let record = find_record_mut(document, collection, &target_id)?;
            {
                let events = record
                    .entry("progress_events".to_string())
                    .or_insert_with(|| JsonValue::Array(Vec::new()))
                    .as_array_mut()
                    .ok_or_else(|| {
                        FunctionFailure::new("DATA_INVALID", "progress_events must be an array")
                    })?;
                events.push(JsonValue::Object(event));
            }
            record.insert("progress".to_string(), JsonValue::from(progress_after));
            record.insert("last_progress_at".to_string(), JsonValue::from(at));
            if target == "goal" {
                record.insert("progress_basis".to_string(), JsonValue::from("manual"));
            }
            record.insert("updated_at".to_string(), JsonValue::from(now_iso8601()));

            Ok(build_result(
                req,
                vec![target_id.clone()],
                format!("recorded progress for {target} {target_id}"),
            ))
        },
    )
}
