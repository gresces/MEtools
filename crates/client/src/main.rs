//! `me-client` command-line entry point.

use clap::{Args, Parser, Subcommand};
use me_client::api::{ApiClient, ApiError};
use me_client::config;
use me_client::util::{
    LocalError, LocalResult, input_str, parse_set_pairs, record_version, request_id, top_version,
};
use serde_json::{Map, Value, json};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Debug, Parser)]
#[command(
    name = "me-client",
    version,
    about = "HTTP client for the ME personal data server",
    long_about = "Read and write ME data through me-server. Output is a single JSON value on stdout; diagnostics go to stderr."
)]
struct Cli {
    /// me-server base URL (env: ME_SERVER_URL).
    #[arg(long, global = true, env = "ME_SERVER_URL")]
    server: Option<String>,

    /// Bearer token (env: ME_SERVER_TOKEN). Prefer --token-file.
    #[arg(long, global = true, env = "ME_SERVER_TOKEN", hide_env_values = true)]
    token: Option<String>,

    /// File containing the bearer token (env: ME_SERVER_TOKEN_FILE).
    #[arg(long, global = true, env = "ME_SERVER_TOKEN_FILE")]
    token_file: Option<PathBuf>,

    /// Request timeout in seconds.
    #[arg(long, global = true)]
    timeout: Option<u64>,

    /// Pretty-print JSON output.
    #[arg(long, global = true)]
    pretty: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Check server connectivity without authentication.
    Ping,
    /// List write programs and their commands.
    Programs,
    /// Show server status and data paths.
    Status,
    /// Show the full contract for a program or one command.
    Describe {
        program: String,
        command: Option<String>,
    },
    /// Read a raw data resource: profile, actions, health, life, goals, projects.
    Get(GetArgs),
    /// Read the dashboard summary view model.
    Summary,
    /// List actions with optional filters.
    Actions(ActionListArgs),
    /// Action shortcuts that manage versions automatically.
    Action {
        #[command(subcommand)]
        command: ActionCommands,
    },
    /// Read the goals document.
    Goals,
    /// Goal shortcuts that manage versions automatically.
    Goal {
        #[command(subcommand)]
        command: GoalCommands,
    },
    /// Read the life document.
    Life,
    /// Read the profile document.
    Profile,
    /// Write through any program command.
    Write(WriteArgs),
}

#[derive(Debug, Args, Clone)]
struct GetArgs {
    resource: String,

    #[arg(long)]
    status: Option<String>,

    #[arg(long)]
    tag: Option<String>,

    #[arg(long)]
    priority: Option<String>,

    /// Include archived actions.
    #[arg(long)]
    all: bool,

    #[arg(long)]
    limit: Option<usize>,

    /// Additional query parameter, repeatable (KEY=VALUE).
    #[arg(long = "query", value_name = "KEY=VALUE")]
    query: Vec<String>,
}

#[derive(Debug, Args, Clone)]
struct ActionListArgs {
    #[arg(long)]
    status: Option<String>,

    #[arg(long)]
    tag: Option<String>,

    #[arg(long)]
    priority: Option<String>,

    /// Include archived actions.
    #[arg(long)]
    all: bool,

    #[arg(long)]
    limit: Option<usize>,
}

#[derive(Debug, Args, Clone)]
struct WriteArgs {
    /// Program and command, for example `personal.action.create`.
    command: String,

    #[arg(long)]
    request_id: Option<String>,

    #[arg(long)]
    expected_version: Option<u64>,

    /// Fetch the current version automatically (single attempt, no retry).
    #[arg(long)]
    auto_version: bool,

    /// Set a dotted input field, repeatable (KEY=VALUE; JSON literals supported).
    #[arg(long = "set", value_name = "KEY=VALUE")]
    set: Vec<String>,

    /// Complete input object as JSON.
    #[arg(long = "input", value_name = "JSON", conflicts_with = "input_file")]
    input: Option<String>,

