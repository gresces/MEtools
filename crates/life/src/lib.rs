//! `life` domain library for personal life and clothing management
//! (wearing state, laundry status, care mode, and storage location).
//!
//! Follows the unified command protocol:
//! - `GET /api/v1/programs/life`
//! - `GET /api/v1/programs/life/commands/<command>`
//! - `POST /api/v1/programs/life/commands/<command>`
//! - `me-client write life.<command> ...`

mod domain;

pub use domain::validate_document;

use domain::{
    handle_clothes_add, handle_clothes_archive, handle_clothes_mark_dirty, handle_clothes_update,
    handle_clothes_wash, handle_clothes_wear, handle_init,
};
use shared::{CommandSpec, FieldSpec, JsonValue, ProgramSpec};

// ---- example inputs ---------------------------------------------------------

fn example_clothes_add() -> JsonValue {
    JsonValue::object([
        ("name", JsonValue::from("Navy Oxford Shirt")),
        ("category", JsonValue::from("top")),
        ("seasons", JsonValue::string_array(&["spring", "autumn"])),
        ("color", JsonValue::from("navy")),
        ("location", JsonValue::from("Main Closet")),
        ("care_mode", JsonValue::from("machine")),
        ("max_wears_before_wash", JsonValue::from(2_u64)),
    ])
}

fn example_clothes_update() -> JsonValue {
    JsonValue::object([
        ("item_id", JsonValue::from("cloth-001")),
        (
            "patch",
            JsonValue::object([
                ("location", JsonValue::from("Bedroom Wardrobe")),
                ("max_wears_before_wash", JsonValue::from(3_u64)),
            ]),
        ),
    ])
}

fn example_clothes_wear() -> JsonValue {
    JsonValue::object([
        ("item_id", JsonValue::from("cloth-001")),
        ("date", JsonValue::from("2026-10-16")),
        ("put_back", JsonValue::from(true)),
    ])
}

fn example_clothes_mark_dirty() -> JsonValue {
    JsonValue::object([
        ("item_id", JsonValue::from("cloth-001")),
        ("notes", JsonValue::from("Coffee stain on cuff")),
    ])
}

fn example_clothes_wash() -> JsonValue {
    JsonValue::object([
        ("item_ids", JsonValue::string_array(&["cloth-001"])),
        ("action", JsonValue::from("complete")),
        ("date", JsonValue::from("2026-10-16")),
    ])
}

fn example_clothes_archive() -> JsonValue {
    JsonValue::object([
        ("item_id", JsonValue::from("cloth-001")),
        ("archived", JsonValue::from(true)),
    ])
}

// ---- field specifications ---------------------------------------------------

const CLOTHES_ADD_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "name",
        field_type: "string",
        required: true,
        description: "Non-empty clothing item name",
    },
    FieldSpec {
        name: "category",
        field_type: "string (top | bottom | outerwear | underwear | shoes | accessory | other)",
        required: true,
        description: "Clothing category",
    },
    FieldSpec {
        name: "seasons",
        field_type: "array<string> (spring | summer | autumn | winter | all)",
        required: false,
        description: "Applicable seasons; defaults to [\"all\"]",
    },
    FieldSpec {
        name: "color",
        field_type: "string",
        required: false,
        description: "Primary color description",
    },
    FieldSpec {
        name: "brand",
        field_type: "string",
        required: false,
        description: "Brand name",
    },
    FieldSpec {
        name: "location",
        field_type: "string",
        required: false,
        description: "Storage location (e.g. closet, drawer, shoe rack)",
    },
    FieldSpec {
        name: "care_mode",
        field_type: "string (machine | hand | dry_clean | air_only)",
        required: false,
        description: "Washing or care instruction; defaults to \"machine\"",
    },
    FieldSpec {
        name: "max_wears_before_wash",
        field_type: "integer (1..100)",
        required: false,
        description: "Recommended maximum wears before washing; defaults by category",
    },
    FieldSpec {
        name: "notes",
        field_type: "string",
        required: false,
        description: "Additional notes on fit, fabric, or care",
    },
    FieldSpec {
        name: "source",
        field_type: "object { type, reference? }",
        required: false,
        description: "Fact source metadata; defaults to {\"type\": \"user_instruction\"}",
    },
];

