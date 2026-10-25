//! `life` domain state validation, persistence, and command handlers for clothing management.

use std::collections::{BTreeMap, BTreeSet};

use shared::{
    Document, DocumentStore, FunctionFailure, JsonValue, MutateOutcome, Request, check_fields,
    generate_id, now_iso8601, validate_instant, validate_iso_date,
};

pub const SCHEMA_VERSION: u64 = 1;
const CLOTHES_CATEGORIES: &[&str] = &[
    "top",
    "bottom",
    "outerwear",
    "underwear",
    "shoes",
    "accessory",
    "other",
];

const SEASONS: &[&str] = &["spring", "summer", "autumn", "winter", "all"];

const CARE_MODES: &[&str] = &["machine", "hand", "dry_clean", "air_only"];

const CLOTHES_STATUSES: &[&str] = &["clean", "wearing", "worn", "dirty", "washing", "drying"];

const WASH_ACTIONS: &[&str] = &["start", "dry", "complete"];

const CLOTHES_ADD_FIELDS: &[&str] = &[
    "name",
    "category",
    "seasons",
    "color",
    "brand",
    "location",
    "care_mode",
    "max_wears_before_wash",
    "notes",
    "source",
];

const CLOTHES_PATCH_FIELDS: &[&str] = &[
    "name",
    "category",
    "seasons",
    "color",
    "brand",
    "location",
    "care_mode",
    "status",
    "max_wears_before_wash",
    "notes",
];

// ---- default_document -------------------------------------------------------

/// Construct a fresh `life.json` document with default clothing collection.
pub fn default_document(timestamp: &str) -> BTreeMap<String, JsonValue> {
    let clothes = JsonValue::object([("items", JsonValue::Array(Vec::new()))]);

    let JsonValue::Object(map) = JsonValue::object([
        ("schema_version", JsonValue::from(SCHEMA_VERSION)),
        ("version", JsonValue::from(1_u64)),
        ("updated_at", JsonValue::from(timestamp)),
        ("updated_by", JsonValue::from("init")),
        ("clothes", clothes),
        ("recent_requests", JsonValue::Array(Vec::new())),
    ]) else {
        unreachable!()
    };
    map
}

// ---- validate_document ------------------------------------------------------

/// Validate the top-level envelope and `clothes` section of `life.json`.
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

    let clothes = doc
        .get("clothes")
        .and_then(JsonValue::as_object)
        .ok_or_else(|| {
            FunctionFailure::new("DATA_INVALID", "missing or invalid clothes section")
        })?;
    let items = clothes
        .get("items")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| FunctionFailure::new("DATA_INVALID", "clothes.items must be an array"))?;

    let mut seen_ids = BTreeSet::new();
    for item in items {
        let obj = item.as_object().ok_or_else(|| {
            FunctionFailure::new("DATA_INVALID", "clothing item must be an object")
        })?;
        let id = obj
            .get("id")
            .and_then(JsonValue::as_str)
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| {
                FunctionFailure::new("DATA_INVALID", "clothing item missing valid id")
            })?;
        if !seen_ids.insert(id.to_string()) {
            return Err(FunctionFailure::new(
                "DATA_INVALID",
                format!("duplicate clothing item ID: {id}"),
            ));
        }
    }

    Ok(version)
}

// ---- handle_init ------------------------------------------------------------

/// Initialize the `life` collection if missing, or verify it if already present.
pub fn handle_init(store: &dyn DocumentStore) -> Result<JsonValue, FunctionFailure> {
    if let Some(document) = store.load_optional("life")? {
        let version = validate_document(&document)?;
        return Ok(JsonValue::object([
            ("status", JsonValue::from("succeeded")),
            ("request_id", JsonValue::from("init")),
            ("changed_records", JsonValue::Array(Vec::new())),
            (
                "new_versions",
                JsonValue::object([("life", JsonValue::from(version))]),
            ),
            ("collection_version", JsonValue::from(version)),
            ("summary", JsonValue::from("life.json already initialized")),
        ]));
    }

    let timestamp = now_iso8601();
    let document = default_document(&timestamp);
    store.insert_initial("life", &document)?;

    Ok(JsonValue::object([
        ("status", JsonValue::from("succeeded")),
        ("request_id", JsonValue::from("init")),
        ("changed_records", JsonValue::string_array(&["life"])),
        (
            "new_versions",
            JsonValue::object([("life", JsonValue::from(1_u64))]),
        ),
        ("collection_version", JsonValue::from(1_u64)),
        ("summary", JsonValue::from("initialized life.json")),
    ]))
}

