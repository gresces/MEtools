//! `health` domain state validation, persistence, and command handlers.

use std::collections::{BTreeMap, BTreeSet};

use shared::{
    Document, DocumentStore, FunctionFailure, JsonValue, MutateOutcome, Request, check_fields,
    generate_id, now_iso8601, validate_hh_mm, validate_instant, validate_iso_date,
};

pub const SCHEMA_VERSION: u64 = 1;
const BLOOD_TYPES: &[&str] = &[
    "A", "B", "AB", "O", "A+", "A-", "B+", "B-", "AB+", "AB-", "O+", "O-", "unknown",
];
const DAYS_OF_WEEK: &[&str] = &["mon", "tue", "wed", "thu", "fri", "sat", "sun"];
const MEAL_TYPES: &[&str] = &["breakfast", "lunch", "dinner", "snack"];

const BODY_PROFILE_FIELDS: &[&str] = &[
    "height_cm",
    "target_weight_kg",
    "blood_type",
    "allergies",
    "conditions",
    "notes",
];

const BODY_RECORD_FIELDS: &[&str] = &[
    "date",
    "weight_kg",
    "body_fat_pct",
    "heart_rate_bpm",
    "systolic_bp",
    "diastolic_bp",
    "energy_level",
    "symptoms",
    "notes",
];

const WATER_RECORD_FIELDS: &[&str] = &["date", "amount_ml", "beverage", "recorded_at", "notes"];

const SLEEP_GOAL_FIELDS: &[&str] = &[
    "target_duration_minutes",
    "target_bedtime",
    "target_wake_time",
];

const SLEEP_RECORD_FIELDS: &[&str] = &[
    "date",
    "slept_at",
    "woke_at",
    "duration_minutes",
    "quality_score",
    "deep_sleep_minutes",
    "awakenings",
    "notes",
];

// ---- default_document -------------------------------------------------------

/// Construct a fresh `health.json` document with default baseline sections.
pub fn default_document(timestamp: &str) -> BTreeMap<String, JsonValue> {
    let body_profile = JsonValue::object([
        ("height_cm", JsonValue::Null),
        ("target_weight_kg", JsonValue::Null),
        ("blood_type", JsonValue::Null),
        ("allergies", JsonValue::Array(Vec::new())),
        ("conditions", JsonValue::Array(Vec::new())),
        ("notes", JsonValue::from("")),
        ("updated_at", JsonValue::from(timestamp)),
    ]);

    let body = JsonValue::object([
        ("profile", body_profile),
        ("records", JsonValue::Array(Vec::new())),
    ]);

    let water = JsonValue::object([
        ("daily_goal_ml", JsonValue::from(2000_u64)),
        ("updated_at", JsonValue::from(timestamp)),
        ("records", JsonValue::Array(Vec::new())),
    ]);

    let diet = JsonValue::object([("plans", JsonValue::Array(Vec::new()))]);

    let sleep_goal = JsonValue::object([
        ("target_duration_minutes", JsonValue::from(480_u64)),
        ("target_bedtime", JsonValue::from("23:00")),
        ("target_wake_time", JsonValue::from("07:00")),
        ("updated_at", JsonValue::from(timestamp)),
    ]);

    let sleep = JsonValue::object([
        ("goal", sleep_goal),
        ("records", JsonValue::Array(Vec::new())),
    ]);

    let JsonValue::Object(map) = JsonValue::object([
        ("schema_version", JsonValue::from(SCHEMA_VERSION)),
        ("version", JsonValue::from(1_u64)),
        ("updated_at", JsonValue::from(timestamp)),
        ("updated_by", JsonValue::from("init")),
        ("body", body),
        ("water", water),
        ("diet", diet),
        ("sleep", sleep),
        ("recent_requests", JsonValue::Array(Vec::new())),
    ]) else {
        unreachable!()
    };
    map
}

// ---- validate_document ------------------------------------------------------

/// Validate the top-level envelope and sections of `health.json`.
pub fn validate_document(doc: &BTreeMap<String, JsonValue>) -> Result<u64, FunctionFailure> {
    let schema = doc
        .get("schema_version")
        .and_then(JsonValue::as_u64)
        .ok_or_else(|| FunctionFailure::new("DATA_INVALID", "invalid or missing schema_version"))?;
    if schema != SCHEMA_VERSION {
        return Err(FunctionFailure::new(
            "DATA_INVALID",
            format!("unsupported schema_version={schema}; expected {SCHEMA_VERSION}"),
        ));
    }

    let version = doc
        .get("version")
        .and_then(JsonValue::as_u64)
        .filter(|v| *v >= 1)
        .ok_or_else(|| FunctionFailure::new("DATA_INVALID", "invalid document version"))?;

    let body = doc
        .get("body")
        .and_then(JsonValue::as_object)
        .ok_or_else(|| FunctionFailure::new("DATA_INVALID", "missing or invalid body section"))?;
    if !body.get("profile").is_some_and(|v| v.as_object().is_some())
        || !body.get("records").is_some_and(|v| v.as_array().is_some())
    {
        return Err(FunctionFailure::new(
            "DATA_INVALID",
            "body section requires profile object and records array",
        ));
    }

    let water = doc
        .get("water")
        .and_then(JsonValue::as_object)
        .ok_or_else(|| FunctionFailure::new("DATA_INVALID", "missing or invalid water section"))?;
    if !water
        .get("daily_goal_ml")
        .and_then(JsonValue::as_u64)
        .is_some_and(|v| (100..=15_000).contains(&v))
        || !water.get("records").is_some_and(|v| v.as_array().is_some())
    {
        return Err(FunctionFailure::new(
            "DATA_INVALID",
            "water section requires valid daily_goal_ml and records array",
        ));
    }

    let diet = doc
        .get("diet")
        .and_then(JsonValue::as_object)
        .ok_or_else(|| FunctionFailure::new("DATA_INVALID", "missing or invalid diet section"))?;
    if !diet.get("plans").is_some_and(|v| v.as_array().is_some()) {
        return Err(FunctionFailure::new(
            "DATA_INVALID",
            "diet section requires plans array",
        ));
    }

    let sleep = doc
        .get("sleep")
        .and_then(JsonValue::as_object)
        .ok_or_else(|| FunctionFailure::new("DATA_INVALID", "missing or invalid sleep section"))?;
    if !sleep.get("goal").is_some_and(|v| v.as_object().is_some())
        || !sleep.get("records").is_some_and(|v| v.as_array().is_some())
    {
        return Err(FunctionFailure::new(
            "DATA_INVALID",
            "sleep section requires goal object and records array",
        ));
    }

    Ok(version)
}

// ---- handle_init ------------------------------------------------------------

