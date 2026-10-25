//! Shared protocol, JSON, document-store abstraction, and validation helpers.

pub mod document;
pub mod json;
pub mod storage;

pub use document::{Document, DocumentStore, DocumentTransaction, MutateOutcome, apply_mutation};

use std::collections::BTreeMap;

pub use json::{JsonValue, parse_json};
pub use storage::{
    RECEIPT_LIMIT, check_fields, civil_from_days, days_from_civil, generate_id, now_iso8601,
    parse_instant_epoch_micros, parse_shanghai_date_start_micros, remember_request, replay_request,
    request_digest, sha256_hex, validate_hh_mm, validate_instant, validate_iso_date,
};

pub const EXIT_SUCCESS: u8 = 0;
pub const EXIT_FAILURE: u8 = 2;
pub const EXIT_VERSION_CONFLICT: u8 = 4;

// ---- FunctionFailure --------------------------------------------------------

/// Expected protocol or domain failure that serializes into a standard JSON error response.
#[derive(Debug, Clone)]
pub struct FunctionFailure {
    pub code: String,
    pub message: String,
    pub request_id: Option<String>,
    pub exit_code: u8,
    pub details: Option<JsonValue>,
}

impl FunctionFailure {
    // ---- constructors -------------------------------------------------------

    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            request_id: None,
            exit_code: EXIT_FAILURE,
            details: None,
        }
    }

    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::new("INVALID_INPUT", message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new("NOT_FOUND", message)
    }

    pub fn not_implemented(command: &str) -> Self {
        Self::new(
            "NOT_IMPLEMENTED",
            format!("command '{command}' is not implemented yet"),
        )
    }

    pub fn version_conflict(message: impl Into<String>) -> Self {
        Self {
            code: "VERSION_CONFLICT".to_string(),
            message: message.into(),
            request_id: None,
            exit_code: EXIT_VERSION_CONFLICT,
            details: None,
        }
    }

    pub fn with_request_id(mut self, request_id: impl Into<String>) -> Self {
        self.request_id = Some(request_id.into());
        self
    }

    pub fn with_details(mut self, details: JsonValue) -> Self {
        self.details = Some(details);
        self
    }
}

impl std::fmt::Display for FunctionFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for FunctionFailure {}

// ---- Request ----------------------------------------------------------------

/// Validated request envelope parsed for a domain write command.
#[derive(Debug, Clone)]
pub struct Request {
    pub request_id: String,
    pub expected_version: u64,
    pub input: BTreeMap<String, JsonValue>,
}

// ---- FieldSpec --------------------------------------------------------------

/// Field specification rendered by `describe`.
#[derive(Debug, Clone)]
pub struct FieldSpec {
    pub name: &'static str,
    pub field_type: &'static str,
    pub required: bool,
    pub description: &'static str,
}

impl FieldSpec {
    pub fn to_json(&self) -> JsonValue {
        JsonValue::object([
            ("name", JsonValue::from(self.name)),
            ("field_type", JsonValue::from(self.field_type)),
            ("required", JsonValue::from(self.required)),
            ("description", JsonValue::from(self.description)),
        ])
    }
}

// ---- CommandSpec ------------------------------------------------------------

/// Metadata, field specifications, rules, and handler entry point for one subcommand.
pub struct CommandSpec {
    pub name: &'static str,
    pub summary: &'static str,
    pub reads: &'static [&'static str],
    pub writes: &'static [&'static str],
    pub expected_version_rule: &'static str,
    pub fields: &'static [FieldSpec],
    pub rules: &'static [&'static str],
    pub example_version: u64,
    pub example_input: fn() -> JsonValue,
    pub handler: fn(&dyn DocumentStore, &Request) -> Result<JsonValue, FunctionFailure>,
}

// ---- ProgramSpec ------------------------------------------------------------

/// Full protocol metadata and command table served by `me-server`.
pub struct ProgramSpec {
    pub name: &'static str,
    pub summary: &'static str,
    pub collections: &'static [&'static str],
    pub usage: &'static [&'static str],
    pub rules: &'static [&'static str],
    pub error_codes: &'static [(&'static str, &'static str)],
    pub init_handler: fn(&dyn DocumentStore) -> Result<JsonValue, FunctionFailure>,
    pub commands: &'static [CommandSpec],
}

// ---- validate_request_value -------------------------------------------------