// ---- run_life_mutation ----------------------------------------------------

/// Execute a transactional, replay-aware mutation on the `life` collection.
fn run_life_mutation<F>(
    store: &dyn DocumentStore,
    request: &Request,
    operation: &str,
    mutate: F,
) -> Result<JsonValue, FunctionFailure>
where
    F: Fn(&mut Document, &Request) -> Result<JsonValue, FunctionFailure>,
{
    let outcome = store.with_document_mut("life", request, operation, &mut |document| {
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
                && versions.contains_key("life")
            {
                versions.insert("life".to_string(), JsonValue::from(current_doc_version + 1));
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
            "life version conflict: expected {}, current is {current}",
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

fn require_choice<'a>(
    value: &'a JsonValue,
    choices: &[&str],
    field: &str,
) -> Result<&'a str, FunctionFailure> {
    let s = require_non_empty_str(value, field)?;
    if !choices.contains(&s) {
        return Err(FunctionFailure::invalid_input(format!(
            "invalid {field} '{s}'; expected one of: {}",
            choices.join(", ")
        )));
    }
    Ok(s)
}

fn validate_seasons(value: &JsonValue) -> Result<JsonValue, FunctionFailure> {
    let arr = value.as_array().filter(|a| !a.is_empty()).ok_or_else(|| {
        FunctionFailure::invalid_input("seasons must be a non-empty array of strings")
    })?;
    let mut cleaned = Vec::with_capacity(arr.len());
    for item in arr {
        let s = require_choice(item, SEASONS, "seasons")?;
        cleaned.push(JsonValue::from(s));
    }
    Ok(JsonValue::Array(cleaned))
}

fn validate_max_wears(value: &JsonValue) -> Result<u64, FunctionFailure> {
    let n = value.as_u64().ok_or_else(|| {
        FunctionFailure::invalid_input("max_wears_before_wash must be a positive integer")
    })?;
    if !(1..=100).contains(&n) {
        return Err(FunctionFailure::invalid_input(
            "max_wears_before_wash must be between 1 and 100",
        ));
    }
    Ok(n)
}

fn default_max_wears_for_category(category: &str) -> u64 {
    match category {
        "underwear" | "top" => 1,
        "bottom" => 3,
        "outerwear" => 10,
        "shoes" | "accessory" => 15,
        _ => 2,
    }
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

fn validate_clothing_record(record: &BTreeMap<String, JsonValue>) -> Result<(), FunctionFailure> {
    require_non_empty_str(record.get("name").unwrap_or(&JsonValue::Null), "name")?;
    require_choice(
        record.get("category").unwrap_or(&JsonValue::Null),
        CLOTHES_CATEGORIES,
        "category",
    )?;
    validate_seasons(record.get("seasons").unwrap_or(&JsonValue::Null))?;
    require_string(record.get("color").unwrap_or(&JsonValue::Null), "color")?;
    require_string(record.get("brand").unwrap_or(&JsonValue::Null), "brand")?;
    require_string(
        record.get("location").unwrap_or(&JsonValue::Null),
        "location",
    )?;
    require_choice(
        record.get("care_mode").unwrap_or(&JsonValue::Null),
        CARE_MODES,
        "care_mode",
    )?;
    require_choice(
        record.get("status").unwrap_or(&JsonValue::Null),
        CLOTHES_STATUSES,
        "status",
    )?;
    validate_max_wears(
        record
            .get("max_wears_before_wash")
            .unwrap_or(&JsonValue::Null),
    )?;
    require_string(record.get("notes").unwrap_or(&JsonValue::Null), "notes")?;

    if let Some(d) = record.get("last_worn_date")
        && !d.is_null()
    {
        let s = require_non_empty_str(d, "last_worn_date")?;
        validate_iso_date(s, "last_worn_date")?;
    }
    if let Some(d) = record.get("last_washed_date")
        && !d.is_null()
    {
        let s = require_non_empty_str(d, "last_washed_date")?;
        validate_iso_date(s, "last_washed_date")?;
    }
    if let Some(archived) = record.get("archived_at")
        && !archived.is_null()
    {
        let s = require_non_empty_str(archived, "archived_at")?;
        validate_instant(s, "archived_at")?;
    }

    Ok(())
}

fn find_clothing_item<'a>(
    doc: &'a mut BTreeMap<String, JsonValue>,
    item_id: &str,
) -> Result<&'a mut BTreeMap<String, JsonValue>, FunctionFailure> {
    let clothes = doc
        .get_mut("clothes")
        .and_then(JsonValue::as_object_mut)
        .unwrap();
    let items = clothes
        .get_mut("items")
        .and_then(JsonValue::as_array_mut)
        .unwrap();

    items
        .iter_mut()
        .find_map(|entry| {
            let obj = entry.as_object_mut()?;
            if obj.get("id").and_then(JsonValue::as_str) == Some(item_id) {
                Some(obj)
            } else {
                None
            }
        })
        .ok_or_else(|| FunctionFailure::not_found(format!("clothing item {item_id} was not found")))
}

fn check_record_version(
    record: &BTreeMap<String, JsonValue>,
    item_id: &str,
    expected_version: u64,
) -> Result<u64, FunctionFailure> {
    let current = record
        .get("version")
        .and_then(JsonValue::as_u64)
        .unwrap_or(0);
    if current != expected_version {
        return Err(FunctionFailure::version_conflict(format!(
            "clothing item {item_id} version conflict: expected {expected_version}, current is {current}"
        )));
    }
    Ok(current)
}

fn ensure_unarchived(
    record: &BTreeMap<String, JsonValue>,
    item_id: &str,
) -> Result<(), FunctionFailure> {
    if record.get("archived_at").is_some_and(|v| !v.is_null()) {
        return Err(FunctionFailure::new(
            "INVALID_TRANSITION",
            format!("restore archived clothing item {item_id} before modifying"),
        ));
    }
    Ok(())
}

// ---- command handlers -------------------------------------------------------

/// Add a new clothing item to `clothes.items`.
pub fn handle_clothes_add(
    store: &dyn DocumentStore,
    request: &Request,
) -> Result<JsonValue, FunctionFailure> {
    run_life_mutation(store, request, "clothes.add", |doc, req| {
        check_fields(&req.input, CLOTHES_ADD_FIELDS, &["name", "category"])?;
        check_top_version(doc, req)?;

        let name = require_non_empty_str(req.input.get("name").unwrap(), "name")?;
        let category = require_choice(
            req.input.get("category").unwrap(),
            CLOTHES_CATEGORIES,
            "category",
        )?;
        let seasons = match req.input.get("seasons") {
            Some(v) => validate_seasons(v)?,
            None => JsonValue::string_array(&["all"]),
        };
        let color = match req.input.get("color") {
            Some(v) => require_string(v, "color")?.to_string(),
            None => String::new(),
        };
        let brand = match req.input.get("brand") {
            Some(v) => require_string(v, "brand")?.to_string(),
            None => String::new(),
        };
        let location = match req.input.get("location") {
            Some(v) => require_string(v, "location")?.to_string(),
            None => String::new(),
        };
        let care_mode = match req.input.get("care_mode") {
            Some(v) => require_choice(v, CARE_MODES, "care_mode")?.to_string(),
            None => "machine".to_string(),
        };
        let max_wears = match req.input.get("max_wears_before_wash") {
            Some(v) => validate_max_wears(v)?,
            None => default_max_wears_for_category(category),
        };
        let notes = match req.input.get("notes") {
            Some(v) => require_string(v, "notes")?.to_string(),
            None => String::new(),
        };
        let source = validate_source_or_default(&req.input)?;

        let clothes = doc
            .get_mut("clothes")
            .and_then(JsonValue::as_object_mut)
            .unwrap();
        let items = clothes
            .get_mut("items")
            .and_then(JsonValue::as_array_mut)
            .unwrap();

        let item_id = generate_id("cloth", &req.request_id, (items.len() + 1) as u64);
        let timestamp = now_iso8601();

        let mut record = BTreeMap::new();
        record.insert("id".to_string(), JsonValue::from(item_id.clone()));
        record.insert("version".to_string(), JsonValue::from(1_u64));
        record.insert("name".to_string(), JsonValue::from(name));
        record.insert("category".to_string(), JsonValue::from(category));
        record.insert("seasons".to_string(), seasons);
        record.insert("color".to_string(), JsonValue::from(color));
        record.insert("brand".to_string(), JsonValue::from(brand));
        record.insert("location".to_string(), JsonValue::from(location));
        record.insert("care_mode".to_string(), JsonValue::from(care_mode));
        record.insert("status".to_string(), JsonValue::from("clean"));
        record.insert("wear_count_since_wash".to_string(), JsonValue::from(0_u64));
        record.insert(
            "max_wears_before_wash".to_string(),
            JsonValue::from(max_wears),
        );
        record.insert("total_wears".to_string(), JsonValue::from(0_u64));
        record.insert("total_washes".to_string(), JsonValue::from(0_u64));
        record.insert("last_worn_date".to_string(), JsonValue::Null);
        record.insert("last_washed_date".to_string(), JsonValue::Null);
        record.insert("notes".to_string(), JsonValue::from(notes));
        record.insert("archived_at".to_string(), JsonValue::Null);
        record.insert("source".to_string(), source);
        record.insert("created_at".to_string(), JsonValue::from(timestamp.clone()));
        record.insert("updated_at".to_string(), JsonValue::from(timestamp));
        record.insert(
            "last_request_id".to_string(),
            JsonValue::from(req.request_id.clone()),
        );

        validate_clothing_record(&record)?;
        items.push(JsonValue::Object(record));

        Ok(JsonValue::object([
            ("status", JsonValue::from("succeeded")),
            ("request_id", JsonValue::from(req.request_id.clone())),
            (
                "changed_records",
                JsonValue::string_array(&[item_id.as_str()]),
            ),
            (
                "new_versions",
                JsonValue::object([(item_id.as_str(), JsonValue::from(1_u64))]),
            ),
            (
                "summary",
                JsonValue::from(format!("added clothing item {item_id}")),
            ),
        ]))
    })
}

/// Update attributes or status of an existing clothing item.
pub fn handle_clothes_update(
    store: &dyn DocumentStore,
    request: &Request,
) -> Result<JsonValue, FunctionFailure> {
    run_life_mutation(store, request, "clothes.update", |doc, req| {
        check_fields(&req.input, &["item_id", "patch"], &["item_id", "patch"])?;
        let item_id = require_non_empty_str(req.input.get("item_id").unwrap(), "item_id")?;
        let patch = req
            .input
            .get("patch")
            .and_then(JsonValue::as_object)
            .filter(|m| !m.is_empty())
            .ok_or_else(|| {
                FunctionFailure::invalid_input("patch must be a non-empty JSON object")
            })?;
        check_fields(patch, CLOTHES_PATCH_FIELDS, &[])?;

        let target = find_clothing_item(doc, item_id)?;
        let current_ver = check_record_version(target, item_id, req.expected_version)?;
        ensure_unarchived(target, item_id)?;

        for (k, v) in patch {
            if k == "seasons" {
                let s = validate_seasons(v)?;
                target.insert(k.clone(), s);
            } else {
                target.insert(k.clone(), v.clone());
            }
        }

        validate_clothing_record(target)?;
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
            ("changed_records", JsonValue::string_array(&[item_id])),
            (
                "new_versions",
                JsonValue::object([(item_id, JsonValue::from(next_ver))]),
            ),
            (
                "summary",
                JsonValue::from(format!("updated clothing item {item_id}")),
            ),
        ]))
    })
}