const CLOTHES_UPDATE_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "item_id",
        field_type: "string",
        required: true,
        description: "Target clothing item ID",
    },
    FieldSpec {
        name: "patch",
        field_type: "object",
        required: true,
        description: "Non-empty patch object allowing name, category, seasons, color, brand, location, care_mode, status (clean/wearing/worn/dirty/washing/drying), max_wears_before_wash, notes",
    },
];

const CLOTHES_WEAR_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "item_id",
        field_type: "string",
        required: true,
        description: "Target clothing item ID",
    },
    FieldSpec {
        name: "date",
        field_type: "string (YYYY-MM-DD)",
        required: false,
        description: "Wearing date; defaults to current date",
    },
    FieldSpec {
        name: "put_back",
        field_type: "boolean",
        required: false,
        description: "When false (default), sets status to 'wearing'; when true, finishes wearing and sets status to 'dirty' (if wear_count_since_wash >= max_wears_before_wash) or 'worn'",
    },
    FieldSpec {
        name: "notes",
        field_type: "string",
        required: false,
        description: "Optional wearing or outfit notes",
    },
];

const CLOTHES_MARK_DIRTY_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "item_id",
        field_type: "string",
        required: true,
        description: "Target clothing item ID",
    },
    FieldSpec {
        name: "notes",
        field_type: "string",
        required: false,
        description: "Optional stain or laundry reason notes",
    },
];

const CLOTHES_WASH_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "item_ids",
        field_type: "array<string>",
        required: true,
        description: "Non-empty list of clothing item IDs to transition",
    },
    FieldSpec {
        name: "action",
        field_type: "string (start | dry | complete)",
        required: true,
        description: "Laundry stage action: 'start' -> washing, 'dry' -> drying, 'complete' -> clean (resets wear_count_since_wash to 0)",
    },
    FieldSpec {
        name: "date",
        field_type: "string (YYYY-MM-DD)",
        required: false,
        description: "Wash date recorded when action is 'complete'; defaults to current date",
    },
    FieldSpec {
        name: "notes",
        field_type: "string",
        required: false,
        description: "Optional laundry notes",
    },
];

const CLOTHES_ARCHIVE_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "item_id",
        field_type: "string",
        required: true,
        description: "Target clothing item ID",
    },
    FieldSpec {
        name: "archived",
        field_type: "boolean",
        required: true,
        description: "true to archive, false to restore",
    },
];

// ---- command table ----------------------------------------------------------

