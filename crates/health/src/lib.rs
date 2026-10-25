//! `health` domain library for personal body status, daily hydration,
//! weekly diet planning, and sleep management.
//!
//! Follows the unified command protocol:
//! - `GET /api/v1/programs/health`
//! - `GET /api/v1/programs/health/commands/<command>`
//! - `POST /api/v1/programs/health/commands/<command>`
//! - `me-client write health.<command> ...`

mod domain;

pub use domain::validate_document;

use domain::{
    handle_body_log, handle_body_set_profile, handle_body_update, handle_diet_set_meal,
    handle_diet_set_week, handle_init, handle_sleep_log, handle_sleep_set_goal,
    handle_sleep_update, handle_water_log, handle_water_set_goal, handle_water_update,
};
use shared::{CommandSpec, FieldSpec, JsonValue, ProgramSpec};

// ---- example inputs ---------------------------------------------------------

fn example_body_set_profile() -> JsonValue {
    JsonValue::object([(
        "patch",
        JsonValue::object([
            ("height_cm", JsonValue::from(175.0)),
            ("target_weight_kg", JsonValue::from(68.0)),
            ("blood_type", JsonValue::from("O")),
        ]),
    )])
}

fn example_body_log() -> JsonValue {
    JsonValue::object([
        ("date", JsonValue::from("2026-10-16")),
        ("weight_kg", JsonValue::from(69.2)),
        ("body_fat_pct", JsonValue::from(16.5)),
        ("heart_rate_bpm", JsonValue::from(62_u64)),
        ("energy_level", JsonValue::from(4_u64)),
    ])
}

fn example_body_update() -> JsonValue {
    JsonValue::object([
        ("record_id", JsonValue::from("body-001")),
        (
            "patch",
            JsonValue::object([
                ("weight_kg", JsonValue::from(68.9)),
                ("energy_level", JsonValue::from(5_u64)),
            ]),
        ),
    ])
}

fn example_water_set_goal() -> JsonValue {
    JsonValue::object([("daily_goal_ml", JsonValue::from(2200_u64))])
}

fn example_water_log() -> JsonValue {
    JsonValue::object([
        ("date", JsonValue::from("2026-10-16")),
        ("amount_ml", JsonValue::from(350_u64)),
        ("beverage", JsonValue::from("water")),
    ])
}

fn example_water_update() -> JsonValue {
    JsonValue::object([
        ("record_id", JsonValue::from("water-001")),
        (
            "patch",
            JsonValue::object([("amount_ml", JsonValue::from(400_u64))]),
        ),
    ])
}

fn example_diet_set_week() -> JsonValue {
    JsonValue::object([
        ("week_start", JsonValue::from("2026-10-12")),
        ("daily_calorie_target", JsonValue::from(2100_u64)),
        (
            "preferences",
            JsonValue::string_array(&["high-protein", "low-sugar"]),
        ),
        ("notes", JsonValue::from("Regular training week")),
    ])
}

fn example_diet_set_meal() -> JsonValue {
    JsonValue::object([
        ("week_start", JsonValue::from("2026-10-12")),
        ("day_of_week", JsonValue::from("mon")),
        ("meal_type", JsonValue::from("breakfast")),
        (
            "items",
            JsonValue::string_array(&["Oatmeal", "Boiled eggs", "Milk"]),
        ),
        ("calories", JsonValue::from(520_u64)),
    ])
}

fn example_sleep_set_goal() -> JsonValue {
    JsonValue::object([
        ("target_duration_minutes", JsonValue::from(480_u64)),
        ("target_bedtime", JsonValue::from("23:00")),
        ("target_wake_time", JsonValue::from("07:00")),
    ])
}

fn example_sleep_log() -> JsonValue {
    JsonValue::object([
        ("date", JsonValue::from("2026-10-16")),
        ("slept_at", JsonValue::from("2026-10-15T23:15:00+08:00")),
        ("woke_at", JsonValue::from("2026-10-16T07:10:00+08:00")),
        ("duration_minutes", JsonValue::from(475_u64)),
        ("quality_score", JsonValue::from(4_u64)),
        ("deep_sleep_minutes", JsonValue::from(110_u64)),
    ])
}

fn example_sleep_update() -> JsonValue {
    JsonValue::object([
        ("record_id", JsonValue::from("sleep-001")),
        (
            "patch",
            JsonValue::object([
                ("quality_score", JsonValue::from(5_u64)),
                ("notes", JsonValue::from("Rested well")),
            ]),
        ),
    ])
}