/// Validate and convert a parsed JSON value into a `Request`.
///
/// The top-level JSON value must be an object with exact keys `request_id`, `expected_version`, and `input`.
pub fn validate_request_value(payload: JsonValue) -> Result<Request, FunctionFailure> {
    let object = payload
        .as_object()
        .ok_or_else(|| FunctionFailure::invalid_input("request must be a JSON object"))?;

    if object.len() != 3
        || !object.contains_key("request_id")
        || !object.contains_key("expected_version")
        || !object.contains_key("input")
    {
        return Err(FunctionFailure::invalid_input(
            "request requires request_id, expected_version, input",
        ));
    }

    let request_id = object
        .get("request_id")
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| FunctionFailure::invalid_input("request_id must be a non-empty string"))?
        .to_string();

    let expected_version = object
        .get("expected_version")
        .and_then(JsonValue::as_u64)
        .ok_or_else(|| {
            FunctionFailure::invalid_input(
                "expected_version must be a non-negative integer (0 only for initialization)",
            )
            .with_request_id(request_id.clone())
        })?;

    let input_obj = object
        .get("input")
        .and_then(JsonValue::as_object)
        .ok_or_else(|| {
            FunctionFailure::invalid_input("input must be a JSON object")
                .with_request_id(request_id.clone())
        })?;

    Ok(Request {
        request_id,
        expected_version,
        input: input_obj.clone(),
    })
}

// ---- build_help_response ----------------------------------------------------

/// Build the short usage JSON for `<program> --help` without reading stdin or accessing data directories.
pub fn build_help_response(program: &ProgramSpec) -> JsonValue {
    let command_items: Vec<JsonValue> = program
        .commands
        .iter()
        .map(|cmd| {
            JsonValue::object([
                ("name", JsonValue::from(cmd.name)),
                ("summary", JsonValue::from(cmd.summary)),
            ])
        })
        .collect();

    let builtin_commands = JsonValue::array([
        JsonValue::object([
            ("name", JsonValue::from("--help")),
            (
                "summary",
                JsonValue::from(
                    "Return short usage and command index (does not read stdin or access the database)",
                ),
            ),
        ]),
        JsonValue::object([
            ("name", JsonValue::from("describe")),
            (
                "summary",
                JsonValue::from(
                    "Return full command list and protocol rules (does not read stdin or access the database)",
                ),
            ),
        ]),
        JsonValue::object([
            ("name", JsonValue::from("describe <command>")),
            (
                "summary",
                JsonValue::from(
                    "Return fields, rules, and request examples for a command (does not read stdin or access the database)",
                ),
            ),
        ]),
        JsonValue::object([
            ("name", JsonValue::from("init")),
            (
                "summary",
                JsonValue::from("Initialize the database with baseline collections"),
            ),
        ]),
    ]);

    let input_modes = JsonValue::array([
        JsonValue::object([
            ("mode", JsonValue::from("json")),
            (
                "description",
                JsonValue::from("Pass full request JSON via stdin or via --json '<request_json>'"),
            ),
        ]),
        JsonValue::object([
            ("mode", JsonValue::from("fields")),
            (
                "description",
                JsonValue::from(
                    "Pass --request-id <id> --expected-version <n> with --set <field>=<value> or --<field> <value> (supports dotted paths such as patch.status=done)",
                ),
            ),
        ]),
    ]);

    JsonValue::object([
        ("status", JsonValue::from("succeeded")),
        ("program", JsonValue::from(program.name)),
        ("summary", JsonValue::from(program.summary)),
        ("usage", JsonValue::string_array(program.usage)),
        ("input_modes", input_modes),
        ("builtin_commands", builtin_commands),
        ("commands", JsonValue::Array(command_items)),
    ])
}

// ---- build_describe_overview ------------------------------------------------

/// Build the full program description JSON for `<program> describe`.
pub fn build_describe_overview(program: &ProgramSpec) -> JsonValue {
    let commands: Vec<JsonValue> = program
        .commands
        .iter()
        .map(|cmd| {
            JsonValue::object([
                ("name", JsonValue::from(cmd.name)),
                ("summary", JsonValue::from(cmd.summary)),
                ("reads", JsonValue::string_array(cmd.reads)),
                ("writes", JsonValue::string_array(cmd.writes)),
                (
                    "expected_version",
                    JsonValue::from(cmd.expected_version_rule),
                ),
            ])
        })
        .collect();

    let errors: Vec<JsonValue> = program
        .error_codes
        .iter()
        .map(|(code, desc)| {
            JsonValue::object([
                ("code", JsonValue::from(*code)),
                ("description", JsonValue::from(*desc)),
            ])
        })
        .collect();

    let request_envelope = JsonValue::object([
        (
            "request_id",
            JsonValue::from(
                "Non-empty string identifying a single write intent; keep unchanged when retrying the same request",
            ),
        ),
        (
            "expected_version",
            JsonValue::from(
                "Non-negative integer specifying the expected version of the target collection or record (0 only for initialization)",
            ),
        ),
        (
            "input",
            JsonValue::from("JSON object containing command-specific input fields"),
        ),
    ]);

    let input_modes = JsonValue::object([
        (
            "json",
            JsonValue::from(
                "POST the envelope {request_id, expected_version, input} as the JSON body; me-client write also accepts --input '<json>' or --input-file <path>",
            ),
        ),
        (
            "fields",
            JsonValue::from(
                "me-client write with --request-id <id> --expected-version <n> and repeatable --set <key>=<value> (dotted paths address nested objects, e.g. patch.bio=profile)",
            ),
        ),
    ]);

    JsonValue::object([
        ("status", JsonValue::from("succeeded")),
        ("program", JsonValue::from(program.name)),
        ("summary", JsonValue::from(program.summary)),
        ("collections", JsonValue::string_array(program.collections)),
        ("usage", JsonValue::string_array(program.usage)),
        ("input_modes", input_modes),
        ("request_envelope", request_envelope),
        ("commands", JsonValue::Array(commands)),
        ("rules", JsonValue::string_array(program.rules)),
        ("error_codes", JsonValue::Array(errors)),
    ])
}