const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        name: "clothes.add",
        summary: "Add a new clothing item to the wardrobe",
        reads: &["life"],
        writes: &["life"],
        expected_version_rule: "Use top-level life collection version",
        fields: CLOTHES_ADD_FIELDS,
        rules: &[
            "New clothing items start with status='clean', wear_count_since_wash=0, and version=1",
        ],
        example_version: 1,
        example_input: example_clothes_add,
        handler: handle_clothes_add,
    },
    CommandSpec {
        name: "clothes.update",
        summary: "Update attributes, location, care mode, or status of a clothing item",
        reads: &["life"],
        writes: &["life"],
        expected_version_rule: "Use target clothing item record version",
        fields: CLOTHES_UPDATE_FIELDS,
        rules: &["Archived clothing items must be restored before updating"],
        example_version: 1,
        example_input: example_clothes_update,
        handler: handle_clothes_update,
    },
    CommandSpec {
        name: "clothes.wear",
        summary: "Record wearing a clothing item and track wear count before washing",
        reads: &["life"],
        writes: &["life"],
        expected_version_rule: "Use target clothing item record version",
        fields: CLOTHES_WEAR_FIELDS,
        rules: &[
            "Items in 'dirty', 'washing', or 'drying' status cannot be worn until washing is completed",
            "Increments wear_count_since_wash and total_wears; returns needs_wash=true when wear_count_since_wash reaches max_wears_before_wash",
        ],
        example_version: 1,
        example_input: example_clothes_wear,
        handler: handle_clothes_wear,
    },
    CommandSpec {
        name: "clothes.mark_dirty",
        summary: "Mark a clothing item as dirty and needing laundry",
        reads: &["life"],
        writes: &["life"],
        expected_version_rule: "Use target clothing item record version",
        fields: CLOTHES_MARK_DIRTY_FIELDS,
        rules: &["Transitions item status to 'dirty'"],
        example_version: 1,
        example_input: example_clothes_mark_dirty,
        handler: handle_clothes_mark_dirty,
    },
    CommandSpec {
        name: "clothes.wash",
        summary: "Start washing, dry, or complete laundry for one or more clothing items",
        reads: &["life"],
        writes: &["life"],
        expected_version_rule: "Use top-level life collection version",
        fields: CLOTHES_WASH_FIELDS,
        rules: &[
            "action='start' sets status to 'washing'; action='dry' sets status to 'drying'",
            "action='complete' sets status to 'clean', resets wear_count_since_wash to 0, and increments total_washes",
        ],
        example_version: 1,
        example_input: example_clothes_wash,
        handler: handle_clothes_wash,
    },
    CommandSpec {
        name: "clothes.archive",
        summary: "Archive or restore a clothing item",
        reads: &["life"],
        writes: &["life"],
        expected_version_rule: "Use target clothing item record version",
        fields: CLOTHES_ARCHIVE_FIELDS,
        rules: &[
            "Items currently in 'wearing', 'washing', or 'drying' status cannot be archived until resolved",
        ],
        example_version: 1,
        example_input: example_clothes_archive,
        handler: handle_clothes_archive,
    },
];

pub const PROGRAM: ProgramSpec = ProgramSpec {
    name: "life",
    summary: "Personal life and clothing management (wearing and laundry status) commands",
    collections: &["life"],
    usage: &[
        "GET /api/v1/programs/life",
        "GET /api/v1/programs/life/commands/<command>",
        "POST /api/v1/programs/life/commands/<command>",
        "me-client write life.<command> ...",
    ],
    rules: &[
        "--help and describe do not access the database",
        "Responses contain exactly one JSON value; diagnostics never join the response payload",
        "Writes use the envelope {request_id, expected_version, input} on POST /api/v1/programs/life/commands/<command>; me-client write accepts --input, --input-file, or repeatable --set key=value",
        "All life data is stored in the `life` collection and written in one SQLite transaction with 128-entry idempotency receipts",
    ],
    error_codes: &[
        (
            "INVALID_INPUT",
            "Invalid request JSON, field values, date format, or command arguments",
        ),
        (
            "NOT_FOUND",
            "The `life` collection or the target clothing item ID was not found",
        ),
        (
            "VERSION_CONFLICT",
            "expected_version does not match current document or record version (exit code 4)",
        ),
        (
            "INVALID_TRANSITION",
            "Invalid clothing status transition or archive precondition",
        ),
        (
            "WRITE_BUSY",
            "Another writer holds the database lock; retry the request",
        ),
        (
            "REQUEST_ID_CONFLICT",
            "request_id was already used with a different request payload",
        ),
        (
            "DATA_INVALID",
            "The stored `life` document is corrupt or could not be decoded",
        ),
    ],
    init_handler: handle_init,
    commands: COMMANDS,
};

// ---- library entry points ---------------------------------------------------

/// Return the static program contract served by `me-server`.
pub fn program() -> &'static ProgramSpec {
    &PROGRAM
}