    /// Read the complete input object from a file.
    #[arg(long = "input-file", value_name = "PATH")]
    input_file: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
enum ActionCommands {
    /// List actions (same as `actions`).
    List(ActionListArgs),
    /// Create an action under an existing checkpoint; the collection version is fetched automatically.
    Add(ActionAddArgs),
    /// Update action fields; the record version is fetched automatically.
    Update(ActionUpdateArgs),
    /// Complete one occurrence of a recurring action.
    Complete { action_id: String },
    /// Mark an action done.
    Done { action_id: String },
    /// Reopen an action.
    Reopen { action_id: String },
    /// Archive an action.
    Archive { action_id: String },
    /// Restore an archived action.
    Restore { action_id: String },
    /// Move an action to a position in the execution queue.
    Queue { action_id: String, position: u32 },
    /// Delete an action and detach it from goals.
    Delete { action_id: String },
}

#[derive(Debug, Args, Clone)]
struct ActionAddArgs {
    #[arg(long)]
    title: String,

    #[arg(long)]
    checkpoint_id: String,

    #[arg(long)]
    notes: Option<String>,

    #[arg(long)]
    priority: Option<String>,

    #[arg(long)]
    due_date: Option<String>,

    #[arg(long)]
    due_at: Option<String>,

    /// Planned duration in minutes; defaults to 30.
    #[arg(long)]
    duration_minutes: Option<u64>,

    /// Elastic buffer in minutes; defaults to the planned duration.
    #[arg(long)]
    buffer_minutes: Option<u64>,

    /// Repeat rule as JSON, for example {"frequency":"weekly","count":8}.
    #[arg(long)]
    recurrence: Option<String>,

    #[arg(long = "tag")]
    tag: Vec<String>,
}

#[derive(Debug, Args, Clone)]
struct ActionUpdateArgs {
    action_id: String,

    #[arg(long)]
    title: Option<String>,

    #[arg(long)]
    notes: Option<String>,

    #[arg(long)]
    priority: Option<String>,

    #[arg(long)]
    status: Option<String>,

    #[arg(long)]
    due_date: Option<String>,

    #[arg(long)]
    due_at: Option<String>,

    #[arg(long)]
    duration_minutes: Option<u64>,

    #[arg(long)]
    buffer_minutes: Option<u64>,

    /// Replace the repeat rule with JSON; pass null to clear it.
    #[arg(long)]
    recurrence: Option<String>,

    /// Replace tags.
    #[arg(long = "tag")]
    tag: Vec<String>,

    /// Clear both due fields.
    #[arg(long)]
    clear_due: bool,
}

#[derive(Debug, Subcommand)]
enum GoalCommands {
    /// Record a progress event for a goal; the goals version is fetched automatically.
    Progress(GoalProgressArgs),
}

#[derive(Debug, Args, Clone)]
struct GoalProgressArgs {
    goal_id: String,

    #[arg(long)]
    percent: u32,

    #[arg(long)]
    summary: String,

    #[arg(long)]
    evidence: Option<String>,

    #[arg(long, default_value = "manual")]
    kind: String,
}

#[derive(Debug)]
enum CommandError {
    Api(ApiError),
    Local(LocalError),
}

impl CommandError {
    fn exit_code(&self) -> u8 {
        match self {
            Self::Api(error) => error.exit_code(),
            Self::Local(_) => 2,
        }
    }

    fn body(&self) -> Value {
        match self {
            Self::Api(error) => error.body(),
            Self::Local(error) => json!({
                "status": "failed",
                "error": {"code": error.code, "message": error.message},
            }),
        }
    }