/// Initialize the `health` collection if missing, or verify it if already present.
pub fn handle_init(store: &dyn DocumentStore) -> Result<JsonValue, FunctionFailure> {
    if let Some(document) = store.load_optional("health")? {
        let version = validate_document(&document)?;
        return Ok(JsonValue::object([
            ("status", JsonValue::from("succeeded")),
            ("request_id", JsonValue::from("init")),
            ("changed_records", JsonValue::Array(Vec::new())),
            (
                "new_versions",
                JsonValue::object([("health", JsonValue::from(version))]),
            ),
            ("collection_version", JsonValue::from(version)),
            (
                "summary",
                JsonValue::from("health.json already initialized"),
            ),
        ]));
    }

    let timestamp = now_iso8601();
    let document = default_document(&timestamp);
    store.insert_initial("health", &document)?;

    Ok(JsonValue::object([
        ("status", JsonValue::from("succeeded")),
        ("request_id", JsonValue::from("init")),
        ("changed_records", JsonValue::string_array(&["health"])),
        (
            "new_versions",
            JsonValue::object([("health", JsonValue::from(1_u64))]),
        ),
        ("collection_version", JsonValue::from(1_u64)),
        ("summary", JsonValue::from("initialized health.json")),
    ]))
}

// ---- run_health_mutation ----------------------------------------------------

/// Execute a transactional, replay-aware mutation on the `health` collection.
fn run_health_mutation<F>(
    store: &dyn DocumentStore,
    request: &Request,
    operation: &str,
    mutate: F,
) -> Result<JsonValue, FunctionFailure>
where
    F: Fn(&mut Document, &Request) -> Result<JsonValue, FunctionFailure>,
{
    let outcome = store.with_document_mut("health", request, operation, &mut |document| {
        let current_doc_version = validate_document(document)?;
        let mut result = mutate(document, request)?;

        if let Some(res_obj) = result.as_object_mut() {
            res_obj.insert(
                "collection_version".to_string(),
                JsonValue::from(current_doc_version + 1),
            );
            if let Some(versions) = res_obj
                .get_mut("new_versions")
                .and_then(JsonValue::as_object_mut)
                && versions.contains_key("health")
            {
                versions.insert(
                    "health".to_string(),
                    JsonValue::from(current_doc_version + 1),
                );
            }
        }

        Ok(result)
    })?;

    match outcome {
        MutateOutcome::Replayed(value) => Ok(value),
        MutateOutcome::Applied { result, .. } => Ok(result),
    }
}

fn check_top_version(
    doc: &BTreeMap<String, JsonValue>,
    request: &Request,
) -> Result<(), FunctionFailure> {
    let current = doc.get("version").and_then(JsonValue::as_u64).unwrap_or(0);
    if current != request.expected_version {
        return Err(FunctionFailure::version_conflict(format!(
            "health version conflict: expected {}, current is {current}",
            request.expected_version
        )));
    }
    Ok(())
}

// ---- validation helpers -----------------------------------------------------

fn require_non_empty_str<'a>(
    value: &'a JsonValue,
    field: &str,
) -> Result<&'a str, FunctionFailure> {
    value
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            FunctionFailure::invalid_input(format!("{field} must be a non-empty string"))
        })
}

fn require_string<'a>(value: &'a JsonValue, field: &str) -> Result<&'a str, FunctionFailure> {
    value
        .as_str()
        .ok_or_else(|| FunctionFailure::invalid_input(format!("{field} must be a string")))
}

fn validate_string_list(value: &JsonValue, field: &str) -> Result<JsonValue, FunctionFailure> {
    let arr = value
        .as_array()
        .ok_or_else(|| FunctionFailure::invalid_input(format!("{field} must be an array")))?;
    let mut cleaned = Vec::with_capacity(arr.len());
    for item in arr {
        let s = require_non_empty_str(item, field)?;
        cleaned.push(JsonValue::from(s));
    }
    Ok(JsonValue::Array(cleaned))
}

fn validate_range_f64(
    value: &JsonValue,
    field: &str,
    min: f64,
    max: f64,
) -> Result<f64, FunctionFailure> {
    let num = value
        .as_f64()
        .ok_or_else(|| FunctionFailure::invalid_input(format!("{field} must be a number")))?;
    if !num.is_finite() || num < min || num > max {
        return Err(FunctionFailure::invalid_input(format!(
            "{field} must be between {min} and {max}"
        )));
    }
    Ok(num)
}

fn validate_range_u64(
    value: &JsonValue,
    field: &str,
    min: u64,
    max: u64,
) -> Result<u64, FunctionFailure> {
    let num = value.as_u64().ok_or_else(|| {
        FunctionFailure::invalid_input(format!("{field} must be a non-negative integer"))
    })?;
    if num < min || num > max {
        return Err(FunctionFailure::invalid_input(format!(
            "{field} must be between {min} and {max}"
        )));
    }
    Ok(num)
}

fn validate_source_or_default(
    payload: &BTreeMap<String, JsonValue>,
) -> Result<JsonValue, FunctionFailure> {
    let Some(source_val) = payload.get("source") else {
        return Ok(JsonValue::object([(
            "type",
            JsonValue::from("user_instruction"),
        )]));
    };
    let obj = source_val
        .as_object()
        .ok_or_else(|| FunctionFailure::invalid_input("source must be an object"))?;
    check_fields(obj, &["type", "reference"], &["type"])?;
    let source_type = require_non_empty_str(obj.get("type").unwrap(), "source.type")?;
    let mut cleaned = BTreeMap::new();
    cleaned.insert("type".to_string(), JsonValue::from(source_type));
    if let Some(reference) = obj.get("reference") {
        let ref_str = require_non_empty_str(reference, "source.reference")?;
        cleaned.insert("reference".to_string(), JsonValue::from(ref_str));
    }
    Ok(JsonValue::Object(cleaned))
}

// ---- 1. Body Status handlers ------------------------------------------------

