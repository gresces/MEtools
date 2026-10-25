//! `personal` domain library for personal profile and action management.
//!
//! The handlers run inside `me-server` and are reached through the HTTP command API.
//!
//! Follows the unified command protocol:
//! - `GET /api/v1/programs/personal`
//! - `GET /api/v1/programs/personal/commands/<command>`
//! - `POST /api/v1/programs/personal/commands/<command>`
//! - `me-client write personal.<command> ...`

mod domain;

pub use domain::{
    SCHEMA_VERSION, migrate_document, validate_action, validate_action_document,
    validate_action_queue, validate_envelope,
};

use domain::{
    handle_action_archive, handle_action_complete, handle_action_create, handle_action_delete,
    handle_action_queue, handle_action_update, handle_init, handle_migrate, handle_profile_update,
};
use shared::{CommandSpec, FieldSpec, JsonValue, ProgramSpec};

// ---- example inputs ---------------------------------------------------------

fn example_action_create() -> JsonValue {
    JsonValue::object([
        ("title", JsonValue::from("Submit proposal")),
        ("checkpoint_id", JsonValue::from("checkpoint-wrapper")),
        ("due_date", JsonValue::from("2099-10-16")),
        ("priority", JsonValue::from("high")),
        ("duration_minutes", JsonValue::from(30_u64)),
    ])
}

fn example_action_complete() -> JsonValue {
    JsonValue::object([("action_id", JsonValue::from("action-001"))])
}

fn example_action_update() -> JsonValue {
    JsonValue::object([
        ("action_id", JsonValue::from("action-001")),
        (
            "patch",
            JsonValue::object([("status", JsonValue::from("done"))]),
        ),
    ])
}

fn example_action_archive() -> JsonValue {
    JsonValue::object([
        ("action_id", JsonValue::from("action-001")),
        ("archived", JsonValue::from(true)),
    ])
}

fn example_action_queue() -> JsonValue {
    JsonValue::object([
        ("action_id", JsonValue::from("action-001")),
        ("position", JsonValue::from(0_u64)),
    ])
}

fn example_action_delete() -> JsonValue {
    JsonValue::object([("action_id", JsonValue::from("action-001"))])
}

fn example_profile_update() -> JsonValue {
    JsonValue::object([
        (
            "patch",
            JsonValue::object([
                ("display_name", JsonValue::from("ME")),
                ("timezone", JsonValue::from("Asia/Shanghai")),
                ("occupation", JsonValue::from("Engineer")),
                (
                    "skills",
                    JsonValue::array([JsonValue::from("Rust"), JsonValue::from("SQLite")]),
                ),
                ("age", JsonValue::from(30_i64)),
                ("remote", JsonValue::from(true)),
            ]),
        ),
        ("remove", JsonValue::array([JsonValue::from("legacy_note")])),
    ])
}

fn example_migrate() -> JsonValue {
    JsonValue::object([
        ("collection", JsonValue::from("actions")),
        ("target_schema_version", JsonValue::from(7_u64)),
    ])
}

// ---- command specifications -------------------------------------------------