// ---- build_describe_command -------------------------------------------------

/// Build the single-command contract JSON for `<program> describe <command>`.
pub fn build_describe_command(
    program: &ProgramSpec,
    command_name: &str,
) -> Result<JsonValue, FunctionFailure> {
    if command_name == "init" {
        return Ok(JsonValue::object([
            ("status", JsonValue::from("succeeded")),
            ("program", JsonValue::from(program.name)),
            ("command", JsonValue::from("init")),
            (
                "summary",
                JsonValue::from("Create missing collections with baseline documents"),
            ),
            ("reads_stdin", JsonValue::from(false)),
            ("collections", JsonValue::string_array(program.collections)),
            ("usage", JsonValue::string_array(&["init"])),
            ("rules", JsonValue::string_array(program.rules)),
        ]));
    }

    let command = program
        .commands
        .iter()
        .find(|cmd| cmd.name == command_name)
        .ok_or_else(|| {
            FunctionFailure::invalid_input(format!(
                "unknown command '{command_name}'; run '{} describe' to list available commands",
                program.name
            ))
        })?;

    let fields: Vec<JsonValue> = command.fields.iter().map(FieldSpec::to_json).collect();

    let example_input = (command.example_input)();
    let example_request = JsonValue::object([
        (
            "request_id",
            JsonValue::from(format!("{}-example", command.name)),
        ),
        ("expected_version", JsonValue::from(command.example_version)),
        ("input", example_input.clone()),
    ]);

    let example_field_args = build_example_field_args(
        program.name,
        command.name,
        command.example_version,
        &example_input,
    );

    Ok(JsonValue::object([
        ("status", JsonValue::from("succeeded")),
        ("program", JsonValue::from(program.name)),
        ("command", JsonValue::from(command.name)),
        ("summary", JsonValue::from(command.summary)),
        ("reads", JsonValue::string_array(command.reads)),
        ("writes", JsonValue::string_array(command.writes)),
        (
            "expected_version",
            JsonValue::from(command.expected_version_rule),
        ),
        ("fields", JsonValue::Array(fields)),
        ("rules", JsonValue::string_array(command.rules)),
        ("example_json_request", example_request),
        (
            "example_field_invocation",
            JsonValue::from(example_field_args),
        ),
    ]))
}

// ---- build_example_field_args -----------------------------------------------

fn build_example_field_args(
    program_name: &str,
    command_name: &str,
    version: u64,
    example_input: &JsonValue,
) -> String {
    let mut parts = vec![
        format!("me-client write {program_name}.{command_name}"),
        format!("--request-id {command_name}-example"),
        format!("--expected-version {version}"),
    ];
    if let Some(obj) = example_input.as_object() {
        append_flattened_fields("", obj, &mut parts);
    }
    parts.join(" ")
}

fn append_flattened_fields(prefix: &str, map: &BTreeMap<String, JsonValue>, out: &mut Vec<String>) {
    for (key, value) in map {
        let full_key = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        if let Some(nested) = value.as_object() {
            append_flattened_fields(&full_key, nested, out);
        } else {
            let formatted = match value {
                JsonValue::String(s) => s.clone(),
                other => other.to_json_string(),
            };
            out.push(format!("--set {full_key}={formatted}"));
        }
    }
}

// ---- build_error_response ---------------------------------------------------

/// Convert `FunctionFailure` into a standard JSON error response.
pub fn build_error_response(failure: &FunctionFailure) -> JsonValue {
    let mut error_obj = BTreeMap::new();
    error_obj.insert("code".to_string(), JsonValue::from(failure.code.clone()));
    error_obj.insert(
        "message".to_string(),
        JsonValue::from(failure.message.clone()),
    );
    if let Some(details) = &failure.details {
        error_obj.insert("details".to_string(), details.clone());
    }

    JsonValue::object([
        ("status", JsonValue::from("failed")),
        (
            "request_id",
            JsonValue::from(failure.request_id.as_deref().unwrap_or("unknown")),
        ),
        ("changed_records", JsonValue::Array(Vec::new())),
        ("new_versions", JsonValue::Object(BTreeMap::new())),
        ("summary", JsonValue::from("function request failed")),
        ("error", JsonValue::Object(error_obj)),
    ])
}