pub fn handle_body_set_profile(
    store: &dyn DocumentStore,
    request: &Request,
) -> Result<JsonValue, FunctionFailure> {
    run_health_mutation(store, request, "body.set_profile", |doc, req| {
        check_fields(&req.input, &["patch"], &["patch"])?;
        check_top_version(doc, req)?;

        let patch = req
            .input
            .get("patch")
            .and_then(JsonValue::as_object)
            .filter(|m| !m.is_empty())
            .ok_or_else(|| {
                FunctionFailure::invalid_input("patch must be a non-empty JSON object")
            })?;
        check_fields(patch, BODY_PROFILE_FIELDS, &[])?;

        let body_obj = doc
            .get_mut("body")
            .and_then(JsonValue::as_object_mut)
            .unwrap();
        let profile = body_obj
            .get_mut("profile")
            .and_then(JsonValue::as_object_mut)
            .unwrap();

        for (key, value) in patch {
            match key.as_str() {
                "height_cm" => {
                    if !value.is_null() {
                        validate_range_f64(value, "height_cm", 30.0, 260.0)?;
                    }
                    profile.insert(key.clone(), value.clone());
                }
                "target_weight_kg" => {
                    if !value.is_null() {
                        validate_range_f64(value, "target_weight_kg", 15.0, 350.0)?;
                    }
                    profile.insert(key.clone(), value.clone());
                }
                "blood_type" => {
                    if !value.is_null() {
                        let bt = require_non_empty_str(value, "blood_type")?;
                        if !BLOOD_TYPES.contains(&bt) {
                            return Err(FunctionFailure::invalid_input(format!(
                                "blood_type must be one of: {}",
                                BLOOD_TYPES.join(", ")
                            )));
                        }
                        profile.insert(key.clone(), JsonValue::from(bt));
                    } else {
                        profile.insert(key.clone(), JsonValue::Null);
                    }
                }
                "allergies" | "conditions" => {
                    let list = validate_string_list(value, key)?;
                    profile.insert(key.clone(), list);
                }
                "notes" => {
                    let s = require_string(value, "notes")?;
                    profile.insert(key.clone(), JsonValue::from(s));
                }
                _ => unreachable!(),
            }
        }

        profile.insert("updated_at".to_string(), JsonValue::from(now_iso8601()));

        Ok(JsonValue::object([
            ("status", JsonValue::from("succeeded")),
            ("request_id", JsonValue::from(req.request_id.clone())),
            (
                "changed_records",
                JsonValue::string_array(&["body.profile"]),
            ),
            (
                "new_versions",
                JsonValue::object([("health", JsonValue::from(0_u64))]),
            ),
            ("summary", JsonValue::from("updated body profile")),
        ]))
    })
}

fn validate_body_record(record: &BTreeMap<String, JsonValue>) -> Result<(), FunctionFailure> {
    let date_str = record
        .get("date")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| FunctionFailure::invalid_input("date is required"))?;
    validate_iso_date(date_str, "date")?;

    let mut has_metric = false;
    if let Some(val) = record.get("weight_kg")
        && !val.is_null()
    {
        validate_range_f64(val, "weight_kg", 15.0, 350.0)?;
        has_metric = true;
    }
    if let Some(val) = record.get("body_fat_pct")
        && !val.is_null()
    {
        validate_range_f64(val, "body_fat_pct", 1.0, 75.0)?;
        has_metric = true;
    }
    if let Some(val) = record.get("heart_rate_bpm")
        && !val.is_null()
    {
        validate_range_u64(val, "heart_rate_bpm", 25, 250)?;
        has_metric = true;
    }

    let sys_opt = record.get("systolic_bp").filter(|v| !v.is_null());
    let dia_opt = record.get("diastolic_bp").filter(|v| !v.is_null());
    match (sys_opt, dia_opt) {
        (Some(sys_v), Some(dia_v)) => {
            let sys = validate_range_u64(sys_v, "systolic_bp", 50, 260)?;
            let dia = validate_range_u64(dia_v, "diastolic_bp", 30, 180)?;
            if sys <= dia {
                return Err(FunctionFailure::invalid_input(
                    "systolic_bp must be greater than diastolic_bp",
                ));
            }
            has_metric = true;
        }
        (Some(_), None) | (None, Some(_)) => {
            return Err(FunctionFailure::invalid_input(
                "systolic_bp and diastolic_bp must be provided together",
            ));
        }
        (None, None) => {}
    }

    if let Some(val) = record.get("energy_level")
        && !val.is_null()
    {
        validate_range_u64(val, "energy_level", 1, 5)?;
        has_metric = true;
    }

    if let Some(symptoms) = record.get("symptoms") {
        let list = validate_string_list(symptoms, "symptoms")?;
        if list.as_array().is_some_and(|a| !a.is_empty()) {
            has_metric = true;
        }
    }

    if let Some(notes) = record.get("notes") {
        let s = require_string(notes, "notes")?;
        if !s.trim().is_empty() {
            has_metric = true;
        }
    }

    if !has_metric {
        return Err(FunctionFailure::invalid_input(
            "body record requires at least one measurement, symptom, energy_level, or non-empty note",
        ));
    }

    Ok(())
}

pub fn handle_body_log(
    store: &dyn DocumentStore,
    request: &Request,
) -> Result<JsonValue, FunctionFailure> {
    run_health_mutation(store, request, "body.log", |doc, req| {
        let mut allowed = BODY_RECORD_FIELDS.to_vec();
        allowed.push("source");
        check_fields(&req.input, &allowed, &["date"])?;
        check_top_version(doc, req)?;

        let source = validate_source_or_default(&req.input)?;
        let timestamp = now_iso8601();

        let body_obj = doc
            .get_mut("body")
            .and_then(JsonValue::as_object_mut)
            .unwrap();
        let records = body_obj
            .get_mut("records")
            .and_then(JsonValue::as_array_mut)
            .unwrap();

        let record_id = generate_id("body", &req.request_id, (records.len() + 1) as u64);
        let mut record = BTreeMap::new();
        record.insert("id".to_string(), JsonValue::from(record_id.clone()));
        record.insert("version".to_string(), JsonValue::from(1_u64));
        record.insert(
            "date".to_string(),
            req.input.get("date").cloned().unwrap_or(JsonValue::Null),
        );
        record.insert(
            "weight_kg".to_string(),
            req.input
                .get("weight_kg")
                .cloned()
                .unwrap_or(JsonValue::Null),
        );
        record.insert(
            "body_fat_pct".to_string(),
            req.input
                .get("body_fat_pct")
                .cloned()
                .unwrap_or(JsonValue::Null),
        );
        record.insert(
            "heart_rate_bpm".to_string(),
            req.input
                .get("heart_rate_bpm")
                .cloned()
                .unwrap_or(JsonValue::Null),
        );
        record.insert(
            "systolic_bp".to_string(),
            req.input
                .get("systolic_bp")
                .cloned()
                .unwrap_or(JsonValue::Null),
        );
        record.insert(
            "diastolic_bp".to_string(),
            req.input
                .get("diastolic_bp")
                .cloned()
                .unwrap_or(JsonValue::Null),
        );
        record.insert(
            "energy_level".to_string(),
            req.input
                .get("energy_level")
                .cloned()
                .unwrap_or(JsonValue::Null),
        );
        let symptoms = match req.input.get("symptoms") {
            Some(v) => validate_string_list(v, "symptoms")?,
            None => JsonValue::Array(Vec::new()),
        };
        record.insert("symptoms".to_string(), symptoms);
        record.insert(
            "notes".to_string(),
            req.input
                .get("notes")
                .cloned()
                .unwrap_or_else(|| JsonValue::from("")),
        );
        record.insert("source".to_string(), source);
        record.insert("created_at".to_string(), JsonValue::from(timestamp.clone()));
        record.insert("updated_at".to_string(), JsonValue::from(timestamp));
        record.insert(
            "last_request_id".to_string(),
            JsonValue::from(req.request_id.clone()),
        );

        validate_body_record(&record)?;
        records.push(JsonValue::Object(record));

        Ok(JsonValue::object([
            ("status", JsonValue::from("succeeded")),
            ("request_id", JsonValue::from(req.request_id.clone())),
            (
                "changed_records",
                JsonValue::string_array(&[record_id.as_str()]),
            ),
            (
                "new_versions",
                JsonValue::object([(record_id.as_str(), JsonValue::from(1_u64))]),
            ),
            (
                "summary",
                JsonValue::from(format!("logged body status {record_id}")),
            ),
        ]))
    })
}