const ACTION_CREATE_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "title",
        field_type: "string",
        required: true,
        description: "Non-empty action title",
    },
    FieldSpec {
        name: "notes",
        field_type: "string",
        required: false,
        description: "Action notes; may be empty",
    },
    FieldSpec {
        name: "priority",
        field_type: "string (high | normal | low)",
        required: false,
        description: "Action priority; defaults to normal",
    },
    FieldSpec {
        name: "due_at",
        field_type: "string (ISO-8601 with offset) | null",
        required: false,
        description: "Deadline timestamp with timezone offset; mutually exclusive with due_date",
    },
    FieldSpec {
        name: "due_date",
        field_type: "string (YYYY-MM-DD) | null",
        required: false,
        description: "Deadline calendar date; mutually exclusive with due_at",
    },
    FieldSpec {
        name: "tags",
        field_type: "array<string>",
        required: false,
        description: "List of non-empty tag strings",
    },
    FieldSpec {
        name: "source",
        field_type: "object { type, reference? }",
        required: false,
        description: "Fact source metadata; defaults to {\"type\": \"user_instruction\"}",
    },
    FieldSpec {
        name: "checkpoint_id",
        field_type: "string",
        required: true,
        description: "Existing checkpoint that owns this action; its goal and milestone are linked automatically",
    },
    FieldSpec {
        name: "duration_minutes",
        field_type: "integer (1..=600)",
        required: false,
        description: "Planned action duration in minutes; defaults to 30",
    },
    FieldSpec {
        name: "buffer_minutes",
        field_type: "integer (0..=600)",
        required: false,
        description: "Elastic buffer in minutes; defaults to duration_minutes",
    },
    FieldSpec {
        name: "recurrence",
        field_type: "object | null",
        required: false,
        description: "Optional repeat rule: {frequency: daily|weekly|monthly|yearly, interval?: 1..99, count?: 1..1000, until?: YYYY-MM-DD, by_weekday?: [mon..sun] (weekly only)}; requires due_at or due_date",
    },
];

const ACTION_UPDATE_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "action_id",
        field_type: "string",
        required: true,
        description: "Target Action ID",
    },
    FieldSpec {
        name: "patch",
        field_type: "object",
        required: true,
        description: "Non-empty patch object allowing title, notes, status, priority, due_at, due_date, tags, duration_minutes, buffer_minutes, recurrence; changing recurrence resets the occurrence count and anchors at the current due",
    },
];

const ACTION_COMPLETE_FIELDS: &[FieldSpec] = &[FieldSpec {
    name: "action_id",
    field_type: "string",
    required: true,
    description: "Open recurring action whose current occurrence should be completed",
}];

const ACTION_ARCHIVE_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "action_id",
        field_type: "string",
        required: true,
        description: "Target Action ID",
    },
    FieldSpec {
        name: "archived",
        field_type: "boolean",
        required: true,
        description: "true to archive, false to restore",
    },
];

const ACTION_DELETE_FIELDS: &[FieldSpec] = &[FieldSpec {
    name: "action_id",
    field_type: "string",
    required: true,
    description: "Action to delete; its goal links are removed",
}];

const ACTION_QUEUE_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "action_id",
        field_type: "string",
        required: true,
        description: "ID of an open, unarchived action",
    },
    FieldSpec {
        name: "position",
        field_type: "integer (>= 0)",
        required: true,
        description: "Zero-based queue position; 0 is the current action",
    },
];

const PROFILE_UPDATE_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "patch",
        field_type: "object<string, JSON>",
        required: false,
        description: "Profile key/value pairs; values may be any JSON value. Keys must be 1-64 characters without surrounding whitespace or control characters",
    },
    FieldSpec {
        name: "remove",
        field_type: "array<string>",
        required: false,
        description: "Profile keys to delete; display_name and timezone cannot be removed. At least one of patch or remove must be non-empty",
    },
];

const MIGRATE_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "collection",
        field_type: "string (actions)",
        required: true,
        description: "Target collection name",
    },
    FieldSpec {
        name: "target_schema_version",
        field_type: "integer (7)",
        required: true,
        description: "Target schema version number; v7 backfills action recurrence fields",
    },
];