// ---- tests ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_FIELDS: &[FieldSpec] = &[FieldSpec {
        name: "title",
        field_type: "string",
        required: true,
        description: "Non-empty item title",
    }];

    fn sample_input() -> JsonValue {
        JsonValue::object([("title", JsonValue::from("example-item"))])
    }

    fn sample_handler(
        _store: &dyn DocumentStore,
        request: &Request,
    ) -> Result<JsonValue, FunctionFailure> {
        Ok(JsonValue::object([
            ("status", JsonValue::from("succeeded")),
            ("request_id", JsonValue::from(request.request_id.clone())),
        ]))
    }

    fn sample_init(_store: &dyn DocumentStore) -> Result<JsonValue, FunctionFailure> {
        Ok(JsonValue::object([(
            "status",
            JsonValue::from("succeeded"),
        )]))
    }

    const SAMPLE_COMMANDS: &[CommandSpec] = &[CommandSpec {
        name: "item.create",
        summary: "Create a sample item",
        reads: &["sample"],
        writes: &["sample"],
        expected_version_rule: "Use the collection version",
        fields: SAMPLE_FIELDS,
        rules: &["title is required and must be non-empty"],
        example_version: 1,
        example_input: sample_input,
        handler: sample_handler,
    }];

    const SAMPLE_PROGRAM: ProgramSpec = ProgramSpec {
        name: "sample",
        summary: "Sample test program",
        collections: &["sample"],
        usage: &[
            "sample --help",
            "sample describe",
            "sample describe <command>",
            "sample init",
        ],
        rules: &["Responses contain a single JSON value"],
        error_codes: &[("INVALID_INPUT", "Invalid input parameters")],
        init_handler: sample_init,
        commands: SAMPLE_COMMANDS,
    };

    #[test]
    fn help_and_describe_are_ascii_and_list_collections() {
        for response in [
            build_help_response(&SAMPLE_PROGRAM),
            build_describe_overview(&SAMPLE_PROGRAM),
            build_describe_command(&SAMPLE_PROGRAM, "init").expect("init describe"),
            build_describe_command(&SAMPLE_PROGRAM, "item.create").expect("command describe"),
        ] {
            let text = response.to_json_string();
            assert!(text.is_ascii(), "response must be ASCII-compatible");
            let parsed = parse_json(&text).expect("valid JSON");
            assert_eq!(
                parsed.get("program").and_then(JsonValue::as_str),
                Some("sample")
            );
        }
    }

    #[test]
    fn describe_lists_collections_instead_of_file_paths() {
        let overview = build_describe_overview(&SAMPLE_PROGRAM);
        assert_eq!(
            overview
                .get("collections")
                .and_then(JsonValue::as_array)
                .map(|items| items.len()),
            Some(1)
        );
        assert!(overview.get("data_dir").is_none());

        let command = build_describe_command(&SAMPLE_PROGRAM, "item.create").expect("describe");
        assert_eq!(
            command
                .get("reads")
                .and_then(JsonValue::as_array)
                .map(|items| items.len()),
            Some(1)
        );
        assert!(
            command
                .get("example_field_invocation")
                .and_then(JsonValue::as_str)
                .unwrap_or("")
                .starts_with("me-client write sample.item.create")
        );
    }

    #[test]
    fn describe_unknown_command_is_rejected() {
        let error = build_describe_command(&SAMPLE_PROGRAM, "missing").expect_err("unknown");
        assert_eq!(error.code, "INVALID_INPUT");
    }

    #[test]
    fn validate_request_value_requires_exact_envelope_keys() {
        let valid = JsonValue::object([
            ("request_id", JsonValue::from("req-1")),
            ("expected_version", JsonValue::from(3_u64)),
            (
                "input",
                JsonValue::object([("title", JsonValue::from("x"))]),
            ),
        ]);
        let request = validate_request_value(valid).expect("valid");
        assert_eq!(request.request_id, "req-1");
        assert_eq!(request.expected_version, 3);
        assert_eq!(request.input.len(), 1);

        let extra = JsonValue::object([
            ("request_id", JsonValue::from("req-1")),
            ("expected_version", JsonValue::from(3_u64)),
            ("input", JsonValue::Object(BTreeMap::new())),
            ("work_dir", JsonValue::from("/tmp")),
        ]);
        let error = validate_request_value(extra).expect_err("extra key");
        assert_eq!(error.code, "INVALID_INPUT");
    }
}
