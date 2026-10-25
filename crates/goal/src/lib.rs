//! `goal` domain library for life planning: goals, milestones, checkpoints,
//! action links, review cadence, and progress evidence in the
//! `goals` collection.
//!
//! Follows the unified command protocol:
//! - `GET /api/v1/programs/goal`
//! - `GET /api/v1/programs/goal/commands/<command>`
//! - `POST /api/v1/programs/goal/commands/<command>`
//! - `me-client write goal.<command> ...`

mod domain;

pub use domain::{migrate_legacy_document, validate_document};

use domain::{
    handle_action_link_remove, handle_action_link_set, handle_checkpoint_create,
    handle_checkpoint_delete, handle_checkpoint_update, handle_goal_create, handle_goal_delete,
    handle_goal_update, handle_init, handle_migrate, handle_milestone_create,
    handle_milestone_delete, handle_milestone_update, handle_progress_record, handle_review_set,
};
use shared::{CommandSpec, FieldSpec, JsonValue, ProgramSpec};

// ---- example inputs ---------------------------------------------------------

fn example_goal_create() -> JsonValue {
    JsonValue::object([
        ("id", JsonValue::from("goal-portfolio")),
        ("parent_goal_ids", JsonValue::string_array(&["goal-career"])),
        (
            "title",
            JsonValue::from("Publish a security tool portfolio"),
        ),
        (
            "description",
            JsonValue::from("Build durable expertise and artifacts"),
        ),
        (
            "purpose",
            JsonValue::from("Turn security knowledge into reusable work"),
        ),
        (
            "success_criteria",
            JsonValue::string_array(&["Ship two runnable tools"]),
        ),
        (
            "constraints",
            JsonValue::string_array(&["Do not trade health for output"]),
        ),
        ("horizon", JsonValue::from("months")),
        ("deadline", JsonValue::from("2027-06-30")),
    ])
}

fn example_goal_update() -> JsonValue {
    JsonValue::object([
        ("goal_id", JsonValue::from("goal-portfolio")),
        (
            "patch",
            JsonValue::object([("parent_goal_ids", JsonValue::string_array(&["goal-career"]))]),
        ),
    ])
}

fn example_milestone_create() -> JsonValue {
    JsonValue::object([
        ("id", JsonValue::from("milestone-design")),
        ("goal_id", JsonValue::from("goal-portfolio")),
        ("title", JsonValue::from("Finish design of first tool")),
        (
            "exit_criteria",
            JsonValue::string_array(&["Scope fixed", "Prototype runs"]),
        ),
        ("due_at", JsonValue::from("2026-10-31")),
    ])
}

fn example_milestone_update() -> JsonValue {
    JsonValue::object([
        ("milestone_id", JsonValue::from("milestone-design")),
        (
            "patch",
            JsonValue::object([
                ("status", JsonValue::from("active")),
                ("progress", JsonValue::from(60_u64)),
            ]),
        ),
    ])
}

fn example_checkpoint_create() -> JsonValue {
    JsonValue::object([
        ("id", JsonValue::from("checkpoint-wrapper")),
        ("milestone_id", JsonValue::from("milestone-design")),
        ("title", JsonValue::from("Write the CLI wrapper")),
        ("estimate_minutes", JsonValue::from(120_u64)),
    ])
}

fn example_checkpoint_update() -> JsonValue {
    JsonValue::object([
        ("checkpoint_id", JsonValue::from("checkpoint-wrapper")),
        (
            "patch",
            JsonValue::object([
                ("status", JsonValue::from("active")),
                ("estimate_minutes", JsonValue::from(90_u64)),
            ]),
        ),
    ])
}

fn example_action_link_set() -> JsonValue {
    JsonValue::object([
        ("action_id", JsonValue::from("action-001")),
        ("goal_id", JsonValue::from("goal-portfolio")),
        ("milestone_id", JsonValue::from("milestone-design")),
        ("checkpoint_id", JsonValue::from("checkpoint-wrapper")),
        ("role", JsonValue::from("core")),
        (
            "contribution",
            JsonValue::from("Write interface boundary notes"),
        ),
    ])
}

fn example_action_link_remove() -> JsonValue {
    JsonValue::object([("action_id", JsonValue::from("action-001"))])
}

fn example_migrate() -> JsonValue {
    JsonValue::object([("target_schema_version", JsonValue::from(2_u64))])
}

fn example_goal_delete() -> JsonValue {
    JsonValue::object([("goal_id", JsonValue::from("goal-portfolio"))])
}

fn example_milestone_delete() -> JsonValue {
    JsonValue::object([("milestone_id", JsonValue::from("milestone-design"))])
}

fn example_checkpoint_delete() -> JsonValue {
    JsonValue::object([("checkpoint_id", JsonValue::from("checkpoint-wrapper"))])
}

fn example_review_set() -> JsonValue {
    JsonValue::object([
        ("cadence", JsonValue::from("weekly")),
        (
            "next_review_at",
            JsonValue::from("2026-10-25T20:00:00+08:00"),
        ),
    ])
}

fn example_progress_record() -> JsonValue {
    JsonValue::object([
        ("target", JsonValue::from("milestone")),
        ("target_id", JsonValue::from("milestone-design")),
        ("kind", JsonValue::from("artifact")),
        (
            "summary",
            JsonValue::from("Finished interface boundary notes"),
        ),
        ("evidence", JsonValue::from("docs/api-boundary.md")),
        ("progress_after", JsonValue::from(60_u64)),
    ])
}

// ---- command field specifications -------------------------------------------

const GOAL_CREATE_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "id",
        field_type: "string (1..=100 identifier characters)",
        required: false,
        description: "Optional stable goal ID; defaults to a generated goal-<hex> value",
    },
    FieldSpec {
        name: "group",
        field_type: "boolean",
        required: false,
        description: "Set true to create a top-level goal group; parent_goal_ids must be empty",
    },
    FieldSpec {
        name: "parent_goal_ids",
        field_type: "array<string>",
        required: false,
        description: "Parent goal IDs; required (non-empty) unless group=true, must exist and remain acyclic",
    },
    FieldSpec {
        name: "title",
        field_type: "string",
        required: true,
        description: "Non-empty goal title",
    },
    FieldSpec {
        name: "description",
        field_type: "string",
        required: false,
        description: "Expected outcome in more detail",
    },
    FieldSpec {
        name: "purpose",
        field_type: "string",
        required: false,
        description: "Why this goal matters",
    },
    FieldSpec {
        name: "horizon",
        field_type: "string",
        required: false,
        description: "Planning horizon label; defaults to long",
    },
    FieldSpec {
        name: "status",
        field_type: "string (proposed | active | paused | achieved | abandoned)",
        required: false,
        description: "Lifecycle status; defaults to active",
    },
    FieldSpec {
        name: "health",
        field_type: "string (on_track | at_risk | blocked | unknown)",
        required: false,
        description: "Momentum indicator; defaults to unknown",
    },
    FieldSpec {
        name: "priority",
        field_type: "string (high | normal | low)",
        required: false,
        description: "Relative priority; defaults to normal",
    },
    FieldSpec {
        name: "start_date",
        field_type: "string (YYYY-MM-DD) | null",
        required: false,
        description: "Optional start date",
    },
    FieldSpec {
        name: "deadline",
        field_type: "string (YYYY-MM-DD) | null",
        required: false,
        description: "Optional deadline; must not precede start_date",
    },
    FieldSpec {
        name: "success_criteria",
        field_type: "array<string>",
        required: true,
        description: "Non-empty list of verifiable success criteria",
    },
    FieldSpec {
        name: "constraints",
        field_type: "array<string>",
        required: false,
        description: "Hard constraints that must not be violated",
    },
    FieldSpec {
        name: "progress",
        field_type: "integer (0..100)",
        required: false,
        description: "Completion percentage; defaults to 0",
    },
    FieldSpec {
        name: "progress_basis",
        field_type: "string (manual | milestone_weighted)",
        required: false,
        description: "How progress was derived; defaults to manual",
    },
    FieldSpec {
        name: "last_progress_at",
        field_type: "string (ISO-8601 with offset) | null",
        required: false,
        description: "When verified progress last happened",
    },
    FieldSpec {
        name: "next_review_at",
        field_type: "string (ISO-8601 with offset) | null",
        required: false,
        description: "Next goal review time",
    },
    FieldSpec {
        name: "next_action",
        field_type: "string",
        required: false,
        description: "Smallest executable next step",
    },
    FieldSpec {
        name: "blockers",
        field_type: "array<string>",
        required: false,
        description: "Current blockers with source and release condition",
    },
    FieldSpec {
        name: "risks",
        field_type: "array<string>",
        required: false,
        description: "Known risks to the goal",
    },
    FieldSpec {
        name: "assumptions",
        field_type: "array<string>",
        required: false,
        description: "Assumptions the plan depends on",
    },
];