pub fn handle_body_update(
    store: &dyn DocumentStore,
    request: &Request,
) -> Result<JsonValue, FunctionFailure> {
    run_health_mutation(store, request, "body.update", |doc, req| {
        check_fields(&req.input, &["record_id", "patch"], &["record_id", "patch"])?;
        let record_id = require_non_empty_str(req.input.get("record_id").unwrap(), "record_id")?;
        let patch = req
            .input
            .get("patch")
            .and_then(JsonValue::as_object)
            .filter(|m| !m.is_empty())
            .ok_or_else(|| {
                FunctionFailure::invalid_input("patch must be a non-empty JSON object")
            })?;
        check_fields(patch, BODY_RECORD_FIELDS, &[])?;

        let body_obj = doc
            .get_mut("body")
            .and_then(JsonValue::as_object_mut)
            .unwrap();
        let records = body_obj
            .get_mut("records")
            .and_then(JsonValue::as_array_mut)
            .unwrap();

        let target = records
            .iter_mut()
            .find_map(|item| {
                let obj = item.as_object_mut()?;
                if obj.get("id").and_then(JsonValue::as_str) == Some(record_id) {
                    Some(obj)
                } else {
                    None
                }
            })
            .ok_or_else(|| {
                FunctionFailure::not_found(format!("body record {record_id} was not found"))
            })?;

        let current_ver = target
            .get("version")
            .and_then(JsonValue::as_u64)
            .unwrap_or(0);
        if current_ver != req.expected_version {
            return Err(FunctionFailure::version_conflict(format!(
                "record {record_id} version conflict: expected {}, current is {current_ver}",
                req.expected_version
            )));
        }

        for (k, v) in patch {
            if k == "symptoms" {
                let list = validate_string_list(v, "symptoms")?;
                target.insert(k.clone(), list);
            } else {
                target.insert(k.clone(), v.clone());
            }
        }

        validate_body_record(target)?;
        let next_ver = current_ver + 1;
        target.insert("version".to_string(), JsonValue::from(next_ver));
        target.insert("updated_at".to_string(), JsonValue::from(now_iso8601()));
        target.insert(
            "last_request_id".to_string(),
            JsonValue::from(req.request_id.clone()),
        );

        Ok(JsonValue::object([
            ("status", JsonValue::from("succeeded")),
            ("request_id", JsonValue::from(req.request_id.clone())),
            ("changed_records", JsonValue::string_array(&[record_id])),
            (
                "new_versions",
                JsonValue::object([(record_id, JsonValue::from(next_ver))]),
            ),
            (
                "summary",
                JsonValue::from(format!("updated body status {record_id}")),
            ),
        ]))
    })
}

// ---- 2. Daily Hydration handlers --------------------------------------------

pub fn handle_water_set_goal(
    store: &dyn DocumentStore,
    request: &Request,
) -> Result<JsonValue, FunctionFailure> {
    run_health_mutation(store, request, "water.set_goal", |doc, req| {
        check_fields(&req.input, &["daily_goal_ml"], &["daily_goal_ml"])?;
        check_top_version(doc, req)?;

        let goal = validate_range_u64(
            req.input.get("daily_goal_ml").unwrap(),
            "daily_goal_ml",
            100,
            15_000,
        )?;

        let water_obj = doc
            .get_mut("water")
            .and_then(JsonValue::as_object_mut)
            .unwrap();
        water_obj.insert("daily_goal_ml".to_string(), JsonValue::from(goal));
        water_obj.insert("updated_at".to_string(), JsonValue::from(now_iso8601()));

        Ok(JsonValue::object([
            ("status", JsonValue::from("succeeded")),
            ("request_id", JsonValue::from(req.request_id.clone())),
            ("changed_records", JsonValue::string_array(&["water.goal"])),
            (
                "new_versions",
                JsonValue::object([("health", JsonValue::from(0_u64))]),
            ),
            ("daily_goal_ml", JsonValue::from(goal)),
            (
                "summary",
                JsonValue::from(format!("set daily water goal to {goal} ml")),
            ),
        ]))
    })
}

fn validate_water_record(
    record: &BTreeMap<String, JsonValue>,
) -> Result<(String, u64), FunctionFailure> {
    let date_str = record
        .get("date")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| FunctionFailure::invalid_input("date is required"))?
        .to_string();
    validate_iso_date(&date_str, "date")?;

    let amount = validate_range_u64(
        record.get("amount_ml").unwrap_or(&JsonValue::Null),
        "amount_ml",
        1,
        5000,
    )?;

    let bev = record
        .get("beverage")
        .ok_or_else(|| FunctionFailure::invalid_input("beverage is required"))?;
    require_non_empty_str(bev, "beverage")?;

    if let Some(recorded_at) = record.get("recorded_at")
        && !recorded_at.is_null()
    {
        let ts = require_non_empty_str(recorded_at, "recorded_at")?;
        validate_instant(ts, "recorded_at")?;
    }

    if let Some(notes) = record.get("notes") {
        require_string(notes, "notes")?;
    }

    Ok((date_str, amount))
}

fn compute_daily_water_total(records: &[JsonValue], date: &str) -> u64 {
    records
        .iter()
        .filter_map(JsonValue::as_object)
        .filter(|obj| obj.get("date").and_then(JsonValue::as_str) == Some(date))
        .filter_map(|obj| obj.get("amount_ml").and_then(JsonValue::as_u64))
        .sum()
}