const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        name: "action.create",
        summary: "Create an action under an existing checkpoint",
        reads: &["actions", "goals"],
        writes: &["actions", "goals"],
        expected_version_rule: "Use top-level actions collection version; the goals version is bumped by the link write",
        fields: ACTION_CREATE_FIELDS,
        rules: &[
            "checkpoint_id must reference an existing checkpoint in the goals collection",
            "The owning goal and milestone are derived from the checkpoint and an action link is written in the same transaction",
            "duration_minutes defaults to 30; buffer_minutes defaults to duration_minutes and is the elastic buffer",
            "recurrence requires due_at or due_date; the first due is the recurrence anchor and recurrence_completed_count starts at 0",
            "New actions start with status=open and record version=1",
            "New actions are appended to the execution queue",
            "At most one of due_at or due_date may be non-null",
        ],
        example_version: 2,
        example_input: example_action_create,
        handler: handle_action_create,
    },
    CommandSpec {
        name: "action.update",
        summary: "Update, complete, reopen, or cancel an action",
        reads: &["actions"],
        writes: &["actions"],
        expected_version_rule: "Use target action record version",
        fields: ACTION_UPDATE_FIELDS,
        rules: &[
            "Archived records must be restored before updating",
            "Changing duration_minutes without buffer_minutes also updates buffer_minutes while it still equals the previous duration",
            "Changing recurrence resets recurrence_completed_count and anchors at the current due; rescheduling a started recurring action without changing recurrence is rejected",
            "Reopen an action to status=open before switching between terminal statuses (done and cancelled)",
            "Terminal status removes the action from the queue; reopening appends it to the end",
            "Explicitly clear the other deadline field in the same patch when switching between due_at and due_date",
        ],
        example_version: 1,
        example_input: example_action_update,
        handler: handle_action_update,
    },
    CommandSpec {
        name: "action.complete",
        summary: "Complete one occurrence of a recurring action",
        reads: &["actions"],
        writes: &["actions"],
        expected_version_rule: "Use target action record version",
        fields: ACTION_COMPLETE_FIELDS,
        rules: &[
            "Only open, unarchived actions with recurrence can complete an occurrence",
            "Advances due_at/due_date to the next occurrence and increments recurrence_completed_count",
            "When count or until is exhausted, marks the action done and removes it from the queue",
            "Use action.update for one-off actions",
        ],
        example_version: 1,
        example_input: example_action_complete,
        handler: handle_action_complete,
    },
    CommandSpec {
        name: "action.archive",
        summary: "Archive or restore an action",
        reads: &["actions"],
        writes: &["actions"],
        expected_version_rule: "Use target action record version",
        fields: ACTION_ARCHIVE_FIELDS,
        rules: &[
            "Only done or cancelled actions can be archived",
            "Archiving removes the action from the queue; restoring an open action re-queues it",
        ],
        example_version: 1,
        example_input: example_action_archive,
        handler: handle_action_archive,
    },
    CommandSpec {
        name: "action.delete",
        summary: "Delete an action and remove its goal references",
        reads: &["actions", "goals"],
        writes: &["actions", "goals"],
        expected_version_rule: "Use target action record version",
        fields: ACTION_DELETE_FIELDS,
        rules: &[
            "Deletes the action and removes it from the execution queue",
            "Removes matching action_links from the goals collection",
        ],
        example_version: 1,
        example_input: example_action_delete,
        handler: handle_action_delete,
    },
    CommandSpec {
        name: "action.queue",
        summary: "Move an open action to a position in the execution queue",
        reads: &["actions"],
        writes: &["actions"],
        expected_version_rule: "Use top-level actions collection version",
        fields: ACTION_QUEUE_FIELDS,
        rules: &[
            "Queue index 0 is the current action; index 1 is the next action",
            "Only open, unarchived actions can be queued",
            "Positions beyond the queue length append the action at the end",
        ],
        example_version: 1,
        example_input: example_action_queue,
        handler: handle_action_queue,
    },
    CommandSpec {
        name: "profile.update",
        summary: "Update arbitrary personal profile key/value pairs",
        reads: &["profile"],
        writes: &["profile"],
        expected_version_rule: "Use top-level profile version",
        fields: PROFILE_UPDATE_FIELDS,
        rules: &[
            "patch and remove are applied in one versioned write; at least one must be non-empty",
            "display_name and timezone remain non-empty strings and cannot be removed",
            "Reserved keys version, updated_at, updated_by, and recent_requests are rejected",
            "Keys are 1-64 characters with no surrounding whitespace or control characters",
            "Each serialized value is limited to 16 KiB; at most 256 profile keys are allowed",
            "Removing a missing key is a no-op; increments top-level version and retains bounded request receipts",
        ],
        example_version: 1,
        example_input: example_profile_update,
        handler: handle_profile_update,
    },
    CommandSpec {
        name: "migrate",
        summary: "Upgrade the actions schema",
        reads: &["actions"],
        writes: &["actions"],
        expected_version_rule: "Use the top-level actions collection version",
        fields: MIGRATE_FIELDS,
        rules: &[
            "Preserves existing record IDs, facts, and versions while filling default fields for schema_version=7",
            "Legacy actions receive duration_minutes=30, a matching buffer_minutes, and recurrence fields",
            "Migrating legacy tasks preserves the existing queue; a missing queue is derived from due date, priority, and creation time",
        ],
        example_version: 1,
        example_input: example_migrate,
        handler: handle_migrate,
    },
];