    fn message(&self) -> String {
        match self {
            Self::Api(error) => error.message(),
            Self::Local(error) => error.message.clone(),
        }
    }
}

impl From<ApiError> for CommandError {
    fn from(error: ApiError) -> Self {
        Self::Api(error)
    }
}

impl From<LocalError> for CommandError {
    fn from(error: LocalError) -> Self {
        Self::Local(error)
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let pretty = cli.pretty;

    match run(cli) {
        Ok(value) => {
            print_json(&value, pretty);
            ExitCode::SUCCESS
        }
        Err(error) => {
            print_json(&error.body(), pretty);
            eprintln!("me-client: {}", error.message());
            ExitCode::from(error.exit_code())
        }
    }
}

fn run(cli: Cli) -> Result<Value, CommandError> {
    let config = config::resolve(cli.server, cli.token, cli.token_file, cli.timeout)
        .map_err(|message| LocalError::new("CONFIG_ERROR", message))?;
    let client =
        ApiClient::new(&config).map_err(|message| LocalError::new("CONFIG_ERROR", message))?;

    match cli.command {
        Commands::Ping => Ok(client.get("/healthz", &[])?),
        Commands::Programs => Ok(client.get("/api/v1/programs", &[])?),
        Commands::Status => Ok(client.get("/api/v1/status", &[])?),
        Commands::Describe { program, command } => {
            ensure_segment(&program, "program")?;
            let path = match command {
                Some(command) => {
                    ensure_segment(&command, "command")?;
                    format!("/api/v1/programs/{program}/commands/{command}")
                }
                None => format!("/api/v1/programs/{program}"),
            };
            Ok(client.get(&path, &[])?)
        }
        Commands::Get(args) => get_resource(&client, args),
        Commands::Summary => Ok(client.get("/api/v1/dashboard/summary", &[])?),
        Commands::Actions(args) => list_actions(&client, args),
        Commands::Action { command } => run_action(&client, command),
        Commands::Goals => Ok(client.get("/api/v1/data/goals", &[])?),
        Commands::Goal { command } => run_goal(&client, command),
        Commands::Life => Ok(client.get("/api/v1/data/life", &[])?),
        Commands::Profile => Ok(client.get("/api/v1/data/profile", &[])?),
        Commands::Write(args) => run_write(&client, args),
    }
}

fn get_resource(client: &ApiClient, args: GetArgs) -> Result<Value, CommandError> {
    ensure_segment(&args.resource, "resource")?;
    let mut query = Vec::new();
    push_opt(&mut query, "status", args.status);
    push_opt(&mut query, "tag", args.tag);
    push_opt(&mut query, "priority", args.priority);
    push_opt(
        &mut query,
        "limit",
        args.limit.map(|value| value.to_string()),
    );
    if args.all {
        query.push(("include_archived".to_string(), "true".to_string()));
    }
    query.extend(parse_query_pairs(&args.query)?);
    Ok(client.get(&format!("/api/v1/data/{}", args.resource), &query)?)
}

fn list_actions(client: &ApiClient, args: ActionListArgs) -> Result<Value, CommandError> {
    let mut query = Vec::new();
    push_opt(&mut query, "status", args.status);
    push_opt(&mut query, "tag", args.tag);
    push_opt(&mut query, "priority", args.priority);
    push_opt(
        &mut query,
        "limit",
        args.limit.map(|value| value.to_string()),
    );
    if args.all {
        query.push(("include_archived".to_string(), "true".to_string()));
    }
    Ok(client.get("/api/v1/data/actions", &query)?)
}

fn run_action(client: &ApiClient, command: ActionCommands) -> Result<Value, CommandError> {
    match command {
        ActionCommands::List(args) => list_actions(client, args),
        ActionCommands::Add(args) => action_add(client, args),
        ActionCommands::Update(args) => action_update(client, args),
        ActionCommands::Complete { action_id } => action_complete(client, &action_id),
        ActionCommands::Done { action_id } => action_status(client, &action_id, "done"),
        ActionCommands::Reopen { action_id } => action_status(client, &action_id, "open"),
        ActionCommands::Archive { action_id } => action_archive(client, &action_id, true),
        ActionCommands::Restore { action_id } => action_archive(client, &action_id, false),
        ActionCommands::Queue {
            action_id,
            position,
        } => {
            let version = fetch_top_version(client, "actions")?;
            let input = json!({"action_id": action_id, "position": position});
            post_object(client, "personal", "action.queue", input, version)
        }
        ActionCommands::Delete { action_id } => {
            let version = fetch_record_version(client, "actions", &["items"], &action_id)?;
            let input = json!({"action_id": action_id});
            post_object(client, "personal", "action.delete", input, version)
        }
    }
}

fn parse_json_arg(raw: &str, field: &str) -> Result<Value, CommandError> {
    serde_json::from_str(raw).map_err(|error| {
        LocalError::new(
            "INVALID_INPUT",
            format!("{field} must be valid JSON: {error}"),
        )
        .into()
    })
}

fn action_add(client: &ApiClient, args: ActionAddArgs) -> Result<Value, CommandError> {
    if args.due_date.is_some() && args.due_at.is_some() {
        return Err(LocalError::new(
            "INVALID_INPUT",
            "due_date and due_at are mutually exclusive",
        )
        .into());
    }

    let mut input = Map::new();
    input.insert("title".to_string(), Value::String(args.title));
    input.insert(
        "checkpoint_id".to_string(),
        Value::String(args.checkpoint_id),
    );
    if let Some(notes) = args.notes {
        input.insert("notes".to_string(), Value::String(notes));
    }
    if let Some(priority) = args.priority {
        input.insert("priority".to_string(), Value::String(priority));
    }
    if let Some(due_date) = args.due_date {
        input.insert("due_date".to_string(), Value::String(due_date));
    }
    if let Some(due_at) = args.due_at {
        input.insert("due_at".to_string(), Value::String(due_at));
    }
    if let Some(duration_minutes) = args.duration_minutes {
        input.insert("duration_minutes".to_string(), json!(duration_minutes));
    }
    if let Some(buffer_minutes) = args.buffer_minutes {
        input.insert("buffer_minutes".to_string(), json!(buffer_minutes));
    }
    if let Some(recurrence) = args.recurrence {
        input.insert(
            "recurrence".to_string(),
            parse_json_arg(&recurrence, "recurrence")?,
        );
    }
    if !args.tag.is_empty() {
        input.insert("tags".to_string(), json!(args.tag));
    }

    let version = fetch_top_version(client, "actions")?;
    post_command(client, "personal", "action.create", input, version)
}

fn action_update(client: &ApiClient, args: ActionUpdateArgs) -> Result<Value, CommandError> {
    let mut patch = Map::new();
    if let Some(title) = args.title {
        patch.insert("title".to_string(), Value::String(title));
    }
    if let Some(notes) = args.notes {
        patch.insert("notes".to_string(), Value::String(notes));
    }
    if let Some(priority) = args.priority {
        patch.insert("priority".to_string(), Value::String(priority));
    }
    if let Some(status) = args.status {
        patch.insert("status".to_string(), Value::String(status));
    }
    if args.clear_due {
        if args.due_date.is_some() || args.due_at.is_some() {
            return Err(LocalError::new(
                "INVALID_INPUT",
                "--clear-due cannot be combined with --due-date or --due-at",
            )
            .into());
        }
        patch.insert("due_at".to_string(), Value::Null);
        patch.insert("due_date".to_string(), Value::Null);
    } else {
        if args.due_date.is_some() && args.due_at.is_some() {
            return Err(LocalError::new(
                "INVALID_INPUT",
                "due_date and due_at are mutually exclusive",
            )
            .into());
        }
        if let Some(due_date) = args.due_date {
            patch.insert("due_date".to_string(), Value::String(due_date));
            patch.insert("due_at".to_string(), Value::Null);
        }
        if let Some(due_at) = args.due_at {
            patch.insert("due_at".to_string(), Value::String(due_at));
            patch.insert("due_date".to_string(), Value::Null);
        }
    }
    if let Some(duration_minutes) = args.duration_minutes {
        patch.insert("duration_minutes".to_string(), json!(duration_minutes));
    }
    if let Some(buffer_minutes) = args.buffer_minutes {
        patch.insert("buffer_minutes".to_string(), json!(buffer_minutes));
    }
    if let Some(recurrence) = args.recurrence {
        patch.insert(
            "recurrence".to_string(),
            parse_json_arg(&recurrence, "recurrence")?,
        );
    }
    if !args.tag.is_empty() {
        patch.insert("tags".to_string(), json!(args.tag));
    }
    if patch.is_empty() {
        return Err(
            LocalError::new("INVALID_INPUT", "action update requires at least one field").into(),
        );
    }

    let version = fetch_record_version(client, "actions", &["items"], &args.action_id)?;
    let mut input = Map::new();
    input.insert("action_id".to_string(), Value::String(args.action_id));
    input.insert("patch".to_string(), Value::Object(patch));
    post_command(client, "personal", "action.update", input, version)
}

fn action_complete(client: &ApiClient, action_id: &str) -> Result<Value, CommandError> {
    let version = fetch_record_version(client, "actions", &["items"], action_id)?;
    let input = json!({"action_id": action_id});
    post_object(client, "personal", "action.complete", input, version)
}

fn action_status(client: &ApiClient, action_id: &str, status: &str) -> Result<Value, CommandError> {
    let version = fetch_record_version(client, "actions", &["items"], action_id)?;
    let input = json!({"action_id": action_id, "patch": {"status": status}});
    post_object(client, "personal", "action.update", input, version)
}

fn action_archive(
    client: &ApiClient,
    action_id: &str,
    archived: bool,
) -> Result<Value, CommandError> {
    let version = fetch_record_version(client, "actions", &["items"], action_id)?;
    let input = json!({"action_id": action_id, "archived": archived});
    post_object(client, "personal", "action.archive", input, version)
}

fn run_goal(client: &ApiClient, command: GoalCommands) -> Result<Value, CommandError> {
    match command {
        GoalCommands::Progress(args) => {
            if args.percent > 100 {
                return Err(LocalError::new(
                    "INVALID_INPUT",
                    "--percent must be between 0 and 100",
                )
                .into());
            }
            let mut input = Map::new();
            input.insert("target".to_string(), Value::String("goal".to_string()));
            input.insert("target_id".to_string(), Value::String(args.goal_id));
            input.insert("kind".to_string(), Value::String(args.kind));
            input.insert("summary".to_string(), Value::String(args.summary));
            input.insert("progress_after".to_string(), Value::from(args.percent));
            if let Some(evidence) = args.evidence {
                input.insert("evidence".to_string(), Value::String(evidence));
            }
            let version = fetch_top_version(client, "goals")?;
            post_command(client, "goal", "progress.record", input, version)
        }
    }
}

fn run_write(client: &ApiClient, args: WriteArgs) -> Result<Value, CommandError> {
    let (program, command) = split_command(&args.command)?;

    let mut input = if let Some(raw) = args.input {
        parse_input(&raw)?
    } else if let Some(path) = args.input_file {
        let raw = std::fs::read_to_string(&path).map_err(|error| {
            LocalError::new(
                "INVALID_INPUT",
                format!("could not read {}: {error}", path.display()),
            )
        })?;
        parse_input(&raw)?
    } else {
        Map::new()
    };

    for (key, value) in parse_set_pairs(&args.set)? {
        input.insert(key, value);
    }

    let expected_version = match (args.expected_version, args.auto_version) {
        (Some(version), false) => version,
        (None, true) => auto_version(client, program, command, &input)?,
        (Some(_), true) => {
            return Err(LocalError::new(
                "INVALID_INPUT",
                "--expected-version and --auto-version are mutually exclusive",
            )
            .into());
        }
        (None, false) => {
            return Err(LocalError::new(
                "INVALID_INPUT",
                "--expected-version is required unless --auto-version is set",
            )
            .into());
        }
    };

    let request_id = args.request_id.unwrap_or_else(request_id);
    let envelope = json!({
        "request_id": request_id,
        "expected_version": expected_version,
        "input": Value::Object(input),
    });
    let path = format!("/api/v1/programs/{program}/commands/{command}");
    Ok(client.post(&path, &envelope)?)
}

fn post_command(
    client: &ApiClient,
    program: &str,
    command: &str,
    input: Map<String, Value>,
    expected_version: u64,
) -> Result<Value, CommandError> {
    post_object(
        client,
        program,
        command,
        Value::Object(input),
        expected_version,
    )
}

fn post_object(
    client: &ApiClient,
    program: &str,
    command: &str,
    input: Value,
    expected_version: u64,
) -> Result<Value, CommandError> {
    let envelope = json!({
        "request_id": request_id(),
        "expected_version": expected_version,
        "input": input,
    });
    let path = format!("/api/v1/programs/{program}/commands/{command}");
    Ok(client.post(&path, &envelope)?)
}

fn fetch_top_version(client: &ApiClient, resource: &str) -> Result<u64, CommandError> {
    let response = client.get(&format!("/api/v1/versions/{resource}"), &[])?;
    top_version(&response).ok_or_else(|| {
        LocalError::new(
            "NOT_FOUND",
            format!("{resource} has no top-level version; initialize it first"),
        )
        .into()
    })
}

fn fetch_record_version(
    client: &ApiClient,
    resource: &str,
    array_path: &[&str],
    id: &str,
) -> Result<u64, CommandError> {
    let query = vec![
        ("path".to_string(), array_path.join(".")),
        ("id".to_string(), id.to_string()),
    ];
    let response = client.get(&format!("/api/v1/versions/{resource}"), &query)?;
    record_version(&response, id).ok_or_else(|| {
        LocalError::new(
            "NOT_FOUND",
            format!("record {id} was not found in {resource}"),
        )
        .into()
    })
}

fn auto_version(
    client: &ApiClient,
    program: &str,
    command: &str,
    input: &Map<String, Value>,
) -> Result<u64, CommandError> {
    match (program, command) {
        ("personal", "action.update" | "action.complete" | "action.archive" | "action.delete") => {
            let id = required_input(input, "action_id")?;
            fetch_record_version(client, "actions", &["items"], id)
        }
        ("personal", "action.create" | "action.queue") => fetch_top_version(client, "actions"),
        ("personal", "profile.update") => fetch_top_version(client, "profile"),
        ("personal", "migrate") => Err(LocalError::new(
            "INVALID_INPUT",
            "personal.migrate requires an explicit --expected-version",
        )
        .into()),
        ("health", "body.update") => {
            let id = required_input(input, "record_id")?;
            fetch_record_version(client, "health", &["body", "records"], id)
        }
        ("health", "water.update") => {
            let id = required_input(input, "record_id")?;
            fetch_record_version(client, "health", &["water", "records"], id)
        }
        ("health", "sleep.update") => {
            let id = required_input(input, "record_id")?;
            fetch_record_version(client, "health", &["sleep", "records"], id)
        }
        ("health", _) => fetch_top_version(client, "health"),
        ("life", "clothes.update" | "clothes.wear" | "clothes.mark_dirty" | "clothes.archive") => {
            let id = required_input(input, "item_id")?;
            fetch_record_version(client, "life", &["clothes", "items"], id)
        }
        ("life", _) => fetch_top_version(client, "life"),
        ("goal", _) => fetch_top_version(client, "goals"),
        _ => Err(LocalError::new(
            "INVALID_INPUT",
            format!("no automatic version rule for {program}.{command}"),
        )
        .into()),
    }
}

fn required_input<'a>(input: &'a Map<String, Value>, key: &str) -> Result<&'a str, CommandError> {
    input_str(input, key).ok_or_else(|| {
        LocalError::new(
            "INVALID_INPUT",
            format!("--auto-version requires input field '{key}'"),
        )
        .into()
    })
}