/// Record wearing a clothing item on a given date, incrementing wear counters and checking wash threshold.
pub fn handle_clothes_wear(
    store: &dyn DocumentStore,
    request: &Request,
) -> Result<JsonValue, FunctionFailure> {
    run_life_mutation(store, request, "clothes.wear", |doc, req| {
        check_fields(
            &req.input,
            &["item_id", "date", "put_back", "notes"],
            &["item_id"],
        )?;
        let item_id = require_non_empty_str(req.input.get("item_id").unwrap(), "item_id")?;

        let timestamp = now_iso8601();
        let date_str = match req.input.get("date") {
            Some(v) => {
                let s = require_non_empty_str(v, "date")?;
                validate_iso_date(s, "date")?;
                s.to_string()
            }
            None => timestamp[0..10].to_string(),
        };

        let put_back = match req.input.get("put_back") {
            Some(v) => v
                .as_bool()
                .ok_or_else(|| FunctionFailure::invalid_input("put_back must be a boolean"))?,
            None => false,
        };

        let notes_opt = match req.input.get("notes") {
            Some(v) => Some(require_string(v, "notes")?.to_string()),
            None => None,
        };

        let target = find_clothing_item(doc, item_id)?;
        let current_ver = check_record_version(target, item_id, req.expected_version)?;
        ensure_unarchived(target, item_id)?;

        let current_status = target
            .get("status")
            .and_then(JsonValue::as_str)
            .unwrap_or("clean");
        if matches!(current_status, "dirty" | "washing" | "drying") {
            return Err(FunctionFailure::new(
                "INVALID_TRANSITION",
                format!(
                    "cannot wear clothing item {item_id} while its status is '{current_status}'; complete washing first"
                ),
            ));
        }

        let wear_count = target
            .get("wear_count_since_wash")
            .and_then(JsonValue::as_u64)
            .unwrap_or(0)
            + 1;
        let total_wears = target
            .get("total_wears")
            .and_then(JsonValue::as_u64)
            .unwrap_or(0)
            + 1;
        let max_wears = target
            .get("max_wears_before_wash")
            .and_then(JsonValue::as_u64)
            .unwrap_or(1);

        let needs_wash = wear_count >= max_wears;
        let new_status = if put_back {
            if needs_wash { "dirty" } else { "worn" }
        } else {
            "wearing"
        };

        let next_ver = current_ver + 1;
        target.insert("status".to_string(), JsonValue::from(new_status));
        target.insert(
            "wear_count_since_wash".to_string(),
            JsonValue::from(wear_count),
        );
        target.insert("total_wears".to_string(), JsonValue::from(total_wears));
        target.insert("last_worn_date".to_string(), JsonValue::from(date_str));
        if let Some(n) = notes_opt {
            target.insert("notes".to_string(), JsonValue::from(n));
        }
        target.insert("version".to_string(), JsonValue::from(next_ver));
        target.insert("updated_at".to_string(), JsonValue::from(timestamp));
        target.insert(
            "last_request_id".to_string(),
            JsonValue::from(req.request_id.clone()),
        );

        Ok(JsonValue::object([
            ("status", JsonValue::from("succeeded")),
            ("request_id", JsonValue::from(req.request_id.clone())),
            ("changed_records", JsonValue::string_array(&[item_id])),
            (
                "new_versions",
                JsonValue::object([(item_id, JsonValue::from(next_ver))]),
            ),
            ("item_status", JsonValue::from(new_status)),
            ("wear_count_since_wash", JsonValue::from(wear_count)),
            ("max_wears_before_wash", JsonValue::from(max_wears)),
            ("needs_wash", JsonValue::from(needs_wash)),
            (
                "summary",
                JsonValue::from(format!(
                    "recorded wear for {item_id} (status={new_status}, wear_count={wear_count}/{max_wears})"
                )),
            ),
        ]))
    })
}