pub fn handle_water_log(
    store: &dyn DocumentStore,
    request: &Request,
) -> Result<JsonValue, FunctionFailure> {
    run_health_mutation(store, request, "water.log", |doc, req| {
        let mut allowed = WATER_RECORD_FIELDS.to_vec();
        allowed.push("source");
        check_fields(&req.input, &allowed, &["date", "amount_ml"])?;
        check_top_version(doc, req)?;

        let source = validate_source_or_default(&req.input)?;
        let timestamp = now_iso8601();

        let water_obj = doc
            .get_mut("water")
            .and_then(JsonValue::as_object_mut)
            .unwrap();
        let goal_ml = water_obj
            .get("daily_goal_ml")
            .and_then(JsonValue::as_u64)
            .unwrap_or(2000);
        let records = water_obj
            .get_mut("records")
            .and_then(JsonValue::as_array_mut)
            .unwrap();

        let record_id = generate_id("water", &req.request_id, (records.len() + 1) as u64);
        let mut record = BTreeMap::new();
        record.insert("id".to_string(), JsonValue::from(record_id.clone()));
        record.insert("version".to_string(), JsonValue::from(1_u64));
        record.insert(
            "date".to_string(),
            req.input.get("date").cloned().unwrap_or(JsonValue::Null),
        );
        record.insert(
            "amount_ml".to_string(),
            req.input
                .get("amount_ml")
                .cloned()
                .unwrap_or(JsonValue::Null),
        );
        record.insert(
            "beverage".to_string(),
            req.input
                .get("beverage")
                .cloned()
                .unwrap_or_else(|| JsonValue::from("water")),
        );
        record.insert(
            "recorded_at".to_string(),
            req.input
                .get("recorded_at")
                .cloned()
                .unwrap_or_else(|| JsonValue::from(timestamp.clone())),
        );
        record.insert(
            "notes".to_string(),
            req.input
                .get("notes")
                .cloned()
                .unwrap_or_else(|| JsonValue::from("")),
        );
        record.insert("source".to_string(), source);
        record.insert("created_at".to_string(), JsonValue::from(timestamp.clone()));
        record.insert("updated_at".to_string(), JsonValue::from(timestamp));
        record.insert(
            "last_request_id".to_string(),
            JsonValue::from(req.request_id.clone()),
        );

        let (date_str, _) = validate_water_record(&record)?;
        records.push(JsonValue::Object(record));

        let daily_total = compute_daily_water_total(records, &date_str);
        let goal_reached = daily_total >= goal_ml;

        Ok(JsonValue::object([
            ("status", JsonValue::from("succeeded")),
            ("request_id", JsonValue::from(req.request_id.clone())),
            (
                "changed_records",
                JsonValue::string_array(&[record_id.as_str()]),
            ),
            (
                "new_versions",
                JsonValue::object([(record_id.as_str(), JsonValue::from(1_u64))]),
            ),
            ("date", JsonValue::from(date_str)),
            ("daily_total_ml", JsonValue::from(daily_total)),
            ("daily_goal_ml", JsonValue::from(goal_ml)),
            ("goal_reached", JsonValue::from(goal_reached)),
            (
                "summary",
                JsonValue::from(format!("logged water intake {record_id}")),
            ),
        ]))
    })
}

pub fn handle_water_update(
    store: &dyn DocumentStore,
    request: &Request,
) -> Result<JsonValue, FunctionFailure> {
    run_health_mutation(store, request, "water.update", |doc, req| {
        check_fields(&req.input, &["record_id", "patch"], &["record_id", "patch"])?;
        let record_id = require_non_empty_str(req.input.get("record_id").unwrap(), "record_id")?;
        let patch = req
            .input
            .get("patch")
            .and_then(JsonValue::as_object)
            .filter(|m| !m.is_empty())
            .ok_or_else(|| {
                FunctionFailure::invalid_input("patch must be a non-empty JSON object")
            })?;
        check_fields(patch, WATER_RECORD_FIELDS, &[])?;

        let water_obj = doc
            .get_mut("water")
            .and_then(JsonValue::as_object_mut)
            .unwrap();
        let goal_ml = water_obj
            .get("daily_goal_ml")
            .and_then(JsonValue::as_u64)
            .unwrap_or(2000);
        let records = water_obj
            .get_mut("records")
            .and_then(JsonValue::as_array_mut)
            .unwrap();

        let (next_ver, date_str) = {
            let target = records
                .iter_mut()
                .find_map(|item| {
                    let obj = item.as_object_mut()?;
                    if obj.get("id").and_then(JsonValue::as_str) == Some(record_id) {
                        Some(obj)
                    } else {
                        None
                    }
                })
                .ok_or_else(|| {
                    FunctionFailure::not_found(format!("water record {record_id} was not found"))
                })?;

            let current_ver = target
                .get("version")
                .and_then(JsonValue::as_u64)
                .unwrap_or(0);
            if current_ver != req.expected_version {
                return Err(FunctionFailure::version_conflict(format!(
                    "record {record_id} version conflict: expected {}, current is {current_ver}",
                    req.expected_version
                )));
            }

            for (k, v) in patch {
                target.insert(k.clone(), v.clone());
            }

            let (date_str, _) = validate_water_record(target)?;
            let next_ver = current_ver + 1;
            target.insert("version".to_string(), JsonValue::from(next_ver));
            target.insert("updated_at".to_string(), JsonValue::from(now_iso8601()));
            target.insert(
                "last_request_id".to_string(),
                JsonValue::from(req.request_id.clone()),
            );
            (next_ver, date_str)
        };

        let daily_total = compute_daily_water_total(records, &date_str);

        Ok(JsonValue::object([
            ("status", JsonValue::from("succeeded")),
            ("request_id", JsonValue::from(req.request_id.clone())),
            ("changed_records", JsonValue::string_array(&[record_id])),
            (
                "new_versions",
                JsonValue::object([(record_id, JsonValue::from(next_ver))]),
            ),
            ("date", JsonValue::from(date_str)),
            ("daily_total_ml", JsonValue::from(daily_total)),
            ("daily_goal_ml", JsonValue::from(goal_ml)),
            ("goal_reached", JsonValue::from(daily_total >= goal_ml)),
            (
                "summary",
                JsonValue::from(format!("updated water intake {record_id}")),
            ),
        ]))
    })
}

// ---- 3. Weekly Diet Plan handlers -------------------------------------------

fn validate_meal_entry(meal: &JsonValue, context: &str) -> Result<JsonValue, FunctionFailure> {
    let obj = meal
        .as_object()
        .ok_or_else(|| FunctionFailure::invalid_input(format!("{context} must be an object")))?;
    check_fields(obj, &["items", "calories", "notes"], &["items"])?;

    let items = validate_string_list(obj.get("items").unwrap(), &format!("{context}.items"))?;
    let calories = match obj.get("calories") {
        Some(v) if !v.is_null() => {
            let cal = validate_range_u64(v, &format!("{context}.calories"), 0, 10_000)?;
            JsonValue::from(cal)
        }
        _ => JsonValue::Null,
    };
    let notes = match obj.get("notes") {
        Some(v) => JsonValue::from(require_string(v, &format!("{context}.notes"))?),
        None => JsonValue::from(""),
    };

    Ok(JsonValue::object([
        ("items", items),
        ("calories", calories),
        ("notes", notes),
    ]))
}