fn parse_input(raw: &str) -> LocalResult<Map<String, Value>> {
    let value: Value = serde_json::from_str(raw).map_err(|error| {
        LocalError::new("INVALID_INPUT", format!("input is not valid JSON: {error}"))
    })?;
    match value {
        Value::Object(map) => Ok(map),
        _ => Err(LocalError::new(
            "INVALID_INPUT",
            "input must be a JSON object",
        )),
    }
}

fn parse_query_pairs(pairs: &[String]) -> LocalResult<Vec<(String, String)>> {
    let mut result = Vec::new();
    for pair in pairs {
        let (key, value) = pair.split_once('=').ok_or_else(|| {
            LocalError::new("INVALID_INPUT", format!("expected key=value, got '{pair}'"))
        })?;
        if key.trim().is_empty() {
            return Err(LocalError::new(
                "INVALID_INPUT",
                "query key must not be empty",
            ));
        }
        result.push((key.trim().to_string(), value.to_string()));
    }
    Ok(result)
}

fn split_command(raw: &str) -> LocalResult<(&str, &str)> {
    let (program, command) = raw
        .split_once('.')
        .ok_or_else(|| LocalError::new("INVALID_INPUT", "expected <program>.<command>"))?;
    ensure_segment(program, "program")?;
    ensure_segment(command, "command")?;
    Ok((program, command))
}

fn ensure_segment(raw: &str, label: &str) -> LocalResult<()> {
    if raw.is_empty()
        || !raw.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
    {
        return Err(LocalError::new(
            "INVALID_INPUT",
            format!("invalid {label} '{raw}'"),
        ));
    }
    Ok(())
}

fn push_opt(query: &mut Vec<(String, String)>, key: &str, value: Option<String>) {
    if let Some(value) = value {
        query.push((key.to_string(), value));
    }
}

fn print_json(value: &Value, pretty: bool) {
    if pretty {
        match serde_json::to_string_pretty(value) {
            Ok(text) => println!("{text}"),
            Err(error) => eprintln!("me-client: could not serialize output: {error}"),
        }
    } else {
        match serde_json::to_string(value) {
            Ok(text) => println!("{text}"),
            Err(error) => eprintln!("me-client: could not serialize output: {error}"),
        }
    }
}