/// Mark a clothing item as needing laundry (`status = "dirty"`).
pub fn handle_clothes_mark_dirty(
    store: &dyn DocumentStore,
    request: &Request,
) -> Result<JsonValue, FunctionFailure> {
    run_life_mutation(store, request, "clothes.mark_dirty", |doc, req| {
        check_fields(&req.input, &["item_id", "notes"], &["item_id"])?;
        let item_id = require_non_empty_str(req.input.get("item_id").unwrap(), "item_id")?;
        let notes_opt = match req.input.get("notes") {
            Some(v) => Some(require_string(v, "notes")?.to_string()),
            None => None,
        };

        let target = find_clothing_item(doc, item_id)?;
        let current_ver = check_record_version(target, item_id, req.expected_version)?;
        ensure_unarchived(target, item_id)?;

        let current_status = target
            .get("status")
            .and_then(JsonValue::as_str)
            .unwrap_or("clean");
        if matches!(current_status, "washing" | "drying") {
            return Err(FunctionFailure::new(
                "INVALID_TRANSITION",
                format!("clothing item {item_id} is currently '{current_status}'"),
            ));
        }

        let next_ver = current_ver + 1;
        target.insert("status".to_string(), JsonValue::from("dirty"));
        if let Some(n) = notes_opt {
            target.insert("notes".to_string(), JsonValue::from(n));
        }
        target.insert("version".to_string(), JsonValue::from(next_ver));
        target.insert("updated_at".to_string(), JsonValue::from(now_iso8601()));
        target.insert(
            "last_request_id".to_string(),
            JsonValue::from(req.request_id.clone()),
        );

        Ok(JsonValue::object([
            ("status", JsonValue::from("succeeded")),
            ("request_id", JsonValue::from(req.request_id.clone())),
            ("changed_records", JsonValue::string_array(&[item_id])),
            (
                "new_versions",
                JsonValue::object([(item_id, JsonValue::from(next_ver))]),
            ),
            ("item_status", JsonValue::from("dirty")),
            (
                "summary",
                JsonValue::from(format!("marked clothing item {item_id} as dirty")),
            ),
        ]))
    })
}