// ---- tests ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use shared::{EXIT_FAILURE, EXIT_SUCCESS, EXIT_VERSION_CONFLICT, parse_json};
    use std::io::Cursor;

    #[test]
    fn life_protocol_commands_return_valid_ascii_json_without_reading_stdin() {
        let store = me_store::MemoryStore::new();
        let mut command_args = vec![
            vec!["--help".to_string()],
            vec!["describe".to_string()],
            vec!["describe".to_string(), "init".to_string()],
        ];
        for cmd in COMMANDS {
            command_args.push(vec!["describe".to_string(), cmd.name.to_string()]);
        }

        for args in command_args {
            let mut out = Vec::new();
            let code = me_test_support::dispatch(
                &store,
                &PROGRAM,
                &args,
                Cursor::new("invalid stdin content"),
                &mut out,
            );
            assert_eq!(code, EXIT_SUCCESS);
            let output_str = String::from_utf8(out).expect("stdout must be UTF-8");
            assert!(
                output_str.is_ascii(),
                "protocol response must be ASCII-compatible: {output_str}"
            );
            let response = parse_json(&output_str).expect("stdout must be valid JSON");
            assert_eq!(
                response.get("status").and_then(JsonValue::as_str),
                Some("succeeded")
            );
            assert_eq!(
                response.get("program").and_then(JsonValue::as_str),
                Some("life")
            );
        }
    }

    #[test]
    fn clothes_add_wear_dirty_wash_and_archive_lifecycle() {
        let store = me_store::MemoryStore::new();

        // 1. init
        let mut out = Vec::new();
        let code = me_test_support::dispatch(
            &store,
            &PROGRAM,
            &["init".to_string()],
            Cursor::new(""),
            &mut out,
        );
        assert_eq!(code, EXIT_SUCCESS);

        // 2. clothes.add with max_wears_before_wash=2
        let mut out = Vec::new();
        let code = me_test_support::dispatch(
            &store,
            &PROGRAM,
            &[
                "clothes.add".to_string(),
                "--request-id".to_string(),
                "req-add-shirt".to_string(),
                "--expected-version".to_string(),
                "1".to_string(),
                "--set".to_string(),
                "name=White Tee".to_string(),
                "--set".to_string(),
                "category=top".to_string(),
                "--set".to_string(),
                "max_wears_before_wash=2".to_string(),
            ],
            Cursor::new(""),
            &mut out,
        );
        assert_eq!(code, EXIT_SUCCESS);
        let add_res = parse_json(&String::from_utf8(out).unwrap()).unwrap();
        let item_id = add_res
            .get("changed_records")
            .and_then(JsonValue::as_array)
            .and_then(|a| a.first())
            .and_then(JsonValue::as_str)
            .unwrap()
            .to_string();

        // 3. First wear with put_back=true -> status becomes 'worn', needs_wash=false
        let mut out = Vec::new();
        let code = me_test_support::dispatch(
            &store,
            &PROGRAM,
            &[
                "clothes.wear".to_string(),
                "--request-id".to_string(),
                "req-wear-1".to_string(),
                "--expected-version".to_string(),
                "1".to_string(),
                "--set".to_string(),
                format!("item_id={item_id}"),
                "--set".to_string(),
                "date=2026-10-15".to_string(),
                "--set".to_string(),
                "put_back=true".to_string(),
            ],
            Cursor::new(""),
            &mut out,
        );
        assert_eq!(code, EXIT_SUCCESS);
        let wear1_res = parse_json(&String::from_utf8(out).unwrap()).unwrap();
        assert_eq!(
            wear1_res.get("item_status").and_then(JsonValue::as_str),
            Some("worn")
        );
        assert_eq!(
            wear1_res.get("needs_wash").and_then(JsonValue::as_bool),
            Some(false)
        );

        // 4. Second wear with put_back=true -> reaches max_wears_before_wash=2 -> status='dirty', needs_wash=true
        let mut out = Vec::new();
        let code = me_test_support::dispatch(
            &store,
            &PROGRAM,
            &[
                "clothes.wear".to_string(),
                "--request-id".to_string(),
                "req-wear-2".to_string(),
                "--expected-version".to_string(),
                "2".to_string(),
                "--set".to_string(),
                format!("item_id={item_id}"),
                "--set".to_string(),
                "date=2026-10-16".to_string(),
                "--set".to_string(),
                "put_back=true".to_string(),
            ],
            Cursor::new(""),
            &mut out,
        );
        assert_eq!(code, EXIT_SUCCESS);
        let wear2_res = parse_json(&String::from_utf8(out).unwrap()).unwrap();
        assert_eq!(
            wear2_res.get("item_status").and_then(JsonValue::as_str),
            Some("dirty")
        );
        assert_eq!(
            wear2_res.get("needs_wash").and_then(JsonValue::as_bool),
            Some(true)
        );

        // 5. Wearing a dirty item is rejected with INVALID_TRANSITION
        let mut out = Vec::new();
        let code = me_test_support::dispatch(
            &store,
            &PROGRAM,
            &[
                "clothes.wear".to_string(),
                "--request-id".to_string(),
                "req-wear-dirty".to_string(),
                "--expected-version".to_string(),
                "3".to_string(),
                "--set".to_string(),
                format!("item_id={item_id}"),
            ],
            Cursor::new(""),
            &mut out,
        );
        assert_eq!(code, EXIT_FAILURE);
        let err_res = parse_json(&String::from_utf8(out).unwrap()).unwrap();
        assert_eq!(
            err_res
                .get("error")
                .and_then(|e| e.get("code"))
                .and_then(JsonValue::as_str),
            Some("INVALID_TRANSITION")
        );

        // 6. Wash start -> washing (collection version is 4)
        let mut out = Vec::new();
        let code = me_test_support::dispatch(
            &store,
            &PROGRAM,
            &[
                "clothes.wash".to_string(),
                "--request-id".to_string(),
                "req-wash-start".to_string(),
                "--expected-version".to_string(),
                "4".to_string(),
                "--set".to_string(),
                format!("item_ids=[\"{item_id}\"]"),
                "--set".to_string(),
                "action=start".to_string(),
            ],
            Cursor::new(""),
            &mut out,
        );
        assert_eq!(code, EXIT_SUCCESS);

        // 7. Wash complete -> clean, resets wear_count_since_wash to 0 (collection version is 5)
        let mut out = Vec::new();
        let code = me_test_support::dispatch(
            &store,
            &PROGRAM,
            &[
                "clothes.wash".to_string(),
                "--request-id".to_string(),
                "req-wash-complete".to_string(),
                "--expected-version".to_string(),
                "5".to_string(),
                "--set".to_string(),
                format!("item_ids=[\"{item_id}\"]"),
                "--set".to_string(),
                "action=complete".to_string(),
                "--set".to_string(),
                "date=2026-10-16".to_string(),
            ],
            Cursor::new(""),
            &mut out,
        );
        assert_eq!(code, EXIT_SUCCESS);

        // 8. Archive and restore (item version is now 5)
        let mut out = Vec::new();
        let code = me_test_support::dispatch(
            &store,
            &PROGRAM,
            &[
                "clothes.archive".to_string(),
                "--request-id".to_string(),
                "req-archive".to_string(),
                "--expected-version".to_string(),
                "5".to_string(),
                "--set".to_string(),
                format!("item_id={item_id}"),
                "--set".to_string(),
                "archived=true".to_string(),
            ],
            Cursor::new(""),
            &mut out,
        );
        assert_eq!(code, EXIT_SUCCESS);

        // 9. Stale version triggers VERSION_CONFLICT
        let mut out = Vec::new();
        let code = me_test_support::dispatch(
            &store,
            &PROGRAM,
            &[
                "clothes.archive".to_string(),
                "--request-id".to_string(),
                "req-archive-stale".to_string(),
                "--expected-version".to_string(),
                "5".to_string(),
                "--set".to_string(),
                format!("item_id={item_id}"),
                "--set".to_string(),
                "archived=false".to_string(),
            ],
            Cursor::new(""),
            &mut out,
        );
        assert_eq!(code, EXIT_VERSION_CONFLICT);
    }
}