// ---- field specifications ---------------------------------------------------

const BODY_SET_PROFILE_FIELDS: &[FieldSpec] = &[FieldSpec {
    name: "patch",
    field_type: "object",
    required: true,
    description: "Non-empty patch object allowing height_cm (30..260), target_weight_kg (15..350), blood_type (A/B/AB/O/A+/A-/B+/B-/AB+/AB-/O+/O-/unknown), allergies (array<string>), conditions (array<string>), notes (string)",
}];

const BODY_LOG_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "date",
        field_type: "string (YYYY-MM-DD)",
        required: true,
        description: "Measurement calendar date",
    },
    FieldSpec {
        name: "weight_kg",
        field_type: "number (15..350) | null",
        required: false,
        description: "Body weight in kilograms",
    },
    FieldSpec {
        name: "body_fat_pct",
        field_type: "number (1..75) | null",
        required: false,
        description: "Body fat percentage",
    },
    FieldSpec {
        name: "heart_rate_bpm",
        field_type: "integer (25..250) | null",
        required: false,
        description: "Resting heart rate in beats per minute",
    },
    FieldSpec {
        name: "systolic_bp",
        field_type: "integer (50..260) | null",
        required: false,
        description: "Systolic blood pressure in mmHg (must be paired with diastolic_bp)",
    },
    FieldSpec {
        name: "diastolic_bp",
        field_type: "integer (30..180) | null",
        required: false,
        description: "Diastolic blood pressure in mmHg (must be less than systolic_bp)",
    },
    FieldSpec {
        name: "energy_level",
        field_type: "integer (1..5) | null",
        required: false,
        description: "Subjective daily energy score from 1 (exhausted) to 5 (peak)",
    },
    FieldSpec {
        name: "symptoms",
        field_type: "array<string>",
        required: false,
        description: "Observed physical symptoms or discomfort tags",
    },
    FieldSpec {
        name: "notes",
        field_type: "string",
        required: false,
        description: "Additional notes on physical condition",
    },
    FieldSpec {
        name: "source",
        field_type: "object { type, reference? }",
        required: false,
        description: "Fact source metadata; defaults to {\"type\": \"user_instruction\"}",
    },
];

const BODY_UPDATE_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "record_id",
        field_type: "string",
        required: true,
        description: "Target body status record ID",
    },
    FieldSpec {
        name: "patch",
        field_type: "object",
        required: true,
        description: "Non-empty patch object allowing date, weight_kg, body_fat_pct, heart_rate_bpm, systolic_bp, diastolic_bp, energy_level, symptoms, notes",
    },
];

const WATER_SET_GOAL_FIELDS: &[FieldSpec] = &[FieldSpec {
    name: "daily_goal_ml",
    field_type: "integer (100..15000)",
    required: true,
    description: "Daily hydration target in milliliters",
}];

const WATER_LOG_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "date",
        field_type: "string (YYYY-MM-DD)",
        required: true,
        description: "Hydration log calendar date",
    },
    FieldSpec {
        name: "amount_ml",
        field_type: "integer (1..5000)",
        required: true,
        description: "Fluid intake volume in milliliters",
    },
    FieldSpec {
        name: "beverage",
        field_type: "string",
        required: false,
        description: "Beverage type (defaults to \"water\")",
    },
    FieldSpec {
        name: "recorded_at",
        field_type: "string (ISO-8601 with offset)",
        required: false,
        description: "Intake timestamp (defaults to current time)",
    },
    FieldSpec {
        name: "notes",
        field_type: "string",
        required: false,
        description: "Optional hydration notes",
    },
    FieldSpec {
        name: "source",
        field_type: "object { type, reference? }",
        required: false,
        description: "Fact source metadata; defaults to {\"type\": \"user_instruction\"}",
    },
];

const WATER_UPDATE_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "record_id",
        field_type: "string",
        required: true,
        description: "Target water intake record ID",
    },
    FieldSpec {
        name: "patch",
        field_type: "object",
        required: true,
        description: "Non-empty patch object allowing date, amount_ml, beverage, recorded_at, notes",
    },
];