const GOAL_UPDATE_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "goal_id",
        field_type: "string",
        required: true,
        description: "Target goal ID",
    },
    FieldSpec {
        name: "patch",
        field_type: "object",
        required: false,
        description: "Optional patch allowing parent_goal_ids, purpose, constraints, title, description, horizon, status, health, priority, start_date, deadline, success_criteria, progress, progress_basis, milestone_ids, last_progress_at, next_review_at, next_action, blockers, risks, assumptions",
    },
    FieldSpec {
        name: "recalculate_progress",
        field_type: "boolean",
        required: false,
        description: "When true, sets progress to the rounded average of linked milestone progress and sets progress_basis=milestone_weighted (overrides progress in patch)",
    },
];

const MILESTONE_CREATE_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "id",
        field_type: "string (1..=100 identifier characters)",
        required: false,
        description: "Optional stable milestone ID; defaults to a generated milestone-<hex> value",
    },
    FieldSpec {
        name: "goal_id",
        field_type: "string",
        required: true,
        description: "Existing goal ID that owns the milestone",
    },
    FieldSpec {
        name: "title",
        field_type: "string",
        required: true,
        description: "Non-empty milestone title",
    },
    FieldSpec {
        name: "sequence",
        field_type: "integer (>=1)",
        required: false,
        description: "Order inside the goal; defaults to the next free sequence",
    },
    FieldSpec {
        name: "status",
        field_type: "string (planned | active | blocked | completed | skipped)",
        required: false,
        description: "Lifecycle status; defaults to planned",
    },
    FieldSpec {
        name: "health",
        field_type: "string (on_track | at_risk | blocked | unknown)",
        required: false,
        description: "Momentum indicator; defaults to unknown",
    },
    FieldSpec {
        name: "start_date",
        field_type: "string (YYYY-MM-DD) | null",
        required: false,
        description: "Optional start date",
    },
    FieldSpec {
        name: "due_at",
        field_type: "string (YYYY-MM-DD) | null",
        required: false,
        description: "Optional due date; must not precede start_date",
    },
    FieldSpec {
        name: "exit_criteria",
        field_type: "array<string>",
        required: true,
        description: "Non-empty list of clear exit criteria",
    },
    FieldSpec {
        name: "progress",
        field_type: "integer (0..100)",
        required: false,
        description: "Completion percentage; defaults to 0",
    },
    FieldSpec {
        name: "action_ids",
        field_type: "array<string>",
        required: false,
        description: "Action IDs from the actions collection serving this milestone; validated when the actions collection exists",
    },
    FieldSpec {
        name: "dependencies",
        field_type: "array<string>",
        required: false,
        description: "Other milestone IDs that must finish first",
    },
    FieldSpec {
        name: "next_action",
        field_type: "string",
        required: false,
        description: "Smallest executable next step",
    },
    FieldSpec {
        name: "blockers",
        field_type: "array<string>",
        required: false,
        description: "Current blockers with source and release condition",
    },
    FieldSpec {
        name: "last_progress_at",
        field_type: "string (ISO-8601 with offset) | null",
        required: false,
        description: "When verified progress last happened",
    },
];

const MILESTONE_UPDATE_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "milestone_id",
        field_type: "string",
        required: true,
        description: "Target milestone ID",
    },
    FieldSpec {
        name: "patch",
        field_type: "object",
        required: true,
        description: "Non-empty patch allowing goal_id (moves ownership), title, sequence, status, health, start_date, due_at, exit_criteria, progress, action_ids, dependencies, next_action, blockers, last_progress_at",
    },
];

const CHECKPOINT_CREATE_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "id",
        field_type: "string",
        required: false,
        description: "Optional checkpoint ID; generated when omitted",
    },
    FieldSpec {
        name: "milestone_id",
        field_type: "string",
        required: true,
        description: "Short-term goal (milestone) this hour-level step belongs to",
    },
    FieldSpec {
        name: "title",
        field_type: "string",
        required: true,
        description: "Non-empty checkpoint title",
    },
    FieldSpec {
        name: "description",
        field_type: "string",
        required: false,
        description: "What this step produces; may be empty",
    },
    FieldSpec {
        name: "status",
        field_type: "string (planned | active | done | cancelled)",
        required: false,
        description: "Checkpoint status; defaults to planned",
    },
    FieldSpec {
        name: "estimate_minutes",
        field_type: "integer (1..600)",
        required: false,
        description: "Expected effort in minutes; defaults to 60",
    },
];

const CHECKPOINT_UPDATE_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "checkpoint_id",
        field_type: "string",
        required: true,
        description: "Target checkpoint ID",
    },
    FieldSpec {
        name: "patch",
        field_type: "object",
        required: true,
        description: "Non-empty patch allowing milestone_id, title, description, status, estimate_minutes",
    },
];

const ACTION_LINK_SET_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "action_id",
        field_type: "string",
        required: true,
        description: "Action ID from the actions collection; must exist when the actions collection exists",
    },
    FieldSpec {
        name: "goal_ids",
        field_type: "array<string>",
        required: false,
        description: "Goal IDs this action contributes to",
    },
    FieldSpec {
        name: "goal_id",
        field_type: "string | null",
        required: false,
        description: "Goal ID this action contributes to",
    },
    FieldSpec {
        name: "milestone_id",
        field_type: "string | null",
        required: false,
        description: "Milestone ID this action contributes to; must belong to goal_id when both are set",
    },
    FieldSpec {
        name: "checkpoint_id",
        field_type: "string | null",
        required: false,
        description: "Checkpoint (hour-level step) this action implements; must belong to milestone_id and goal_id when set",
    },
    FieldSpec {
        name: "role",
        field_type: "string (core | supporting | maintenance)",
        required: false,
        description: "Contribution role; defaults to core",
    },
    FieldSpec {
        name: "contribution",
        field_type: "string",
        required: false,
        description: "How the action advances the linked entity",
    },
];

const ACTION_LINK_REMOVE_FIELDS: &[FieldSpec] = &[FieldSpec {
    name: "action_id",
    field_type: "string",
    required: true,
    description: "Action ID whose link should be removed",
}];

const DELETE_GOAL_FIELDS: &[FieldSpec] = &[FieldSpec {
    name: "goal_id",
    field_type: "string",
    required: true,
    description: "Target goal/group ID; its descendant goals, milestones, checkpoints and action links are deleted",
}];

const DELETE_MILESTONE_FIELDS: &[FieldSpec] = &[FieldSpec {
    name: "milestone_id",
    field_type: "string",
    required: true,
    description: "Target milestone ID; its checkpoints and action links are deleted",
}];

const DELETE_CHECKPOINT_FIELDS: &[FieldSpec] = &[FieldSpec {
    name: "checkpoint_id",
    field_type: "string",
    required: true,
    description: "Target checkpoint ID; its action links are deleted",
}];

const MIGRATE_FIELDS: &[FieldSpec] = &[FieldSpec {
    name: "target_schema_version",
    field_type: "integer",
    required: false,
    description: "Target schema version; only 2 is supported",
}];

const REVIEW_SET_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "cadence",
        field_type: "string",
        required: false,
        description: "Review cadence label such as weekly or monthly",
    },
    FieldSpec {
        name: "next_review_at",
        field_type: "string (ISO-8601 with offset) | null",
        required: false,
        description: "Next review time",
    },
];