/// Transition one or more clothing items through laundry stages (`start` -> `washing`, `dry` -> `drying`, `complete` -> `clean`).
pub fn handle_clothes_wash(
    store: &dyn DocumentStore,
    request: &Request,
) -> Result<JsonValue, FunctionFailure> {
    run_life_mutation(store, request, "clothes.wash", |doc, req| {
        check_fields(
            &req.input,
            &["item_ids", "action", "date", "notes"],
            &["item_ids", "action"],
        )?;
        check_top_version(doc, req)?;

        let action = require_choice(req.input.get("action").unwrap(), WASH_ACTIONS, "action")?;

        let raw_ids = req
            .input
            .get("item_ids")
            .and_then(JsonValue::as_array)
            .filter(|a| !a.is_empty())
            .ok_or_else(|| {
                FunctionFailure::invalid_input("item_ids must be a non-empty array of strings")
            })?;
        let mut item_ids = Vec::with_capacity(raw_ids.len());
        let mut seen = BTreeSet::new();
        for val in raw_ids {
            let id = require_non_empty_str(val, "item_ids")?;
            if seen.insert(id.to_string()) {
                item_ids.push(id.to_string());
            }
        }

        let timestamp = now_iso8601();
        let date_str = match req.input.get("date") {
            Some(v) => {
                let s = require_non_empty_str(v, "date")?;
                validate_iso_date(s, "date")?;
                s.to_string()
            }
            None => timestamp[0..10].to_string(),
        };

        let notes_opt = match req.input.get("notes") {
            Some(v) => Some(require_string(v, "notes")?.to_string()),
            None => None,
        };

        let target_status = match action {
            "start" => "washing",
            "dry" => "drying",
            "complete" => "clean",
            _ => unreachable!(),
        };

        // First pass: verify all target items exist and are unarchived
        for id in &item_ids {
            let item = find_clothing_item(doc, id)?;
            ensure_unarchived(item, id)?;
        }

        // Second pass: apply laundry transition
        let mut new_versions = BTreeMap::new();
        let mut changed_ids = Vec::with_capacity(item_ids.len());
        for id in &item_ids {
            let item = find_clothing_item(doc, id)?;
            let next_ver = item.get("version").and_then(JsonValue::as_u64).unwrap_or(1) + 1;
            item.insert("status".to_string(), JsonValue::from(target_status));
            if action == "complete" {
                let washes = item
                    .get("total_washes")
                    .and_then(JsonValue::as_u64)
                    .unwrap_or(0)
                    + 1;
                item.insert("wear_count_since_wash".to_string(), JsonValue::from(0_u64));
                item.insert("total_washes".to_string(), JsonValue::from(washes));
                item.insert(
                    "last_washed_date".to_string(),
                    JsonValue::from(date_str.clone()),
                );
            }
            if let Some(ref n) = notes_opt {
                item.insert("notes".to_string(), JsonValue::from(n.clone()));
            }
            item.insert("version".to_string(), JsonValue::from(next_ver));
            item.insert("updated_at".to_string(), JsonValue::from(timestamp.clone()));
            item.insert(
                "last_request_id".to_string(),
                JsonValue::from(req.request_id.clone()),
            );

            new_versions.insert(id.clone(), JsonValue::from(next_ver));
            changed_ids.push(JsonValue::from(id.clone()));
        }

        Ok(JsonValue::object([
            ("status", JsonValue::from("succeeded")),
            ("request_id", JsonValue::from(req.request_id.clone())),
            ("changed_records", JsonValue::Array(changed_ids)),
            ("new_versions", JsonValue::Object(new_versions)),
            ("item_status", JsonValue::from(target_status)),
            (
                "summary",
                JsonValue::from(format!(
                    "applied wash action '{action}' -> '{target_status}' for {} item(s)",
                    item_ids.len()
                )),
            ),
        ]))
    })
}