const DIET_SET_WEEK_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "week_start",
        field_type: "string (YYYY-MM-DD)",
        required: true,
        description: "Start date of the planned week (typically Monday)",
    },
    FieldSpec {
        name: "daily_calorie_target",
        field_type: "integer (500..10000) | null",
        required: false,
        description: "Target daily calorie intake in kcal",
    },
    FieldSpec {
        name: "preferences",
        field_type: "array<string>",
        required: false,
        description: "Dietary preferences, goals, or restrictions for the week",
    },
    FieldSpec {
        name: "days",
        field_type: "object",
        required: false,
        description: "Mapping of day_of_week (mon..sun) to meal objects (breakfast/lunch/dinner/snack -> { items, calories?, notes? })",
    },
    FieldSpec {
        name: "notes",
        field_type: "string",
        required: false,
        description: "Weekly diet plan notes",
    },
];

const DIET_SET_MEAL_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "week_start",
        field_type: "string (YYYY-MM-DD)",
        required: true,
        description: "Start date of the target week",
    },
    FieldSpec {
        name: "day_of_week",
        field_type: "string (mon | tue | wed | thu | fri | sat | sun)",
        required: true,
        description: "Target day of the week",
    },
    FieldSpec {
        name: "meal_type",
        field_type: "string (breakfast | lunch | dinner | snack)",
        required: true,
        description: "Target meal slot",
    },
    FieldSpec {
        name: "items",
        field_type: "array<string>",
        required: true,
        description: "List of planned foods or dishes for the meal",
    },
    FieldSpec {
        name: "calories",
        field_type: "integer (0..10000) | null",
        required: false,
        description: "Estimated meal calories in kcal",
    },
    FieldSpec {
        name: "notes",
        field_type: "string",
        required: false,
        description: "Preparation or nutrition notes for the meal",
    },
];

const SLEEP_SET_GOAL_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "target_duration_minutes",
        field_type: "integer (60..960)",
        required: false,
        description: "Target sleep duration in minutes (e.g. 480 for 8 hours)",
    },
    FieldSpec {
        name: "target_bedtime",
        field_type: "string (HH:MM) | null",
        required: false,
        description: "Target local bedtime in 24-hour HH:MM format",
    },
    FieldSpec {
        name: "target_wake_time",
        field_type: "string (HH:MM) | null",
        required: false,
        description: "Target local wake time in 24-hour HH:MM format",
    },
];

const SLEEP_LOG_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "date",
        field_type: "string (YYYY-MM-DD)",
        required: true,
        description: "Wake-up or attribution calendar date for the sleep session",
    },
    FieldSpec {
        name: "slept_at",
        field_type: "string (ISO-8601 with offset)",
        required: true,
        description: "Timestamp when sleep started",
    },
    FieldSpec {
        name: "woke_at",
        field_type: "string (ISO-8601 with offset)",
        required: true,
        description: "Timestamp when sleep ended",
    },
    FieldSpec {
        name: "duration_minutes",
        field_type: "integer (1..1440)",
        required: true,
        description: "Total sleep duration in minutes",
    },
    FieldSpec {
        name: "quality_score",
        field_type: "integer (1..5) | null",
        required: false,
        description: "Subjective sleep quality score from 1 (poor) to 5 (excellent)",
    },
    FieldSpec {
        name: "deep_sleep_minutes",
        field_type: "integer (0..1440) | null",
        required: false,
        description: "Deep sleep duration in minutes (must not exceed duration_minutes)",
    },
    FieldSpec {
        name: "awakenings",
        field_type: "integer (0..100)",
        required: false,
        description: "Number of nighttime awakenings (defaults to 0)",
    },
    FieldSpec {
        name: "notes",
        field_type: "string",
        required: false,
        description: "Notes on sleep quality, dreams, or disturbances",
    },
    FieldSpec {
        name: "source",
        field_type: "object { type, reference? }",
        required: false,
        description: "Fact source metadata; defaults to {\"type\": \"user_instruction\"}",
    },
];

const SLEEP_UPDATE_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "record_id",
        field_type: "string",
        required: true,
        description: "Target sleep record ID",
    },
    FieldSpec {
        name: "patch",
        field_type: "object",
        required: true,
        description: "Non-empty patch object allowing date, slept_at, woke_at, duration_minutes, quality_score, deep_sleep_minutes, awakenings, notes",
    },
];

// ---- command table ----------------------------------------------------------