fn validate_days_map(days_val: &JsonValue) -> Result<JsonValue, FunctionFailure> {
    let days_obj = days_val
        .as_object()
        .ok_or_else(|| FunctionFailure::invalid_input("days must be an object"))?;

    let allowed_days: BTreeSet<&str> = DAYS_OF_WEEK.iter().copied().collect();
    let allowed_meals: BTreeSet<&str> = MEAL_TYPES.iter().copied().collect();

    let mut cleaned_days = BTreeMap::new();
    for (day_key, day_meals_val) in days_obj {
        if !allowed_days.contains(day_key.as_str()) {
            return Err(FunctionFailure::invalid_input(format!(
                "invalid day_of_week '{day_key}'; expected one of: {}",
                DAYS_OF_WEEK.join(", ")
            )));
        }
        let day_meals = day_meals_val.as_object().ok_or_else(|| {
            FunctionFailure::invalid_input(format!("days.{day_key} must be an object"))
        })?;
        let mut cleaned_meals = BTreeMap::new();
        for (meal_key, meal_val) in day_meals {
            if !allowed_meals.contains(meal_key.as_str()) {
                return Err(FunctionFailure::invalid_input(format!(
                    "invalid meal_type '{meal_key}'; expected one of: {}",
                    MEAL_TYPES.join(", ")
                )));
            }
            let validated_meal =
                validate_meal_entry(meal_val, &format!("days.{day_key}.{meal_key}"))?;
            cleaned_meals.insert(meal_key.clone(), validated_meal);
        }
        cleaned_days.insert(day_key.clone(), JsonValue::Object(cleaned_meals));
    }

    Ok(JsonValue::Object(cleaned_days))
}

pub fn handle_diet_set_week(
    store: &dyn DocumentStore,
    request: &Request,
) -> Result<JsonValue, FunctionFailure> {
    run_health_mutation(store, request, "diet.set_week", |doc, req| {
        check_fields(
            &req.input,
            &[
                "week_start",
                "daily_calorie_target",
                "preferences",
                "days",
                "notes",
            ],
            &["week_start"],
        )?;
        check_top_version(doc, req)?;

        let week_start = require_non_empty_str(req.input.get("week_start").unwrap(), "week_start")?;
        validate_iso_date(week_start, "week_start")?;

        let calorie_target = match req.input.get("daily_calorie_target") {
            Some(v) if !v.is_null() => {
                let cal = validate_range_u64(v, "daily_calorie_target", 500, 10_000)?;
                JsonValue::from(cal)
            }
            _ => JsonValue::Null,
        };

        let preferences = match req.input.get("preferences") {
            Some(v) => Some(validate_string_list(v, "preferences")?),
            None => None,
        };

        let days = match req.input.get("days") {
            Some(v) => Some(validate_days_map(v)?),
            None => None,
        };

        let notes = match req.input.get("notes") {
            Some(v) => Some(JsonValue::from(require_string(v, "notes")?)),
            None => None,
        };

        let diet_obj = doc
            .get_mut("diet")
            .and_then(JsonValue::as_object_mut)
            .unwrap();
        let plans = diet_obj
            .get_mut("plans")
            .and_then(JsonValue::as_array_mut)
            .unwrap();

        let timestamp = now_iso8601();
        let plan_id = format!("diet-{week_start}");

        if let Some(existing) = plans.iter_mut().find_map(|item| {
            let obj = item.as_object_mut()?;
            if obj.get("week_start").and_then(JsonValue::as_str) == Some(week_start) {
                Some(obj)
            } else {
                None
            }
        }) {
            let next_ver = existing
                .get("version")
                .and_then(JsonValue::as_u64)
                .unwrap_or(1)
                + 1;
            if req.input.contains_key("daily_calorie_target") {
                existing.insert("daily_calorie_target".to_string(), calorie_target);
            }
            if let Some(prefs) = preferences {
                existing.insert("preferences".to_string(), prefs);
            }
            if let Some(d) = days {
                existing.insert("days".to_string(), d);
            }
            if let Some(n) = notes {
                existing.insert("notes".to_string(), n);
            }
            existing.insert("version".to_string(), JsonValue::from(next_ver));
            existing.insert("updated_at".to_string(), JsonValue::from(timestamp));
            existing.insert(
                "last_request_id".to_string(),
                JsonValue::from(req.request_id.clone()),
            );

            Ok(JsonValue::object([
                ("status", JsonValue::from("succeeded")),
                ("request_id", JsonValue::from(req.request_id.clone())),
                (
                    "changed_records",
                    JsonValue::string_array(&[plan_id.as_str()]),
                ),
                (
                    "new_versions",
                    JsonValue::object([(plan_id.as_str(), JsonValue::from(next_ver))]),
                ),
                (
                    "summary",
                    JsonValue::from(format!("updated weekly diet plan {plan_id}")),
                ),
            ]))
        } else {
            let plan = JsonValue::object([
                ("id", JsonValue::from(plan_id.clone())),
                ("week_start", JsonValue::from(week_start)),
                ("version", JsonValue::from(1_u64)),
                ("daily_calorie_target", calorie_target),
                (
                    "preferences",
                    preferences.unwrap_or_else(|| JsonValue::Array(Vec::new())),
                ),
                (
                    "days",
                    days.unwrap_or_else(|| JsonValue::Object(BTreeMap::new())),
                ),
                ("notes", notes.unwrap_or_else(|| JsonValue::from(""))),
                ("created_at", JsonValue::from(timestamp.clone())),
                ("updated_at", JsonValue::from(timestamp)),
                ("last_request_id", JsonValue::from(req.request_id.clone())),
            ]);
            plans.push(plan);

            Ok(JsonValue::object([
                ("status", JsonValue::from("succeeded")),
                ("request_id", JsonValue::from(req.request_id.clone())),
                (
                    "changed_records",
                    JsonValue::string_array(&[plan_id.as_str()]),
                ),
                (
                    "new_versions",
                    JsonValue::object([(plan_id.as_str(), JsonValue::from(1_u64))]),
                ),
                (
                    "summary",
                    JsonValue::from(format!("created weekly diet plan {plan_id}")),
                ),
            ]))
        }
    })
}