/// Archive or restore a clothing item.
pub fn handle_clothes_archive(
    store: &dyn DocumentStore,
    request: &Request,
) -> Result<JsonValue, FunctionFailure> {
    run_life_mutation(store, request, "clothes.archive", |doc, req| {
        check_fields(
            &req.input,
            &["item_id", "archived"],
            &["item_id", "archived"],
        )?;
        let item_id = require_non_empty_str(req.input.get("item_id").unwrap(), "item_id")?;
        let archived = req
            .input
            .get("archived")
            .and_then(JsonValue::as_bool)
            .ok_or_else(|| FunctionFailure::invalid_input("archived must be a boolean"))?;

        let target = find_clothing_item(doc, item_id)?;
        let current_ver = check_record_version(target, item_id, req.expected_version)?;

        if archived {
            let status = target
                .get("status")
                .and_then(JsonValue::as_str)
                .unwrap_or("clean");
            if matches!(status, "wearing" | "washing" | "drying") {
                return Err(FunctionFailure::new(
                    "INVALID_TRANSITION",
                    format!(
                        "cannot archive clothing item {item_id} while its status is '{status}'"
                    ),
                ));
            }
        }

        let timestamp = now_iso8601();
        let next_ver = current_ver + 1;
        target.insert(
            "archived_at".to_string(),
            if archived {
                JsonValue::from(timestamp.clone())
            } else {
                JsonValue::Null
            },
        );
        target.insert("version".to_string(), JsonValue::from(next_ver));
        target.insert("updated_at".to_string(), JsonValue::from(timestamp));
        target.insert(
            "last_request_id".to_string(),
            JsonValue::from(req.request_id.clone()),
        );

        Ok(JsonValue::object([
            ("status", JsonValue::from("succeeded")),
            ("request_id", JsonValue::from(req.request_id.clone())),
            ("changed_records", JsonValue::string_array(&[item_id])),
            (
                "new_versions",
                JsonValue::object([(item_id, JsonValue::from(next_ver))]),
            ),
            (
                "summary",
                JsonValue::from(if archived {
                    format!("archived clothing item {item_id}")
                } else {
                    format!("restored clothing item {item_id}")
                }),
            ),
        ]))
    })
}