const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        name: "body.set_profile",
        summary: "Update baseline physical profile and target metrics",
        reads: &["health"],
        writes: &["health"],
        expected_version_rule: "Use top-level health collection version",
        fields: BODY_SET_PROFILE_FIELDS,
        rules: &[
            "height_cm must be between 30 and 260 when non-null",
            "target_weight_kg must be between 15 and 350 when non-null",
        ],
        example_version: 1,
        example_input: example_body_set_profile,
        handler: handle_body_set_profile,
    },
    CommandSpec {
        name: "body.log",
        summary: "Record a body status measurement or physical check-in",
        reads: &["health"],
        writes: &["health"],
        expected_version_rule: "Use top-level health collection version",
        fields: BODY_LOG_FIELDS,
        rules: &[
            "Requires date and at least one measurement, symptom, energy_level, or non-empty note",
            "systolic_bp and diastolic_bp must be provided together with systolic_bp > diastolic_bp",
        ],
        example_version: 1,
        example_input: example_body_log,
        handler: handle_body_log,
    },
    CommandSpec {
        name: "body.update",
        summary: "Modify an existing body status record",
        reads: &["health"],
        writes: &["health"],
        expected_version_rule: "Use target body record version",
        fields: BODY_UPDATE_FIELDS,
        rules: &[
            "Increments both the target record version and top-level health collection version",
        ],
        example_version: 1,
        example_input: example_body_update,
        handler: handle_body_update,
    },
    CommandSpec {
        name: "water.set_goal",
        summary: "Set daily hydration target in milliliters",
        reads: &["health"],
        writes: &["health"],
        expected_version_rule: "Use top-level health collection version",
        fields: WATER_SET_GOAL_FIELDS,
        rules: &["daily_goal_ml must be an integer between 100 and 15000"],
        example_version: 1,
        example_input: example_water_set_goal,
        handler: handle_water_set_goal,
    },
    CommandSpec {
        name: "water.log",
        summary: "Log a water or beverage intake entry",
        reads: &["health"],
        writes: &["health"],
        expected_version_rule: "Use top-level health collection version",
        fields: WATER_LOG_FIELDS,
        rules: &["Returns daily_total_ml, daily_goal_ml, and goal_reached for the logged date"],
        example_version: 1,
        example_input: example_water_log,
        handler: handle_water_log,
    },
    CommandSpec {
        name: "water.update",
        summary: "Modify an existing water intake record",
        reads: &["health"],
        writes: &["health"],
        expected_version_rule: "Use target water record version",
        fields: WATER_UPDATE_FIELDS,
        rules: &[
            "Recomputes and returns daily_total_ml and goal_reached for the updated record date",
        ],
        example_version: 1,
        example_input: example_water_update,
        handler: handle_water_update,
    },
    CommandSpec {
        name: "diet.set_week",
        summary: "Create or update a weekly diet plan",
        reads: &["health"],
        writes: &["health"],
        expected_version_rule: "Use top-level health collection version",
        fields: DIET_SET_WEEK_FIELDS,
        rules: &[
            "Identifies weekly plans by week_start (YYYY-MM-DD); creates a new plan or updates the existing one",
        ],
        example_version: 1,
        example_input: example_diet_set_week,
        handler: handle_diet_set_week,
    },
    CommandSpec {
        name: "diet.set_meal",
        summary: "Set or update a single meal slot inside a weekly diet plan",
        reads: &["health"],
        writes: &["health"],
        expected_version_rule: "Use top-level health collection version",
        fields: DIET_SET_MEAL_FIELDS,
        rules: &[
            "Automatically creates the weekly plan for week_start if it does not exist yet",
            "day_of_week must be mon..sun and meal_type must be breakfast, lunch, dinner, or snack",
        ],
        example_version: 1,
        example_input: example_diet_set_meal,
        handler: handle_diet_set_meal,
    },
    CommandSpec {
        name: "sleep.set_goal",
        summary: "Set target sleep duration and schedule",
        reads: &["health"],
        writes: &["health"],
        expected_version_rule: "Use top-level health collection version",
        fields: SLEEP_SET_GOAL_FIELDS,
        rules: &[
            "target_duration_minutes must be between 60 and 960",
            "target_bedtime and target_wake_time use 24-hour HH:MM format",
        ],
        example_version: 1,
        example_input: example_sleep_set_goal,
        handler: handle_sleep_set_goal,
    },
    CommandSpec {
        name: "sleep.log",
        summary: "Log a sleep session",
        reads: &["health"],
        writes: &["health"],
        expected_version_rule: "Use top-level health collection version",
        fields: SLEEP_LOG_FIELDS,
        rules: &[
            "duration_minutes must be between 1 and 1440, and deep_sleep_minutes cannot exceed duration_minutes",
        ],
        example_version: 1,
        example_input: example_sleep_log,
        handler: handle_sleep_log,
    },
    CommandSpec {
        name: "sleep.update",
        summary: "Modify an existing sleep record",
        reads: &["health"],
        writes: &["health"],
        expected_version_rule: "Use target sleep record version",
        fields: SLEEP_UPDATE_FIELDS,
        rules: &[
            "Increments both the target sleep record version and top-level health collection version",
        ],
        example_version: 1,
        example_input: example_sleep_update,
        handler: handle_sleep_update,
    },
];