pub fn handle_diet_set_meal(
    store: &dyn DocumentStore,
    request: &Request,
) -> Result<JsonValue, FunctionFailure> {
    run_health_mutation(store, request, "diet.set_meal", |doc, req| {
        check_fields(
            &req.input,
            &[
                "week_start",
                "day_of_week",
                "meal_type",
                "items",
                "calories",
                "notes",
            ],
            &["week_start", "day_of_week", "meal_type", "items"],
        )?;
        check_top_version(doc, req)?;

        let week_start = require_non_empty_str(req.input.get("week_start").unwrap(), "week_start")?;
        validate_iso_date(week_start, "week_start")?;

        let day_of_week =
            require_non_empty_str(req.input.get("day_of_week").unwrap(), "day_of_week")?;
        if !DAYS_OF_WEEK.contains(&day_of_week) {
            return Err(FunctionFailure::invalid_input(format!(
                "day_of_week must be one of: {}",
                DAYS_OF_WEEK.join(", ")
            )));
        }

        let meal_type = require_non_empty_str(req.input.get("meal_type").unwrap(), "meal_type")?;
        if !MEAL_TYPES.contains(&meal_type) {
            return Err(FunctionFailure::invalid_input(format!(
                "meal_type must be one of: {}",
                MEAL_TYPES.join(", ")
            )));
        }

        let meal_entry = validate_meal_entry(
            &JsonValue::object([
                (
                    "items",
                    req.input.get("items").cloned().unwrap_or(JsonValue::Null),
                ),
                (
                    "calories",
                    req.input
                        .get("calories")
                        .cloned()
                        .unwrap_or(JsonValue::Null),
                ),
                (
                    "notes",
                    req.input
                        .get("notes")
                        .cloned()
                        .unwrap_or_else(|| JsonValue::from("")),
                ),
            ]),
            "meal",
        )?;

        let diet_obj = doc
            .get_mut("diet")
            .and_then(JsonValue::as_object_mut)
            .unwrap();
        let plans = diet_obj
            .get_mut("plans")
            .and_then(JsonValue::as_array_mut)
            .unwrap();

        let timestamp = now_iso8601();
        let plan_id = format!("diet-{week_start}");

        if let Some(existing) = plans.iter_mut().find_map(|item| {
            let obj = item.as_object_mut()?;
            if obj.get("week_start").and_then(JsonValue::as_str) == Some(week_start) {
                Some(obj)
            } else {
                None
            }
        }) {
            let next_ver = existing
                .get("version")
                .and_then(JsonValue::as_u64)
                .unwrap_or(1)
                + 1;
            let days_obj = existing
                .entry("days".to_string())
                .or_insert_with(|| JsonValue::Object(BTreeMap::new()))
                .as_object_mut()
                .unwrap();
            let day_obj = days_obj
                .entry(day_of_week.to_string())
                .or_insert_with(|| JsonValue::Object(BTreeMap::new()))
                .as_object_mut()
                .unwrap();
            day_obj.insert(meal_type.to_string(), meal_entry);

            existing.insert("version".to_string(), JsonValue::from(next_ver));
            existing.insert("updated_at".to_string(), JsonValue::from(timestamp));
            existing.insert(
                "last_request_id".to_string(),
                JsonValue::from(req.request_id.clone()),
            );

            Ok(JsonValue::object([
                ("status", JsonValue::from("succeeded")),
                ("request_id", JsonValue::from(req.request_id.clone())),
                (
                    "changed_records",
                    JsonValue::string_array(&[plan_id.as_str()]),
                ),
                (
                    "new_versions",
                    JsonValue::object([(plan_id.as_str(), JsonValue::from(next_ver))]),
                ),
                (
                    "summary",
                    JsonValue::from(format!(
                        "set {day_of_week}.{meal_type} in weekly diet plan {plan_id}"
                    )),
                ),
            ]))
        } else {
            let days =
                JsonValue::object([(day_of_week, JsonValue::object([(meal_type, meal_entry)]))]);
            let plan = JsonValue::object([
                ("id", JsonValue::from(plan_id.clone())),
                ("week_start", JsonValue::from(week_start)),
                ("version", JsonValue::from(1_u64)),
                ("daily_calorie_target", JsonValue::Null),
                ("preferences", JsonValue::Array(Vec::new())),
                ("days", days),
                ("notes", JsonValue::from("")),
                ("created_at", JsonValue::from(timestamp.clone())),
                ("updated_at", JsonValue::from(timestamp)),
                ("last_request_id", JsonValue::from(req.request_id.clone())),
            ]);
            plans.push(plan);

            Ok(JsonValue::object([
                ("status", JsonValue::from("succeeded")),
                ("request_id", JsonValue::from(req.request_id.clone())),
                (
                    "changed_records",
                    JsonValue::string_array(&[plan_id.as_str()]),
                ),
                (
                    "new_versions",
                    JsonValue::object([(plan_id.as_str(), JsonValue::from(1_u64))]),
                ),
                (
                    "summary",
                    JsonValue::from(format!(
                        "created weekly diet plan {plan_id} with {day_of_week}.{meal_type}"
                    )),
                ),
            ]))
        }
    })
}

// ---- 4. Sleep Management handlers -------------------------------------------

pub fn handle_sleep_set_goal(
    store: &dyn DocumentStore,
    request: &Request,
) -> Result<JsonValue, FunctionFailure> {
    run_health_mutation(store, request, "sleep.set_goal", |doc, req| {
        check_fields(&req.input, SLEEP_GOAL_FIELDS, &[])?;
        if req.input.is_empty() {
            return Err(FunctionFailure::invalid_input(
                "input must specify at least one sleep goal field",
            ));
        }
        check_top_version(doc, req)?;

        let sleep_obj = doc
            .get_mut("sleep")
            .and_then(JsonValue::as_object_mut)
            .unwrap();
        let goal_obj = sleep_obj
            .get_mut("goal")
            .and_then(JsonValue::as_object_mut)
            .unwrap();

        if let Some(dur) = req.input.get("target_duration_minutes") {
            let mins = validate_range_u64(dur, "target_duration_minutes", 60, 960)?;
            goal_obj.insert("target_duration_minutes".to_string(), JsonValue::from(mins));
        }
        if let Some(bedtime) = req.input.get("target_bedtime") {
            if bedtime.is_null() {
                goal_obj.insert("target_bedtime".to_string(), JsonValue::Null);
            } else {
                let s = require_non_empty_str(bedtime, "target_bedtime")?;
                validate_hh_mm(s, "target_bedtime")?;
                goal_obj.insert("target_bedtime".to_string(), JsonValue::from(s));
            }
        }
        if let Some(wake) = req.input.get("target_wake_time") {
            if wake.is_null() {
                goal_obj.insert("target_wake_time".to_string(), JsonValue::Null);
            } else {
                let s = require_non_empty_str(wake, "target_wake_time")?;
                validate_hh_mm(s, "target_wake_time")?;
                goal_obj.insert("target_wake_time".to_string(), JsonValue::from(s));
            }
        }

        goal_obj.insert("updated_at".to_string(), JsonValue::from(now_iso8601()));

        Ok(JsonValue::object([
            ("status", JsonValue::from("succeeded")),
            ("request_id", JsonValue::from(req.request_id.clone())),
            ("changed_records", JsonValue::string_array(&["sleep.goal"])),
            (
                "new_versions",
                JsonValue::object([("health", JsonValue::from(0_u64))]),
            ),
            ("summary", JsonValue::from("updated sleep goal")),
        ]))
    })
}