const PROGRESS_RECORD_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "target",
        field_type: "string (goal | milestone)",
        required: true,
        description: "Entity kind receiving the progress event",
    },
    FieldSpec {
        name: "target_id",
        field_type: "string",
        required: true,
        description: "Goal or milestone ID",
    },
    FieldSpec {
        name: "at",
        field_type: "string (ISO-8601 with offset)",
        required: false,
        description: "Evidence timestamp; defaults to now",
    },
    FieldSpec {
        name: "kind",
        field_type: "string",
        required: true,
        description: "Evidence kind such as artifact, decision, or note",
    },
    FieldSpec {
        name: "summary",
        field_type: "string",
        required: true,
        description: "Non-empty evidence summary",
    },
    FieldSpec {
        name: "evidence",
        field_type: "string",
        required: false,
        description: "Path or reference to the evidence",
    },
    FieldSpec {
        name: "progress_after",
        field_type: "integer (0..100)",
        required: true,
        description: "Completion percentage after this evidence; updates the record progress and last_progress_at",
    },
];

// ---- command table ----------------------------------------------------------

const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        name: "goal.create",
        summary: "Create a goal under an existing parent, or a top-level goal group",
        reads: &["goals"],
        writes: &["goals"],
        expected_version_rule: "Use top-level goals collection version",
        fields: GOAL_CREATE_FIELDS,
        rules: &[
            "A concrete goal requires non-empty parent_goal_ids that reference existing goals",
            "A goal group requires group=true and empty parent_goal_ids; groups have no parent",
            "success_criteria must not be empty; milestone_ids are managed by milestone.create and goal.update",
        ],
        example_version: 1,
        example_input: example_goal_create,
        handler: handle_goal_create,
    },
    CommandSpec {
        name: "goal.update",
        summary: "Update a long-term goal, including milestone-weighted progress",
        reads: &["goals"],
        writes: &["goals"],
        expected_version_rule: "Use top-level goals collection version",
        fields: GOAL_UPDATE_FIELDS,
        rules: &[
            "recalculate_progress averages progress across linked milestones and sets progress_basis=milestone_weighted",
            "milestone_ids must reference milestones owned by this goal",
        ],
        example_version: 1,
        example_input: example_goal_update,
        handler: handle_goal_update,
    },
    CommandSpec {
        name: "milestone.create",
        summary: "Create a milestone inside a goal",
        reads: &["goals", "actions"],
        writes: &["goals"],
        expected_version_rule: "Use top-level goals collection version",
        fields: MILESTONE_CREATE_FIELDS,
        rules: &[
            "exit_criteria must not be empty",
            "The new milestone ID is appended to the owning goal milestone_ids",
            "action_ids are checked against the actions collection when it exists",
            "Milestones are stored grouped by goal and sorted by start_date (due_at fallback; undated last)",
        ],
        example_version: 1,
        example_input: example_milestone_create,
        handler: handle_milestone_create,
    },
    CommandSpec {
        name: "milestone.update",
        summary: "Update a milestone or move it to another goal",
        reads: &["goals", "actions"],
        writes: &["goals"],
        expected_version_rule: "Use top-level goals collection version",
        fields: MILESTONE_UPDATE_FIELDS,
        rules: &[
            "Changing goal_id updates milestone_ids on both the old and new goal",
            "dependencies must reference existing milestones and cannot include the milestone itself",
            "Changing dates or goal_id re-sorts milestones by start_date within the goal",
        ],
        example_version: 1,
        example_input: example_milestone_update,
        handler: handle_milestone_update,
    },
    CommandSpec {
        name: "checkpoint.create",
        summary: "Create an hour-level checkpoint under a short-term goal",
        reads: &["goals"],
        writes: &["goals"],
        expected_version_rule: "Use top-level goals collection version",
        fields: CHECKPOINT_CREATE_FIELDS,
        rules: &[
            "milestone_id must reference an existing short-term goal (milestone)",
            "estimate_minutes is the expected effort and must be between 1 and 600",
            "Actions that implement this step link to it with action_link.set checkpoint_id",
        ],
        example_version: 1,
        example_input: example_checkpoint_create,
        handler: handle_checkpoint_create,
    },
    CommandSpec {
        name: "checkpoint.update",
        summary: "Update an hour-level checkpoint or move it to another short-term goal",
        reads: &["goals"],
        writes: &["goals"],
        expected_version_rule: "Use top-level goals collection version",
        fields: CHECKPOINT_UPDATE_FIELDS,
        rules: &[
            "Changing milestone_id moves the step under another short-term goal",
            "status is planned, active, done, or cancelled",
        ],
        example_version: 1,
        example_input: example_checkpoint_update,
        handler: handle_checkpoint_update,
    },
    CommandSpec {
        name: "action_link.set",
        summary: "Create or replace the life-planning links for an action",
        reads: &["goals", "actions"],
        writes: &["goals"],
        expected_version_rule: "Use top-level goals collection version",
        fields: ACTION_LINK_SET_FIELDS,
        rules: &[
            "The goals collection does not copy action titles, durations, or statuses; only action IDs are stored",
            "The link must reference at least one goal, milestone, or checkpoint",
            "checkpoint_id must belong to milestone_id and goal_id when those are set",
            "When the actions collection exists, the action ID must exist there",
        ],
        example_version: 1,
        example_input: example_action_link_set,
        handler: handle_action_link_set,
    },
    CommandSpec {
        name: "action_link.remove",
        summary: "Remove the life-planning links for an action",
        reads: &["goals"],
        writes: &["goals"],
        expected_version_rule: "Use top-level goals collection version",
        fields: ACTION_LINK_REMOVE_FIELDS,
        rules: &["Removing a missing action link returns NOT_FOUND"],
        example_version: 1,
        example_input: example_action_link_remove,
        handler: handle_action_link_remove,
    },
    CommandSpec {
        name: "goal.delete",
        summary: "Delete a goal/group and all descendant planning structure",
        reads: &["goals"],
        writes: &["goals"],
        expected_version_rule: "Use top-level goals collection version",
        fields: DELETE_GOAL_FIELDS,
        rules: &[
            "Deletes descendant goals, their milestones, checkpoints and action links",
            "Linked actions themselves are not deleted",
            "Action links are the only action association removed",
        ],
        example_version: 1,
        example_input: example_goal_delete,
        handler: handle_goal_delete,
    },
    CommandSpec {
        name: "milestone.delete",
        summary: "Delete a milestone with its checkpoints and action links",
        reads: &["goals"],
        writes: &["goals"],
        expected_version_rule: "Use top-level goals collection version",
        fields: DELETE_MILESTONE_FIELDS,
        rules: &[
            "Deletes the milestone, its checkpoints and action links",
            "Removes the milestone from parent goal milestone_ids and remaining dependencies",
            "Linked actions themselves are not deleted",
        ],
        example_version: 1,
        example_input: example_milestone_delete,
        handler: handle_milestone_delete,
    },
    CommandSpec {
        name: "checkpoint.delete",
        summary: "Delete a checkpoint and its action links",
        reads: &["goals"],
        writes: &["goals"],
        expected_version_rule: "Use top-level goals collection version",
        fields: DELETE_CHECKPOINT_FIELDS,
        rules: &["Deletes the checkpoint and action links that point to it"],
        example_version: 1,
        example_input: example_checkpoint_delete,
        handler: handle_checkpoint_delete,
    },
    CommandSpec {
        name: "migrate",
        summary: "Migrate goals schema v1 to v2 (directions become goals)",
        reads: &["goals", "actions"],
        writes: &["goals"],
        expected_version_rule: "Use the current top-level goals collection version",
        fields: MIGRATE_FIELDS,
        rules: &[
            "Directions become top-level goals; direction_ids become parent_goal_ids",
            "current_goals become checkpoints and task_links become action_links",
            "Migration is idempotent once schema_version=2",
        ],
        example_version: 1,
        example_input: example_migrate,
        handler: handle_migrate,
    },
    CommandSpec {
        name: "review.set",
        summary: "Update the periodic life-planning review cadence",
        reads: &["goals"],
        writes: &["goals"],
        expected_version_rule: "Use top-level goals collection version",
        fields: REVIEW_SET_FIELDS,
        rules: &[
            "Provide cadence, next_review_at, or both",
            "review.next_review_at is a reminder, not an action deadline",
        ],
        example_version: 1,
        example_input: example_review_set,
        handler: handle_review_set,
    },
    CommandSpec {
        name: "progress.record",
        summary: "Append progress evidence and update progress for a goal or milestone",
        reads: &["goals"],
        writes: &["goals"],
        expected_version_rule: "Use top-level goals collection version",
        fields: PROGRESS_RECORD_FIELDS,
        rules: &[
            "progress_events are audit evidence; current progress and last_progress_at remain authoritative",
            "Recording progress on a goal sets progress_basis=manual",
        ],
        example_version: 1,
        example_input: example_progress_record,
        handler: handle_progress_record,
    },
];