pub const PROGRAM: ProgramSpec = ProgramSpec {
    name: "health",
    summary: "Body status, daily hydration, weekly diet plans, and sleep management commands",
    collections: &["health"],
    usage: &[
        "GET /api/v1/programs/health",
        "GET /api/v1/programs/health/commands/<command>",
        "POST /api/v1/programs/health/commands/<command>",
        "me-client write health.<command> ...",
    ],
    rules: &[
        "--help and describe do not access the database",
        "Responses contain exactly one JSON value; diagnostics never join the response payload",
        "Writes use the envelope {request_id, expected_version, input} on POST /api/v1/programs/health/commands/<command>; me-client write accepts --input, --input-file, or repeatable --set key=value",
        "All health sections (body, water, diet, sleep) are stored in the `health` collection and written in one SQLite transaction with 128-entry idempotency receipts",
    ],
    error_codes: &[
        (
            "INVALID_INPUT",
            "Invalid request JSON, field values, date/time format, or command arguments",
        ),
        (
            "NOT_FOUND",
            "The `health` collection or the target record ID was not found",
        ),
        (
            "VERSION_CONFLICT",
            "expected_version does not match current document or record version (exit code 4)",
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
            "The stored `health` document is corrupt or could not be decoded",
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
    use shared::{EXIT_SUCCESS, EXIT_VERSION_CONFLICT, parse_json};
    use std::io::Cursor;

    #[test]
    fn health_protocol_commands_return_valid_ascii_json_without_reading_stdin() {
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
                Some("health")
            );
        }
    }

    #[test]
    fn health_end_to_end_body_water_diet_and_sleep_lifecycle() {
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
        let init_res = parse_json(&String::from_utf8(out).unwrap()).unwrap();
        assert_eq!(
            init_res
                .get("collection_version")
                .and_then(JsonValue::as_u64),
            Some(1)
        );

        // 2. body.set_profile via field input
        let mut out = Vec::new();
        let code = me_test_support::dispatch(
            &store,
            &PROGRAM,
            &[
                "body.set_profile".to_string(),
                "--request-id".to_string(),
                "req-body-prof".to_string(),
                "--expected-version".to_string(),
                "1".to_string(),
                "--set".to_string(),
                "patch.height_cm=178.0".to_string(),
                "--set".to_string(),
                "patch.target_weight_kg=70.0".to_string(),
                "--set".to_string(),
                "patch.blood_type=A".to_string(),
            ],
            Cursor::new(""),
            &mut out,
        );
        assert_eq!(code, EXIT_SUCCESS);

        // 3. body.log & body.update
        let mut out = Vec::new();
        let code = me_test_support::dispatch(
            &store,
            &PROGRAM,
            &[
                "body.log".to_string(),
                "--request-id".to_string(),
                "req-body-log".to_string(),
                "--expected-version".to_string(),
                "2".to_string(),
                "--set".to_string(),
                "date=2026-10-16".to_string(),
                "--set".to_string(),
                "weight_kg=71.2".to_string(),
                "--set".to_string(),
                "energy_level=4".to_string(),
            ],
            Cursor::new(""),
            &mut out,
        );
        assert_eq!(code, EXIT_SUCCESS);
        let body_log_res = parse_json(&String::from_utf8(out).unwrap()).unwrap();
        let body_id = body_log_res
            .get("changed_records")
            .and_then(JsonValue::as_array)
            .and_then(|a| a.first())
            .and_then(JsonValue::as_str)
            .unwrap()
            .to_string();

        let mut out = Vec::new();
        let code = me_test_support::dispatch(
            &store,
            &PROGRAM,
            &[
                "body.update".to_string(),
                "--request-id".to_string(),
                "req-body-upd".to_string(),
                "--expected-version".to_string(),
                "1".to_string(),
                "--set".to_string(),
                format!("record_id={body_id}"),
                "--set".to_string(),
                "patch.weight_kg=70.8".to_string(),
            ],
            Cursor::new(""),
            &mut out,
        );
        assert_eq!(code, EXIT_SUCCESS);

        // 4. water.set_goal & water.log
        let mut out = Vec::new();
        let code = me_test_support::dispatch(
            &store,
            &PROGRAM,
            &[
                "water.set_goal".to_string(),
                "--request-id".to_string(),
                "req-water-goal".to_string(),
                "--expected-version".to_string(),
                "4".to_string(),
                "--set".to_string(),
                "daily_goal_ml=1000".to_string(),
            ],
            Cursor::new(""),
            &mut out,
        );
        assert_eq!(code, EXIT_SUCCESS);

        let mut out = Vec::new();
        let code = me_test_support::dispatch(
            &store,
            &PROGRAM,
            &[
                "water.log".to_string(),
                "--request-id".to_string(),
                "req-water-log".to_string(),
                "--expected-version".to_string(),
                "5".to_string(),
                "--set".to_string(),
                "date=2026-10-16".to_string(),
                "--set".to_string(),
                "amount_ml=1200".to_string(),
            ],
            Cursor::new(""),
            &mut out,
        );
        assert_eq!(code, EXIT_SUCCESS);
        let water_res = parse_json(&String::from_utf8(out).unwrap()).unwrap();
        assert_eq!(
            water_res.get("goal_reached").and_then(JsonValue::as_bool),
            Some(true)
        );

        // 5. diet.set_meal (auto-creates weekly plan)
        let mut out = Vec::new();
        let code = me_test_support::dispatch(
            &store,
            &PROGRAM,
            &[
                "diet.set_meal".to_string(),
                "--request-id".to_string(),
                "req-diet-meal".to_string(),
                "--expected-version".to_string(),
                "6".to_string(),
                "--set".to_string(),
                "week_start=2026-10-12".to_string(),
                "--set".to_string(),
                "day_of_week=mon".to_string(),
                "--set".to_string(),
                "meal_type=breakfast".to_string(),
                "--set".to_string(),
                "items=[\"Oatmeal\",\"Eggs\"]".to_string(),
                "--set".to_string(),
                "calories=450".to_string(),
            ],
            Cursor::new(""),
            &mut out,
        );
        assert_eq!(code, EXIT_SUCCESS);

        // 6. sleep.log via JSON stdin
        let sleep_req = JsonValue::object([
            ("request_id", JsonValue::from("req-sleep-log")),
            ("expected_version", JsonValue::from(7_u64)),
            ("input", example_sleep_log()),
        ]);
        let mut out = Vec::new();
        let code = me_test_support::dispatch(
            &store,
            &PROGRAM,
            &["sleep.log".to_string()],
            Cursor::new(sleep_req.to_json_string()),
            &mut out,
        );
        assert_eq!(code, EXIT_SUCCESS);

        // 7. Replay idempotency check: repeating req-sleep-log with old expected_version=7 replays original result
        let mut out_replay = Vec::new();
        let code_replay = me_test_support::dispatch(
            &store,
            &PROGRAM,
            &["sleep.log".to_string()],
            Cursor::new(sleep_req.to_json_string()),
            &mut out_replay,
        );
        assert_eq!(code_replay, EXIT_SUCCESS);

        // 8. Version conflict check with a new request_id and stale version
        let stale_req = JsonValue::object([
            ("request_id", JsonValue::from("req-sleep-stale")),
            ("expected_version", JsonValue::from(7_u64)),
            ("input", example_sleep_log()),
        ]);
        let mut out_conflict = Vec::new();
        let code_conflict = me_test_support::dispatch(
            &store,
            &PROGRAM,
            &["sleep.log".to_string()],
            Cursor::new(stale_req.to_json_string()),
            &mut out_conflict,
        );
        assert_eq!(code_conflict, EXIT_VERSION_CONFLICT);
    }
}