fn validate_sleep_record(record: &BTreeMap<String, JsonValue>) -> Result<(), FunctionFailure> {
    let date_str = record
        .get("date")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| FunctionFailure::invalid_input("date is required"))?;
    validate_iso_date(date_str, "date")?;

    let slept_at = record
        .get("slept_at")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| FunctionFailure::invalid_input("slept_at is required"))?;
    validate_instant(slept_at, "slept_at")?;

    let woke_at = record
        .get("woke_at")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| FunctionFailure::invalid_input("woke_at is required"))?;
    validate_instant(woke_at, "woke_at")?;

    let duration = validate_range_u64(
        record.get("duration_minutes").unwrap_or(&JsonValue::Null),
        "duration_minutes",
        1,
        1440,
    )?;

    if let Some(score) = record.get("quality_score")
        && !score.is_null()
    {
        validate_range_u64(score, "quality_score", 1, 5)?;
    }

    if let Some(deep) = record.get("deep_sleep_minutes")
        && !deep.is_null()
    {
        let deep_mins = validate_range_u64(deep, "deep_sleep_minutes", 0, 1440)?;
        if deep_mins > duration {
            return Err(FunctionFailure::invalid_input(
                "deep_sleep_minutes cannot exceed duration_minutes",
            ));
        }
    }

    if let Some(awake) = record.get("awakenings")
        && !awake.is_null()
    {
        validate_range_u64(awake, "awakenings", 0, 100)?;
    }

    if let Some(notes) = record.get("notes") {
        require_string(notes, "notes")?;
    }

    Ok(())
}

pub fn handle_sleep_log(
    store: &dyn DocumentStore,
    request: &Request,
) -> Result<JsonValue, FunctionFailure> {
    run_health_mutation(store, request, "sleep.log", |doc, req| {
        let mut allowed = SLEEP_RECORD_FIELDS.to_vec();
        allowed.push("source");
        check_fields(
            &req.input,
            &allowed,
            &["date", "slept_at", "woke_at", "duration_minutes"],
        )?;
        check_top_version(doc, req)?;

        let source = validate_source_or_default(&req.input)?;
        let timestamp = now_iso8601();

        let sleep_obj = doc
            .get_mut("sleep")
            .and_then(JsonValue::as_object_mut)
            .unwrap();
        let records = sleep_obj
            .get_mut("records")
            .and_then(JsonValue::as_array_mut)
            .unwrap();

        let record_id = generate_id("sleep", &req.request_id, (records.len() + 1) as u64);
        let mut record = BTreeMap::new();
        record.insert("id".to_string(), JsonValue::from(record_id.clone()));
        record.insert("version".to_string(), JsonValue::from(1_u64));
        record.insert(
            "date".to_string(),
            req.input.get("date").cloned().unwrap_or(JsonValue::Null),
        );
        record.insert(
            "slept_at".to_string(),
            req.input
                .get("slept_at")
                .cloned()
                .unwrap_or(JsonValue::Null),
        );
        record.insert(
            "woke_at".to_string(),
            req.input.get("woke_at").cloned().unwrap_or(JsonValue::Null),
        );
        record.insert(
            "duration_minutes".to_string(),
            req.input
                .get("duration_minutes")
                .cloned()
                .unwrap_or(JsonValue::Null),
        );
        record.insert(
            "quality_score".to_string(),
            req.input
                .get("quality_score")
                .cloned()
                .unwrap_or(JsonValue::Null),
        );
        record.insert(
            "deep_sleep_minutes".to_string(),
            req.input
                .get("deep_sleep_minutes")
                .cloned()
                .unwrap_or(JsonValue::Null),
        );
        record.insert(
            "awakenings".to_string(),
            req.input
                .get("awakenings")
                .cloned()
                .unwrap_or_else(|| JsonValue::from(0_u64)),
        );
        record.insert(
            "notes".to_string(),
            req.input
                .get("notes")
                .cloned()
                .unwrap_or_else(|| JsonValue::from("")),
        );
        record.insert("source".to_string(), source);
        record.insert("created_at".to_string(), JsonValue::from(timestamp.clone()));
        record.insert("updated_at".to_string(), JsonValue::from(timestamp));
        record.insert(
            "last_request_id".to_string(),
            JsonValue::from(req.request_id.clone()),
        );

        validate_sleep_record(&record)?;
        records.push(JsonValue::Object(record));

        Ok(JsonValue::object([
            ("status", JsonValue::from("succeeded")),
            ("request_id", JsonValue::from(req.request_id.clone())),
            (
                "changed_records",
                JsonValue::string_array(&[record_id.as_str()]),
            ),
            (
                "new_versions",
                JsonValue::object([(record_id.as_str(), JsonValue::from(1_u64))]),
            ),
            (
                "summary",
                JsonValue::from(format!("logged sleep session {record_id}")),
            ),
        ]))
    })
}

pub fn handle_sleep_update(
    store: &dyn DocumentStore,
    request: &Request,
) -> Result<JsonValue, FunctionFailure> {
    run_health_mutation(store, request, "sleep.update", |doc, req| {
        check_fields(&req.input, &["record_id", "patch"], &["record_id", "patch"])?;
        let record_id = require_non_empty_str(req.input.get("record_id").unwrap(), "record_id")?;
        let patch = req
            .input
            .get("patch")
            .and_then(JsonValue::as_object)
            .filter(|m| !m.is_empty())
            .ok_or_else(|| {
                FunctionFailure::invalid_input("patch must be a non-empty JSON object")
            })?;
        check_fields(patch, SLEEP_RECORD_FIELDS, &[])?;

        let sleep_obj = doc
            .get_mut("sleep")
            .and_then(JsonValue::as_object_mut)
            .unwrap();
        let records = sleep_obj
            .get_mut("records")
            .and_then(JsonValue::as_array_mut)
            .unwrap();

        let target = records
            .iter_mut()
            .find_map(|item| {
                let obj = item.as_object_mut()?;
                if obj.get("id").and_then(JsonValue::as_str) == Some(record_id) {
                    Some(obj)
                } else {
                    None
                }
            })
            .ok_or_else(|| {
                FunctionFailure::not_found(format!("sleep record {record_id} was not found"))
            })?;

        let current_ver = target
            .get("version")
            .and_then(JsonValue::as_u64)
            .unwrap_or(0);
        if current_ver != req.expected_version {
            return Err(FunctionFailure::version_conflict(format!(
                "record {record_id} version conflict: expected {}, current is {current_ver}",
                req.expected_version
            )));
        }

        for (k, v) in patch {
            target.insert(k.clone(), v.clone());
        }

        validate_sleep_record(target)?;
        let next_ver = current_ver + 1;
        target.insert("version".to_string(), JsonValue::from(next_ver));
        target.insert("updated_at".to_string(), JsonValue::from(now_iso8601()));
        target.insert(
            "last_request_id".to_string(),
            JsonValue::from(req.request_id.clone()),
        );

        Ok(JsonValue::object([
            ("status", JsonValue::from("succeeded")),
            ("request_id", JsonValue::from(req.request_id.clone())),
            ("changed_records", JsonValue::string_array(&[record_id])),
            (
                "new_versions",
                JsonValue::object([(record_id, JsonValue::from(next_ver))]),
            ),
            (
                "summary",
                JsonValue::from(format!("updated sleep session {record_id}")),
            ),
        ]))
    })
}