pub const PROGRAM: ProgramSpec = ProgramSpec {
    name: "goal",
    summary: "Life planning in four levels: goal (all horizons), milestone, checkpoint, and action",
    collections: &["goals"],
    usage: &[
        "GET /api/v1/programs/goal",
        "GET /api/v1/programs/goal/commands/<command>",
        "POST /api/v1/programs/goal/commands/<command>",
        "me-client write goal.<command> ...",
    ],
    rules: &[
        "--help and describe do not access the database",
        "Responses contain exactly one JSON value; diagnostics never join the response payload",
        "Writes use the envelope {request_id, expected_version, input} on POST /api/v1/programs/goal/commands/<command>; me-client write accepts --input, --input-file, or repeatable --set key=value",
        "The `goals` collection is the fact source for goals, milestones, checkpoints, and action links; the `actions` collection remains the fact source for action details",
        "Levels: top-level goal groups (group=true) and parented goals contain milestones, milestones contain checkpoints, and new actions link to a checkpoint through action_links",
        "An action link may point at its checkpoint so the queue shows which hour-level step an action implements",
        "Deleting a goal/group cascades to descendant goals, milestones, checkpoints and action links; deleting a milestone cascades to checkpoints and links; linked actions remain",
        "status is lifecycle, health is momentum, progress is 0..100 completion, and next_action is the smallest executable step; paused/achieved/abandoned/completed do not drive default decisions",
        "All writes run in one SQLite transaction with top-level version checks and 128-entry idempotency receipts",
    ],
    error_codes: &[
        (
            "INVALID_INPUT",
            "Invalid request JSON, field values, identifier, date/time format, or command arguments",
        ),
        (
            "NOT_FOUND",
            "The `goals` collection or a referenced goal/milestone/checkpoint/action was not found",
        ),
        (
            "VERSION_CONFLICT",
            "expected_version does not match the current `goals` collection version (exit code 4)",
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
            "The stored `goals` document is corrupt or cross references are inconsistent",
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
    use shared::{DocumentStore, EXIT_FAILURE, EXIT_SUCCESS, EXIT_VERSION_CONFLICT, parse_json};
    use std::io::Cursor;

    trait InsertStr {
        fn insert_str(&mut self, key: &str, value: JsonValue);
    }

    impl InsertStr for shared::Document {
        fn insert_str(&mut self, key: &str, value: JsonValue) {
            self.insert(key.to_string(), value);
        }
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

    fn invoke(
        store: &me_store::MemoryStore,
        command: &str,
        request_id: &str,
        version: u64,
        input: JsonValue,
    ) -> (u8, JsonValue) {
        let request = JsonValue::object([
            ("request_id", JsonValue::from(request_id)),
            ("expected_version", JsonValue::from(version)),
            ("input", input),
        ]);
        let mut out = Vec::new();
        let code = me_test_support::dispatch(
            store,
            &PROGRAM,
            &[command.to_string()],
            Cursor::new(request.to_json_string()),
            &mut out,
        );
        let parsed = parse_json(&String::from_utf8(out).unwrap()).expect("valid JSON");
        (code, parsed)
    }

    fn find_in_collection<'a>(
        document: &'a JsonValue,
        collection: &str,
        id_field: &str,
        id: &str,
    ) -> &'a JsonValue {
        document
            .get(collection)
            .and_then(JsonValue::as_array)
            .and_then(|items| {
                items
                    .iter()
                    .find(|item| item.get(id_field).and_then(JsonValue::as_str) == Some(id))
            })
            .expect("record exists")
    }

    fn has_record(document: &JsonValue, collection: &str, id_field: &str, id: &str) -> bool {
        document
            .get(collection)
            .and_then(JsonValue::as_array)
            .is_some_and(|items| {
                items
                    .iter()
                    .any(|item| item.get(id_field).and_then(JsonValue::as_str) == Some(id))
            })
    }

    fn error_code(response: &JsonValue) -> Option<&str> {
        response
            .get("error")
            .and_then(|error| error.get("code"))
            .and_then(JsonValue::as_str)
    }

    fn init(store: &me_store::MemoryStore) {
        let mut out = Vec::new();
        let code = me_test_support::dispatch(
            store,
            &PROGRAM,
            &["init".to_string()],
            Cursor::new(""),
            &mut out,
        );
        assert_eq!(code, EXIT_SUCCESS);
    }

    fn seed_actions(store: &me_store::MemoryStore) {
        let mut document = shared::Document::new();
        document.insert_str("schema_version", JsonValue::from(4_u64));
        document.insert_str("version", JsonValue::from(1_u64));
        document.insert_str("updated_at", JsonValue::from(shared::now_iso8601()));
        document.insert_str("updated_by", JsonValue::from("test"));
        document.insert_str(
            "items",
            JsonValue::Array(vec![JsonValue::object([
                ("id", JsonValue::from("action-001")),
                ("title", JsonValue::from("Write boundary notes")),
            ])]),
        );
        document.insert_str("queue", JsonValue::Array(Vec::new()));
        store.insert_initial("actions", &document).unwrap();
    }

    fn create_goal(
        store: &me_store::MemoryStore,
        request_id: &str,
        input: JsonValue,
    ) -> (u8, JsonValue) {
        invoke(
            store,
            "goal.create",
            request_id,
            version_of(store, "goals"),
            input,
        )
    }

    #[test]
    fn goal_protocol_commands_return_valid_ascii_json_without_reading_stdin() {
        let store = me_store::MemoryStore::new();
        let mut command_args = vec![
            vec!["--help".to_string()],
            vec!["describe".to_string()],
            vec!["describe".to_string(), "init".to_string()],
        ];
        for command in COMMANDS {
            command_args.push(vec!["describe".to_string(), command.name.to_string()]);
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
            let output = String::from_utf8(out).expect("stdout must be UTF-8");
            assert!(
                output.is_ascii(),
                "protocol response must be ASCII-compatible: {output}"
            );
            let response = parse_json(&output).expect("stdout must be valid JSON");
            assert_eq!(
                response.get("status").and_then(JsonValue::as_str),
                Some("succeeded")
            );
            assert_eq!(
                response.get("program").and_then(JsonValue::as_str),
                Some("goal")
            );
        }
    }

    #[test]
    fn goals_init_creates_schema_v2_collection() {
        let store = me_store::MemoryStore::new();
        init(&store);
        let goals = document(&store, "goals");
        assert_eq!(
            goals.get("schema_version").and_then(JsonValue::as_u64),
            Some(2)
        );
        assert!(goals.get("directions").is_none());
        assert!(goals.get("checkpoints").is_some());
        assert!(goals.get("action_links").is_some());
    }

    #[test]
    fn goal_lifecycle_links_goal_milestone_checkpoint_and_action() {
        let store = me_store::MemoryStore::new();
        init(&store);
        seed_actions(&store);

        let (code, _) = create_goal(
            &store,
            "req-parent",
            JsonValue::object([
                ("id", JsonValue::from("goal-career")),
                ("group", JsonValue::from(true)),
                ("title", JsonValue::from("Career direction")),
                ("purpose", JsonValue::from("Build durable leverage")),
                (
                    "constraints",
                    JsonValue::string_array(&["Do not trade health for output"]),
                ),
                (
                    "success_criteria",
                    JsonValue::string_array(&["Publish two artifacts"]),
                ),
                ("horizon", JsonValue::from("years")),
                ("start_date", JsonValue::from("2026-10-01")),
                ("deadline", JsonValue::from("2027-12-31")),
            ]),
        );
        assert_eq!(code, EXIT_SUCCESS);

        let (code, response) = create_goal(
            &store,
            "req-missing-parent",
            JsonValue::object([
                (
                    "parent_goal_ids",
                    JsonValue::string_array(&["goal-missing"]),
                ),
                ("title", JsonValue::from("Missing parent")),
                ("success_criteria", JsonValue::string_array(&["Something"])),
            ]),
        );
        assert_eq!(code, EXIT_FAILURE);
        assert_eq!(error_code(&response), Some("NOT_FOUND"));

        let (code, response) = create_goal(
            &store,
            "req-missing-criteria",
            JsonValue::object([
                ("parent_goal_ids", JsonValue::string_array(&["goal-career"])),
                ("title", JsonValue::from("Missing criteria")),
            ]),
        );
        assert_eq!(code, EXIT_FAILURE);
        assert_eq!(error_code(&response), Some("INVALID_INPUT"));

        let (code, response) = create_goal(
            &store,
            "req-bad-dates",
            JsonValue::object([
                ("parent_goal_ids", JsonValue::string_array(&["goal-career"])),
                ("title", JsonValue::from("Bad dates")),
                ("success_criteria", JsonValue::string_array(&["Something"])),
                ("start_date", JsonValue::from("2027-01-01")),
                ("deadline", JsonValue::from("2026-01-01")),
            ]),
        );
        assert_eq!(code, EXIT_FAILURE);
        assert_eq!(error_code(&response), Some("INVALID_INPUT"));

        let (code, _) = create_goal(
            &store,
            "req-child",
            JsonValue::object([
                ("id", JsonValue::from("goal-portfolio")),
                ("parent_goal_ids", JsonValue::string_array(&["goal-career"])),
                ("title", JsonValue::from("Publish a security portfolio")),
                (
                    "success_criteria",
                    JsonValue::string_array(&["Ship two runnable tools"]),
                ),
                ("horizon", JsonValue::from("months")),
                ("start_date", JsonValue::from("2026-10-01")),
                ("deadline", JsonValue::from("2027-06-30")),
            ]),
        );
        assert_eq!(code, EXIT_SUCCESS);
        let stored = document(&store, "goals");
        let parent = find_in_collection(&stored, "goals", "id", "goal-portfolio");
        assert_eq!(
            parent
                .get("parent_goal_ids")
                .and_then(JsonValue::as_array)
                .and_then(|items| items.first())
                .and_then(JsonValue::as_str),
            Some("goal-career")
        );
        assert!(parent.get("created_at").is_some());

        let (code, _) = invoke(
            &store,
            "milestone.create",
            "req-milestone",
            version_of(&store, "goals"),
            JsonValue::object([
                ("id", JsonValue::from("milestone-design")),
                ("goal_id", JsonValue::from("goal-portfolio")),
                ("title", JsonValue::from("Finish design of first tool")),
                (
                    "exit_criteria",
                    JsonValue::string_array(&["Scope fixed", "Prototype runs"]),
                ),
                ("start_date", JsonValue::from("2026-10-01")),
                ("due_at", JsonValue::from("2026-10-31")),
                ("action_ids", JsonValue::string_array(&["action-001"])),
            ]),
        );
        assert_eq!(code, EXIT_SUCCESS);
        let stored = document(&store, "goals");
        assert_eq!(
            find_in_collection(&stored, "goals", "id", "goal-portfolio")
                .get("milestone_ids")
                .and_then(JsonValue::as_array)
                .and_then(|items| items.first())
                .and_then(JsonValue::as_str),
            Some("milestone-design")
        );

        let (code, response) = invoke(
            &store,
            "milestone.create",
            "req-milestone-missing-action",
            version_of(&store, "goals"),
            JsonValue::object([
                ("goal_id", JsonValue::from("goal-portfolio")),
                ("title", JsonValue::from("Missing action")),
                ("exit_criteria", JsonValue::string_array(&["Something"])),
                ("action_ids", JsonValue::string_array(&["action-missing"])),
            ]),
        );
        assert_eq!(code, EXIT_FAILURE);
        assert_eq!(error_code(&response), Some("NOT_FOUND"));

        let (code, _) = invoke(
            &store,
            "checkpoint.create",
            "req-checkpoint",
            version_of(&store, "goals"),
            JsonValue::object([
                ("id", JsonValue::from("checkpoint-wrapper")),
                ("milestone_id", JsonValue::from("milestone-design")),
                ("title", JsonValue::from("Write the CLI wrapper")),
                ("estimate_minutes", JsonValue::from(120_u64)),
            ]),
        );
        assert_eq!(code, EXIT_SUCCESS);

        let (code, response) = invoke(
            &store,
            "checkpoint.create",
            "req-checkpoint-missing-milestone",
            version_of(&store, "goals"),
            JsonValue::object([
                ("milestone_id", JsonValue::from("milestone-missing")),
                ("title", JsonValue::from("Orphan")),
            ]),
        );
        assert_eq!(code, EXIT_FAILURE);
        assert_eq!(error_code(&response), Some("NOT_FOUND"));

        let (code, _) = invoke(
            &store,
            "action_link.set",
            "req-link",
            version_of(&store, "goals"),
            JsonValue::object([
                ("action_id", JsonValue::from("action-001")),
                ("goal_ids", JsonValue::string_array(&["goal-career"])),
                ("goal_id", JsonValue::from("goal-portfolio")),
                ("milestone_id", JsonValue::from("milestone-design")),
                ("checkpoint_id", JsonValue::from("checkpoint-wrapper")),
                ("role", JsonValue::from("core")),
                (
                    "contribution",
                    JsonValue::from("Write interface boundary notes"),
                ),
            ]),
        );
        assert_eq!(code, EXIT_SUCCESS);
        let stored = document(&store, "goals");
        let link = find_in_collection(&stored, "action_links", "action_id", "action-001");
        assert_eq!(
            link.get("checkpoint_id").and_then(JsonValue::as_str),
            Some("checkpoint-wrapper")
        );

        let (code, response) = invoke(
            &store,
            "action_link.set",
            "req-link-mismatch",
            version_of(&store, "goals"),
            JsonValue::object([
                ("action_id", JsonValue::from("action-001")),
                ("goal_id", JsonValue::from("goal-career")),
                ("milestone_id", JsonValue::from("milestone-design")),
            ]),
        );
        assert_eq!(code, EXIT_FAILURE);
        assert_eq!(error_code(&response), Some("INVALID_INPUT"));

        let (code, _) = invoke(
            &store,
            "review.set",
            "req-review",
            version_of(&store, "goals"),
            JsonValue::object([
                ("cadence", JsonValue::from("weekly")),
                (
                    "next_review_at",
                    JsonValue::from("2026-11-01T20:00:00+08:00"),
                ),
            ]),
        );
        assert_eq!(code, EXIT_SUCCESS);

        let (code, _) = invoke(
            &store,
            "progress.record",
            "req-progress",
            version_of(&store, "goals"),
            JsonValue::object([
                ("target", JsonValue::from("milestone")),
                ("target_id", JsonValue::from("milestone-design")),
                ("kind", JsonValue::from("artifact")),
                ("summary", JsonValue::from("Finished notes")),
                ("evidence", JsonValue::from("docs/notes.md")),
                ("progress_after", JsonValue::from(60_u64)),
            ]),
        );
        assert_eq!(code, EXIT_SUCCESS);
        let stored = document(&store, "goals");
        assert_eq!(
            find_in_collection(&stored, "milestones", "id", "milestone-design")
                .get("progress")
                .and_then(JsonValue::as_u64),
            Some(60)
        );

        let (code, _) = invoke(
            &store,
            "goal.update",
            "req-recalc",
            version_of(&store, "goals"),
            JsonValue::object([
                ("goal_id", JsonValue::from("goal-portfolio")),
                ("recalculate_progress", JsonValue::from(true)),
            ]),
        );
        assert_eq!(code, EXIT_SUCCESS);
        let stored = document(&store, "goals");
        assert_eq!(
            find_in_collection(&stored, "goals", "id", "goal-portfolio")
                .get("progress")
                .and_then(JsonValue::as_u64),
            Some(60)
        );

        let (code, _) = invoke(
            &store,
            "checkpoint.update",
            "req-checkpoint-done",
            version_of(&store, "goals"),
            JsonValue::object([
                ("checkpoint_id", JsonValue::from("checkpoint-wrapper")),
                (
                    "patch",
                    JsonValue::object([("status", JsonValue::from("done"))]),
                ),
            ]),
        );
        assert_eq!(code, EXIT_SUCCESS);

        let (code, _) = invoke(
            &store,
            "action_link.remove",
            "req-link-remove",
            version_of(&store, "goals"),
            JsonValue::object([("action_id", JsonValue::from("action-001"))]),
        );
        assert_eq!(code, EXIT_SUCCESS);
        let stored = document(&store, "goals");
        assert!(
            stored
                .get("action_links")
                .and_then(JsonValue::as_array)
                .is_some_and(Vec::is_empty)
        );
    }

    #[test]
    fn goal_create_requires_parent_or_explicit_group() {
        let store = me_store::MemoryStore::new();
        init(&store);

        let (code, response) = create_goal(
            &store,
            "req-no-parent",
            JsonValue::object([
                ("id", JsonValue::from("goal-orphan")),
                ("title", JsonValue::from("No parent")),
                ("success_criteria", JsonValue::string_array(&["done"])),
            ]),
        );
        assert_eq!(code, EXIT_FAILURE);
        assert_eq!(error_code(&response), Some("INVALID_INPUT"));
        assert!(!has_record(
            &document(&store, "goals"),
            "goals",
            "id",
            "goal-orphan"
        ));

        let (code, _) = create_goal(
            &store,
            "req-root-group",
            JsonValue::object([
                ("id", JsonValue::from("goal-root")),
                ("group", JsonValue::from(true)),
                ("title", JsonValue::from("Root group")),
                ("success_criteria", JsonValue::string_array(&["done"])),
            ]),
        );
        assert_eq!(code, EXIT_SUCCESS);

        let (code, response) = create_goal(
            &store,
            "req-group-with-parent",
            JsonValue::object([
                ("group", JsonValue::from(true)),
                ("parent_goal_ids", JsonValue::string_array(&["goal-root"])),
                ("title", JsonValue::from("Nested group")),
                ("success_criteria", JsonValue::string_array(&["done"])),
            ]),
        );
        assert_eq!(code, EXIT_FAILURE);
        assert_eq!(error_code(&response), Some("INVALID_INPUT"));

        let (code, _) = create_goal(
            &store,
            "req-child-goal",
            JsonValue::object([
                ("id", JsonValue::from("goal-child")),
                ("parent_goal_ids", JsonValue::string_array(&["goal-root"])),
                ("title", JsonValue::from("Child goal")),
                ("success_criteria", JsonValue::string_array(&["done"])),
            ]),
        );
        assert_eq!(code, EXIT_SUCCESS);

        let (code, response) = create_goal(
            &store,
            "req-non-bool-group",
            JsonValue::object([
                ("group", JsonValue::from("yes")),
                ("title", JsonValue::from("Bad flag")),
                ("success_criteria", JsonValue::string_array(&["done"])),
            ]),
        );
        assert_eq!(code, EXIT_FAILURE);
        assert_eq!(error_code(&response), Some("INVALID_INPUT"));
    }

    #[test]
    fn milestone_and_checkpoint_create_require_existing_parent() {
        let store = me_store::MemoryStore::new();
        init(&store);

        let version = version_of(&store, "goals");
        let (code, response) = invoke(
            &store,
            "milestone.create",
            "req-no-goal",
            version,
            JsonValue::object([
                ("title", JsonValue::from("No goal")),
                ("exit_criteria", JsonValue::string_array(&["exit"])),
            ]),
        );
        assert_eq!(code, EXIT_FAILURE);
        assert_eq!(error_code(&response), Some("INVALID_INPUT"));

        let (code, response) = invoke(
            &store,
            "milestone.create",
            "req-missing-goal",
            version,
            JsonValue::object([
                ("goal_id", JsonValue::from("goal-missing")),
                ("title", JsonValue::from("Missing goal")),
                ("exit_criteria", JsonValue::string_array(&["exit"])),
            ]),
        );
        assert_eq!(code, EXIT_FAILURE);
        assert_eq!(error_code(&response), Some("NOT_FOUND"));

        let (code, _) = create_goal(
            &store,
            "req-parent-group",
            JsonValue::object([
                ("id", JsonValue::from("goal-parent")),
                ("group", JsonValue::from(true)),
                ("title", JsonValue::from("Parent group")),
                ("success_criteria", JsonValue::string_array(&["done"])),
            ]),
        );
        assert_eq!(code, EXIT_SUCCESS);
        let (code, _) = invoke(
            &store,
            "milestone.create",
            "req-parent-milestone",
            version_of(&store, "goals"),
            JsonValue::object([
                ("id", JsonValue::from("milestone-parent")),
                ("goal_id", JsonValue::from("goal-parent")),
                ("title", JsonValue::from("Parent milestone")),
                ("exit_criteria", JsonValue::string_array(&["exit"])),
            ]),
        );
        assert_eq!(code, EXIT_SUCCESS);

        let version = version_of(&store, "goals");
        let (code, response) = invoke(
            &store,
            "checkpoint.create",
            "req-no-milestone",
            version,
            JsonValue::object([("title", JsonValue::from("No milestone"))]),
        );
        assert_eq!(code, EXIT_FAILURE);
        assert_eq!(error_code(&response), Some("INVALID_INPUT"));

        let (code, response) = invoke(
            &store,
            "checkpoint.create",
            "req-missing-milestone",
            version,
            JsonValue::object([
                ("milestone_id", JsonValue::from("milestone-missing")),
                ("title", JsonValue::from("Missing milestone")),
            ]),
        );
        assert_eq!(code, EXIT_FAILURE);
        assert_eq!(error_code(&response), Some("NOT_FOUND"));
    }

    #[test]
    fn delete_commands_cascade_planning_children_and_keep_actions() {
        let store = me_store::MemoryStore::new();
        init(&store);
        seed_actions(&store);

        let (code, _) = create_goal(
            &store,
            "req-group",
            JsonValue::object([
                ("id", JsonValue::from("goal-group")),
                ("group", JsonValue::from(true)),
                ("title", JsonValue::from("Group")),
                ("success_criteria", JsonValue::string_array(&["group done"])),
                ("horizon", JsonValue::from("years")),
            ]),
        );
        assert_eq!(code, EXIT_SUCCESS);
        let (code, _) = create_goal(
            &store,
            "req-child",
            JsonValue::object([
                ("id", JsonValue::from("goal-child")),
                ("parent_goal_ids", JsonValue::string_array(&["goal-group"])),
                ("title", JsonValue::from("Child")),
                ("success_criteria", JsonValue::string_array(&["child done"])),
            ]),
        );
        assert_eq!(code, EXIT_SUCCESS);
        let (code, _) = invoke(
            &store,
            "milestone.create",
            "req-delete-milestone",
            version_of(&store, "goals"),
            JsonValue::object([
                ("id", JsonValue::from("milestone-delete")),
                ("goal_id", JsonValue::from("goal-child")),
                ("title", JsonValue::from("Delete milestone")),
                ("exit_criteria", JsonValue::string_array(&["exit"])),
                ("action_ids", JsonValue::string_array(&["action-001"])),
            ]),
        );
        assert_eq!(code, EXIT_SUCCESS);
        let (code, _) = invoke(
            &store,
            "checkpoint.create",
            "req-delete-checkpoint",
            version_of(&store, "goals"),
            JsonValue::object([
                ("id", JsonValue::from("checkpoint-delete")),
                ("milestone_id", JsonValue::from("milestone-delete")),
                ("title", JsonValue::from("Delete checkpoint")),
            ]),
        );
        assert_eq!(code, EXIT_SUCCESS);
        let (code, _) = invoke(
            &store,
            "action_link.set",
            "req-delete-link",
            version_of(&store, "goals"),
            JsonValue::object([
                ("action_id", JsonValue::from("action-001")),
                ("goal_id", JsonValue::from("goal-child")),
                ("milestone_id", JsonValue::from("milestone-delete")),
                ("checkpoint_id", JsonValue::from("checkpoint-delete")),
            ]),
        );
        assert_eq!(code, EXIT_SUCCESS);

        let (code, _) = invoke(
            &store,
            "checkpoint.delete",
            "req-delete-checkpoint-cmd",
            version_of(&store, "goals"),
            JsonValue::object([("checkpoint_id", JsonValue::from("checkpoint-delete"))]),
        );
        assert_eq!(code, EXIT_SUCCESS);
        let stored = document(&store, "goals");
        assert!(!has_record(
            &stored,
            "checkpoints",
            "id",
            "checkpoint-delete"
        ));
        assert!(
            stored
                .get("action_links")
                .and_then(JsonValue::as_array)
                .is_some_and(Vec::is_empty)
        );
        assert!(has_record(
            &document(&store, "actions"),
            "items",
            "id",
            "action-001"
        ));

        let (code, _) = invoke(
            &store,
            "milestone.delete",
            "req-delete-milestone-cmd",
            version_of(&store, "goals"),
            JsonValue::object([("milestone_id", JsonValue::from("milestone-delete"))]),
        );
        assert_eq!(code, EXIT_SUCCESS);
        let stored = document(&store, "goals");
        assert!(!has_record(&stored, "milestones", "id", "milestone-delete"));
        assert!(
            !find_in_collection(&stored, "goals", "id", "goal-child")
                .get("milestone_ids")
                .and_then(JsonValue::as_array)
                .is_some_and(|items| items
                    .iter()
                    .any(|item| item.as_str() == Some("milestone-delete")))
        );

        let (code, _) = invoke(
            &store,
            "goal.delete",
            "req-delete-child",
            version_of(&store, "goals"),
            JsonValue::object([("goal_id", JsonValue::from("goal-child"))]),
        );
        assert_eq!(code, EXIT_SUCCESS);
        let stored = document(&store, "goals");
        assert!(!has_record(&stored, "goals", "id", "goal-child"));
        assert!(has_record(&stored, "goals", "id", "goal-group"));

        let (code, _) = invoke(
            &store,
            "goal.delete",
            "req-delete-group",
            version_of(&store, "goals"),
            JsonValue::object([("goal_id", JsonValue::from("goal-group"))]),
        );
        assert_eq!(code, EXIT_SUCCESS);
        let stored = document(&store, "goals");
        assert!(!has_record(&stored, "goals", "id", "goal-group"));
        assert!(has_record(
            &document(&store, "actions"),
            "items",
            "id",
            "action-001"
        ));
    }

    #[test]
    fn milestone_storage_sorts_by_start_date_within_goal() {
        let store = me_store::MemoryStore::new();
        init(&store);
        let (code, _) = create_goal(
            &store,
            "req-sort-goal",
            JsonValue::object([
                ("id", JsonValue::from("goal-sort")),
                ("group", JsonValue::from(true)),
                ("title", JsonValue::from("Sort milestones")),
                ("success_criteria", JsonValue::string_array(&["done"])),
            ]),
        );
        assert_eq!(code, EXIT_SUCCESS);

        for (id, start, due) in [
            ("milestone-late", Some("2027-03-01"), None),
            ("milestone-early", Some("2026-12-01"), None),
            ("milestone-fallback", None, Some("2027-01-01")),
            ("milestone-none", None, None),
        ] {
            let mut input = JsonValue::object([
                ("id", JsonValue::from(id)),
                ("goal_id", JsonValue::from("goal-sort")),
                ("title", JsonValue::from(id)),
                ("exit_criteria", JsonValue::string_array(&["exit"])),
            ]);
            if let Some(object) = input.as_object_mut() {
                if let Some(start) = start {
                    object.insert("start_date".to_string(), JsonValue::from(start));
                }
                if let Some(due) = due {
                    object.insert("due_at".to_string(), JsonValue::from(due));
                }
            }
            let (code, response) = invoke(
                &store,
                "milestone.create",
                &format!("req-{id}"),
                version_of(&store, "goals"),
                input,
            );
            assert_eq!(code, EXIT_SUCCESS, "{response}");
        }

        let order = |stored: &JsonValue| -> Vec<String> {
            stored
                .get("milestones")
                .and_then(JsonValue::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter(|milestone| {
                            milestone.get("goal_id").and_then(JsonValue::as_str)
                                == Some("goal-sort")
                        })
                        .filter_map(|milestone| {
                            milestone
                                .get("id")
                                .and_then(JsonValue::as_str)
                                .map(str::to_string)
                        })
                        .collect()
                })
                .unwrap_or_default()
        };

        assert_eq!(
            order(&document(&store, "goals")),
            [
                "milestone-early",
                "milestone-fallback",
                "milestone-late",
                "milestone-none"
            ]
            .map(str::to_string)
        );

        let (code, response) = invoke(
            &store,
            "milestone.update",
            "req-sort-update",
            version_of(&store, "goals"),
            JsonValue::object([
                ("milestone_id", JsonValue::from("milestone-late")),
                (
                    "patch",
                    JsonValue::object([("start_date", JsonValue::from("2026-11-01"))]),
                ),
            ]),
        );
        assert_eq!(code, EXIT_SUCCESS, "{response}");
        assert_eq!(
            order(&document(&store, "goals")),
            [
                "milestone-late",
                "milestone-early",
                "milestone-fallback",
                "milestone-none"
            ]
            .map(str::to_string)
        );
    }

    #[test]
    fn goal_hierarchy_cycles_are_rejected_without_writing() {
        let store = me_store::MemoryStore::new();
        init(&store);
        for (id, parent) in [("goal-a", None), ("goal-b", Some("goal-a"))] {
            let mut input = JsonValue::object([
                ("id", JsonValue::from(id)),
                ("title", JsonValue::from(id)),
                (
                    "success_criteria",
                    JsonValue::string_array(&["Something verifiable"]),
                ),
            ]);
            if let Some(parent) = parent {
                input.as_object_mut().expect("input object").insert(
                    "parent_goal_ids".to_string(),
                    JsonValue::string_array(&[parent]),
                );
            } else {
                input
                    .as_object_mut()
                    .expect("input object")
                    .insert("group".to_string(), JsonValue::from(true));
            }
            let (code, _) = create_goal(&store, &format!("req-{id}"), input);
            assert_eq!(code, EXIT_SUCCESS);
        }

        let version = version_of(&store, "goals");
        let (code, response) = invoke(
            &store,
            "goal.update",
            "req-cycle",
            version,
            JsonValue::object([
                ("goal_id", JsonValue::from("goal-a")),
                (
                    "patch",
                    JsonValue::object([("parent_goal_ids", JsonValue::string_array(&["goal-b"]))]),
                ),
            ]),
        );
        assert_eq!(code, EXIT_FAILURE);
        assert_eq!(error_code(&response), Some("DATA_INVALID"));
        assert_eq!(version_of(&store, "goals"), version);
        assert!(
            find_in_collection(&document(&store, "goals"), "goals", "id", "goal-a")
                .get("parent_goal_ids")
                .and_then(JsonValue::as_array)
                .is_some_and(Vec::is_empty)
        );
    }

    #[test]
    fn migration_converts_directions_current_goals_and_task_links() {
        let store = me_store::MemoryStore::new();
        seed_actions(&store);
        let mut legacy = shared::Document::new();
        legacy.insert_str("schema_version", JsonValue::from(1_u64));
        legacy.insert_str("version", JsonValue::from(3_u64));
        legacy.insert_str("updated_at", JsonValue::from("2026-10-18T18:00:00+08:00"));
        legacy.insert_str("updated_by", JsonValue::from("legacy"));
        legacy.insert_str("timezone", JsonValue::from("Asia/Shanghai"));
        legacy.insert_str(
            "review",
            JsonValue::object([
                ("cadence", JsonValue::from("weekly")),
                ("next_review_at", JsonValue::Null),
            ]),
        );
        legacy.insert_str(
            "directions",
            JsonValue::Array(vec![JsonValue::object([
                ("id", JsonValue::from("direction-career")),
                ("title", JsonValue::from("Career")),
                ("description", JsonValue::from("Build leverage")),
                ("purpose", JsonValue::from("Durable capability")),
                ("status", JsonValue::from("active")),
                ("health", JsonValue::from("unknown")),
                ("priority", JsonValue::from("high")),
                ("horizon", JsonValue::from("years")),
                (
                    "success_definition",
                    JsonValue::string_array(&["Publish two artifacts"]),
                ),
                ("constraints", JsonValue::string_array(&["Protect health"])),
                ("review_at", JsonValue::from("2027-01-15")),
                ("created_at", JsonValue::from("2026-10-18T18:08:54+08:00")),
                ("updated_at", JsonValue::from("2026-10-18T18:08:54+08:00")),
            ])]),
        );
        legacy.insert_str(
            "goals",
            JsonValue::Array(vec![JsonValue::object([
                ("id", JsonValue::from("goal-portfolio")),
                (
                    "direction_ids",
                    JsonValue::string_array(&["direction-career"]),
                ),
                ("title", JsonValue::from("Portfolio")),
                ("description", JsonValue::from("Ship tools")),
                ("horizon", JsonValue::from("months")),
                ("status", JsonValue::from("active")),
                ("health", JsonValue::from("on_track")),
                ("priority", JsonValue::from("high")),
                ("start_date", JsonValue::from("2026-10-01")),
                ("deadline", JsonValue::from("2027-06-30")),
                (
                    "success_criteria",
                    JsonValue::string_array(&["Ship two tools"]),
                ),
                ("progress", JsonValue::from(0_u64)),
                ("progress_basis", JsonValue::from("manual")),
                (
                    "milestone_ids",
                    JsonValue::string_array(&["milestone-design"]),
                ),
                ("last_progress_at", JsonValue::Null),
                ("next_review_at", JsonValue::Null),
                ("next_action", JsonValue::from("")),
                ("blockers", JsonValue::Array(Vec::new())),
                ("risks", JsonValue::Array(Vec::new())),
                ("assumptions", JsonValue::Array(Vec::new())),
                ("updated_at", JsonValue::from("2026-10-18T18:08:54+08:00")),
            ])]),
        );
        legacy.insert_str(
            "milestones",
            JsonValue::Array(vec![JsonValue::object([
                ("id", JsonValue::from("milestone-design")),
                ("goal_id", JsonValue::from("goal-portfolio")),
                ("title", JsonValue::from("Design")),
                ("sequence", JsonValue::from(1_u64)),
                ("status", JsonValue::from("planned")),
                ("health", JsonValue::from("unknown")),
                ("start_date", JsonValue::from("2026-10-01")),
                ("due_at", JsonValue::from("2026-10-31")),
                (
                    "exit_criteria",
                    JsonValue::string_array(&["Prototype runs"]),
                ),
                ("progress", JsonValue::from(0_u64)),
                ("task_ids", JsonValue::string_array(&["action-001"])),
                ("dependencies", JsonValue::Array(Vec::new())),
                ("next_action", JsonValue::from("")),
                ("blockers", JsonValue::Array(Vec::new())),
                ("last_progress_at", JsonValue::Null),
                ("updated_at", JsonValue::from("2026-10-18T18:08:54+08:00")),
            ])]),
        );
        legacy.insert_str(
            "current_goals",
            JsonValue::Array(vec![JsonValue::object([
                ("id", JsonValue::from("current-wrapper")),
                ("milestone_id", JsonValue::from("milestone-design")),
                ("title", JsonValue::from("Write wrapper")),
                ("description", JsonValue::from("")),
                ("status", JsonValue::from("planned")),
                ("estimate_minutes", JsonValue::from(60_u64)),
                ("created_at", JsonValue::from("2026-10-18T18:08:54+08:00")),
                ("updated_at", JsonValue::from("2026-10-18T18:08:54+08:00")),
            ])]),
        );
        legacy.insert_str(
            "task_links",
            JsonValue::Array(vec![JsonValue::object([
                ("task_id", JsonValue::from("action-001")),
                (
                    "direction_ids",
                    JsonValue::string_array(&["direction-career"]),
                ),
                ("goal_id", JsonValue::from("goal-portfolio")),
                ("milestone_id", JsonValue::from("milestone-design")),
                ("current_goal_id", JsonValue::from("current-wrapper")),
                ("role", JsonValue::from("core")),
                ("contribution", JsonValue::from("")),
            ])]),
        );
        legacy.insert_str("recent_requests", JsonValue::Array(Vec::new()));
        store.insert_initial("goals", &legacy).unwrap();

        let (code, response) = invoke(
            &store,
            "migrate",
            "req-migrate",
            3,
            JsonValue::object([("target_schema_version", JsonValue::from(2_u64))]),
        );
        assert_eq!(code, EXIT_SUCCESS, "{response:?}");
        let migrated = document(&store, "goals");
        assert_eq!(
            migrated.get("schema_version").and_then(JsonValue::as_u64),
            Some(2)
        );
        assert_eq!(version_of(&store, "goals"), 4);
        assert!(migrated.get("directions").is_none());
        assert!(migrated.get("current_goals").is_none());
        assert!(migrated.get("task_links").is_none());
        let direction = find_in_collection(&migrated, "goals", "id", "direction-career");
        assert_eq!(
            direction.get("purpose").and_then(JsonValue::as_str),
            Some("Durable capability")
        );
        assert_eq!(
            direction.get("created_at").and_then(JsonValue::as_str),
            Some("2026-10-18T18:08:54+08:00")
        );
        assert_eq!(
            direction
                .get("success_criteria")
                .and_then(JsonValue::as_array)
                .map(Vec::len),
            Some(1)
        );
        assert_eq!(
            find_in_collection(&migrated, "goals", "id", "goal-portfolio")
                .get("parent_goal_ids")
                .and_then(JsonValue::as_array)
                .and_then(|items| items.first())
                .and_then(JsonValue::as_str),
            Some("direction-career")
        );
        assert_eq!(
            find_in_collection(&migrated, "checkpoints", "id", "current-wrapper")
                .get("milestone_id")
                .and_then(JsonValue::as_str),
            Some("milestone-design")
        );
        let link = find_in_collection(&migrated, "action_links", "action_id", "action-001");
        assert_eq!(
            link.get("goal_ids")
                .and_then(JsonValue::as_array)
                .and_then(|items| items.first())
                .and_then(JsonValue::as_str),
            Some("direction-career")
        );
        assert_eq!(
            link.get("checkpoint_id").and_then(JsonValue::as_str),
            Some("current-wrapper")
        );

        // Idempotent second run.
        let (code, _) = invoke(
            &store,
            "migrate",
            "req-migrate-again",
            4,
            JsonValue::object([("target_schema_version", JsonValue::from(2_u64))]),
        );
        assert_eq!(code, EXIT_SUCCESS);
        assert_eq!(version_of(&store, "goals"), 4);
    }

    #[test]
    fn migration_rejects_stale_version_and_empty_success_definition() {
        let store = me_store::MemoryStore::new();
        let mut legacy = shared::Document::new();
        legacy.insert_str("schema_version", JsonValue::from(1_u64));
        legacy.insert_str("version", JsonValue::from(2_u64));
        legacy.insert_str("updated_at", JsonValue::from("2026-10-18T18:00:00+08:00"));
        legacy.insert_str("updated_by", JsonValue::from("legacy"));
        legacy.insert_str("timezone", JsonValue::from("Asia/Shanghai"));
        legacy.insert_str(
            "review",
            JsonValue::object([
                ("cadence", JsonValue::from("weekly")),
                ("next_review_at", JsonValue::Null),
            ]),
        );
        legacy.insert_str(
            "directions",
            JsonValue::Array(vec![JsonValue::object([
                ("id", JsonValue::from("direction-empty")),
                ("title", JsonValue::from("Empty")),
                ("description", JsonValue::from("")),
                ("purpose", JsonValue::from("")),
                ("status", JsonValue::from("active")),
                ("health", JsonValue::from("unknown")),
                ("priority", JsonValue::from("normal")),
                ("horizon", JsonValue::from("years")),
                ("success_definition", JsonValue::Array(Vec::new())),
                ("constraints", JsonValue::Array(Vec::new())),
                ("review_at", JsonValue::Null),
                ("created_at", JsonValue::from("2026-10-18T18:08:54+08:00")),
                ("updated_at", JsonValue::from("2026-10-18T18:08:54+08:00")),
            ])]),
        );
        legacy.insert_str("goals", JsonValue::Array(Vec::new()));
        legacy.insert_str("milestones", JsonValue::Array(Vec::new()));
        legacy.insert_str("current_goals", JsonValue::Array(Vec::new()));
        legacy.insert_str("task_links", JsonValue::Array(Vec::new()));
        legacy.insert_str("recent_requests", JsonValue::Array(Vec::new()));
        store.insert_initial("goals", &legacy).unwrap();

        let (code, response) = invoke(
            &store,
            "migrate",
            "req-stale",
            1,
            JsonValue::object([("target_schema_version", JsonValue::from(2_u64))]),
        );
        assert_eq!(code, EXIT_VERSION_CONFLICT);
        assert_eq!(error_code(&response), Some("VERSION_CONFLICT"));

        let (code, response) = invoke(
            &store,
            "migrate",
            "req-empty",
            2,
            JsonValue::object([("target_schema_version", JsonValue::from(2_u64))]),
        );
        assert_eq!(code, EXIT_FAILURE);
        assert_eq!(error_code(&response), Some("DATA_INVALID"));
        assert_eq!(
            version_of(&store, "goals"),
            2,
            "document after failed migration: {}",
            document(&store, "goals").to_json_string()
        );
    }
}