pub const PROGRAM: ProgramSpec = ProgramSpec {
    name: "personal",
    summary: "Personal profile, actions, and data migration commands",
    collections: &["profile", "actions"],
    usage: &[
        "GET /api/v1/programs/personal",
        "GET /api/v1/programs/personal/commands/<command>",
        "POST /api/v1/programs/personal/commands/<command>",
        "me-client write personal.<command> ...",
    ],
    rules: &[
        "--help and describe do not access the database",
        "Responses contain exactly one JSON value; diagnostics never join the response payload",
        "Writes use the envelope {request_id, expected_version, input} on POST /api/v1/programs/personal/commands/<command>; me-client write accepts --input, --input-file, or repeatable --set key=value",
        "Each write runs in one SQLite transaction on its collection and retains the last 128 successful request receipts",
        "The `actions` document holds a `queue` array of open action IDs: index 0 is the current action, index 1 the next one",
        "Actions are atomic steps with duration_minutes and an elastic buffer_minutes; the buffer defaults to the duration and follows duration changes until it is set explicitly",
        "Recurring actions keep one record, advance due_at/due_date after each action.complete, and expose recurrence_completed_count for the current occurrence",
        "Actions must be created under an existing checkpoint; action.create writes the action link in the same transaction",
    ],
    error_codes: &[
        (
            "INVALID_INPUT",
            "Invalid request JSON, field types, date/time format, or command arguments",
        ),
        ("NOT_FOUND", "Target record or collection was not found"),
        (
            "VERSION_CONFLICT",
            "expected_version does not match current data version (exit code 4)",
        ),
        (
            "INVALID_TRANSITION",
            "Invalid lifecycle status transition, archive precondition, or linked action state",
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
            "MIGRATION_REQUIRED",
            "Collection must be migrated to the required schema_version first",
        ),
        (
            "DATA_INVALID",
            "Stored document data is corrupt or could not be decoded",
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
    use shared::{DocumentStore, EXIT_FAILURE, EXIT_SUCCESS, parse_json};
    use std::io::Cursor;

    const STAMP: &str = "2026-10-12T20:00:00+08:00";
    const CHECKPOINT_ID: &str = "checkpoint-wrapper";

    fn seed_goal_checkpoint(store: &me_store::MemoryStore) {
        let goals = JsonValue::object([
            ("version", JsonValue::from(1_u64)),
            (
                "goals",
                JsonValue::array([JsonValue::object([(
                    "id",
                    JsonValue::from("goal-portfolio"),
                )])]),
            ),
            (
                "milestones",
                JsonValue::array([JsonValue::object([
                    ("id", JsonValue::from("milestone-design")),
                    ("goal_id", JsonValue::from("goal-portfolio")),
                ])]),
            ),
            (
                "checkpoints",
                JsonValue::array([JsonValue::object([
                    ("id", JsonValue::from(CHECKPOINT_ID)),
                    ("milestone_id", JsonValue::from("milestone-design")),
                ])]),
            ),
            ("action_links", JsonValue::Array(Vec::new())),
        ]);
        store
            .insert_initial("goals", &goals.as_object().cloned().unwrap())
            .unwrap();
    }

    fn document(store: &me_store::MemoryStore, name: &str) -> JsonValue {
        JsonValue::Object(store.load(name).expect("document"))
    }

    fn version_of(store: &me_store::MemoryStore, name: &str) -> u64 {
        store
            .load(name)
            .expect("document")
            .get("version")
            .and_then(JsonValue::as_u64)
            .unwrap_or(0)
    }

    fn find_action<'a>(document: &'a JsonValue, id: &str) -> &'a JsonValue {
        document
            .get("items")
            .and_then(JsonValue::as_array)
            .and_then(|items| {
                items
                    .iter()
                    .find(|item| item.get("id").and_then(JsonValue::as_str) == Some(id))
            })
            .expect("action exists")
    }

    fn queue_of(store: &me_store::MemoryStore) -> Vec<String> {
        document(store, "actions")
            .get("queue")
            .and_then(JsonValue::as_array)
            .map(|queue| {
                queue
                    .iter()
                    .filter_map(JsonValue::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    fn error_code(response: &JsonValue) -> Option<&str> {
        response
            .get("error")
            .and_then(|error| error.get("code"))
            .and_then(JsonValue::as_str)
    }

    fn invoke(
        store: &me_store::MemoryStore,
        command: &str,
        request_id: &str,
        version: u64,
        input: JsonValue,
    ) -> (u8, JsonValue) {
        let req = JsonValue::object([
            ("request_id", JsonValue::from(request_id)),
            ("expected_version", JsonValue::from(version)),
            ("input", input),
        ]);
        let mut out = Vec::new();
        let code = me_test_support::dispatch(
            store,
            &PROGRAM,
            &[command.to_string()],
            Cursor::new(req.to_json_string()),
            &mut out,
        );
        let parsed = parse_json(&String::from_utf8(out).unwrap()).expect("valid JSON");
        (code, parsed)
    }

    #[test]
    fn contract_describes_without_touching_storage() {
        let store = me_store::MemoryStore::new();
        for args in [
            vec!["--help".to_string()],
            vec!["describe".to_string()],
            vec!["describe".to_string(), "action.create".to_string()],
            vec!["describe".to_string(), "action.complete".to_string()],
            vec!["describe".to_string(), "migrate".to_string()],
        ] {
            let mut out = Vec::new();
            let code = me_test_support::dispatch(
                &store,
                &PROGRAM,
                &args,
                Cursor::new("invalid stdin"),
                &mut out,
            );
            assert_eq!(code, EXIT_SUCCESS);
            let response = parse_json(&String::from_utf8(out).unwrap()).expect("JSON");
            assert_eq!(
                response.get("status").and_then(JsonValue::as_str),
                Some("succeeded")
            );
        }
    }

    #[test]
    fn action_create_defaults_and_recurring_completion() {
        let store = me_store::MemoryStore::new();
        (PROGRAM.init_handler)(&store).expect("init");
        seed_goal_checkpoint(&store);

        let (code, created) = invoke(
            &store,
            "action.create",
            "req-create",
            version_of(&store, "actions"),
            JsonValue::object([
                ("title", JsonValue::from("Daily practice")),
                ("checkpoint_id", JsonValue::from(CHECKPOINT_ID)),
                ("due_date", JsonValue::from("2099-10-16")),
                (
                    "recurrence",
                    JsonValue::object([
                        ("frequency", JsonValue::from("daily")),
                        ("count", JsonValue::from(2_u64)),
                    ]),
                ),
            ]),
        );
        assert_eq!(code, EXIT_SUCCESS, "{created:?}");
        let action_id = created
            .get("changed_records")
            .and_then(JsonValue::as_array)
            .and_then(|items| items.first())
            .and_then(JsonValue::as_str)
            .expect("action id")
            .to_string();
        let stored = document(&store, "actions");
        let action = find_action(&stored, &action_id);
        assert_eq!(
            action.get("duration_minutes").and_then(JsonValue::as_u64),
            Some(30)
        );
        assert_eq!(
            action.get("buffer_minutes").and_then(JsonValue::as_u64),
            Some(30)
        );
        assert_eq!(
            action
                .get("recurrence_start_at")
                .and_then(JsonValue::as_str),
            Some("2099-10-16")
        );
        assert_eq!(
            action
                .get("recurrence_completed_count")
                .and_then(JsonValue::as_u64),
            Some(0)
        );

        let (code, response) = invoke(
            &store,
            "action.complete",
            "req-complete-1",
            1,
            JsonValue::object([("action_id", JsonValue::from(action_id.as_str()))]),
        );
        assert_eq!(code, EXIT_SUCCESS, "{response:?}");
        assert_eq!(
            response.get("next_due").and_then(JsonValue::as_str),
            Some("2099-10-17")
        );
        assert_eq!(
            response
                .get("series_completed")
                .and_then(JsonValue::as_bool),
            Some(false)
        );
        let (code, response) = invoke(
            &store,
            "action.complete",
            "req-complete-2",
            2,
            JsonValue::object([("action_id", JsonValue::from(action_id.as_str()))]),
        );
        assert_eq!(code, EXIT_SUCCESS, "{response:?}");
        assert_eq!(
            response
                .get("series_completed")
                .and_then(JsonValue::as_bool),
            Some(true)
        );
        let stored = document(&store, "actions");
        let action = find_action(&stored, &action_id);
        assert_eq!(
            action.get("status").and_then(JsonValue::as_str),
            Some("done")
        );
        assert!(queue_of(&store).is_empty());
    }

    #[test]
    fn action_create_requires_checkpoint_and_recurrence_requires_due() {
        let store = me_store::MemoryStore::new();
        (PROGRAM.init_handler)(&store).expect("init");
        seed_goal_checkpoint(&store);

        let (code, response) = invoke(
            &store,
            "action.create",
            "req-no-checkpoint",
            version_of(&store, "actions"),
            JsonValue::object([("title", JsonValue::from("Orphan"))]),
        );
        assert_eq!(code, EXIT_FAILURE);
        assert_eq!(error_code(&response), Some("INVALID_INPUT"));

        let (code, response) = invoke(
            &store,
            "action.create",
            "req-no-due",
            version_of(&store, "actions"),
            JsonValue::object([
                ("title", JsonValue::from("No anchor")),
                ("checkpoint_id", JsonValue::from(CHECKPOINT_ID)),
                (
                    "recurrence",
                    JsonValue::object([("frequency", JsonValue::from("daily"))]),
                ),
            ]),
        );
        assert_eq!(code, EXIT_FAILURE);
        assert_eq!(error_code(&response), Some("INVALID_INPUT"));
    }

    #[test]
    fn migrate_backfills_action_recurrence_fields() {
        let store = me_store::MemoryStore::new();
        let actions = JsonValue::object([
            ("schema_version", JsonValue::from(5_u64)),
            ("version", JsonValue::from(1_u64)),
            ("updated_at", JsonValue::from(STAMP)),
            ("updated_by", JsonValue::from("fixture")),
            (
                "items",
                JsonValue::array([JsonValue::object([
                    ("id", JsonValue::from("action-old")),
                    ("version", JsonValue::from(1_u64)),
                    ("status", JsonValue::from("open")),
                    ("title", JsonValue::from("Old action")),
                    ("created_at", JsonValue::from(STAMP)),
                    ("updated_at", JsonValue::from(STAMP)),
                ])]),
            ),
            ("queue", JsonValue::array([JsonValue::from("action-old")])),
        ]);
        store
            .insert_initial("actions", &actions.as_object().cloned().unwrap())
            .unwrap();

        let (code, response) = invoke(
            &store,
            "migrate",
            "req-migrate",
            1,
            JsonValue::object([
                ("collection", JsonValue::from("actions")),
                ("target_schema_version", JsonValue::from(7_u64)),
            ]),
        );
        assert_eq!(code, EXIT_SUCCESS, "{response:?}");
        let stored = document(&store, "actions");
        assert_eq!(
            stored.get("schema_version").and_then(JsonValue::as_u64),
            Some(7)
        );
        let action = find_action(&stored, "action-old");
        assert!(action.get("recurrence").is_some_and(JsonValue::is_null));
        assert_eq!(
            action
                .get("recurrence_completed_count")
                .and_then(JsonValue::as_u64),
            Some(0)
        );
        assert_eq!(
            action.get("duration_minutes").and_then(JsonValue::as_u64),
            Some(30)
        );
    }
}
