//! Test-only command dispatch.
//!
//! Domain tests exercise handlers through an argv/JSON shape and resolve the
//! store from the caller. Production code never links this crate.

use std::collections::BTreeMap;
use std::io::Read;

use shared::{
    DocumentStore, FunctionFailure, JsonValue, ProgramSpec, Request, build_describe_command,
    build_describe_overview, build_error_response, build_help_response, parse_json,
    validate_request_value,
};

/// Dispatch `args` against `program` using `store`, appending one JSON response to `out`.
pub fn dispatch<R: Read>(
    store: &dyn DocumentStore,
    program: &ProgramSpec,
    args: &[String],
    mut stdin: R,
    out: &mut Vec<u8>,
) -> u8 {
    let (code, response) = run(store, program, args, &mut stdin);
    out.extend_from_slice(response.to_json_string().as_bytes());
    code
}

fn run<R: Read>(
    store: &dyn DocumentStore,
    program: &ProgramSpec,
    args: &[String],
    stdin: &mut R,
) -> (u8, JsonValue) {
    let result = match args {
        [] => Err(FunctionFailure::invalid_input("missing command")),
        [flag] if flag == "--help" || flag == "-h" || flag == "help" => {
            return (0, build_help_response(program));
        }
        [cmd] if cmd == "describe" => return (0, build_describe_overview(program)),
        [cmd, target] if cmd == "describe" => build_describe_command(program, target),
        [cmd] if cmd == "init" => (program.init_handler)(store),
        [cmd, rest @ ..] => {
            let Some(spec) = program.commands.iter().find(|spec| spec.name == cmd) else {
                return failure(FunctionFailure::invalid_input(format!(
                    "unknown command '{cmd}'"
                )));
            };
            let request = match parse_request(rest, stdin) {
                Ok(request) => request,
                Err(error) => return failure(error),
            };
            (spec.handler)(store, &request).map_err(|mut error| {
                if error.request_id.is_none() {
                    error.request_id = Some(request.request_id.clone());
                }
                error
            })
        }
    };

    match result {
        Ok(response) => (0, response),
        Err(error) => failure(error),
    }
}

fn failure(error: FunctionFailure) -> (u8, JsonValue) {
    (error.exit_code, build_error_response(&error))
}

fn parse_request<R: Read>(args: &[String], stdin: &mut R) -> Result<Request, FunctionFailure> {
    if args.is_empty() {
        let mut raw = String::new();
        stdin
            .read_to_string(&mut raw)
            .map_err(|_| FunctionFailure::invalid_input("stdin must contain valid UTF-8"))?;
        let payload = parse_json(&raw)
            .map_err(|_| FunctionFailure::invalid_input("stdin must contain valid JSON"))?;
        return validate_request_value(payload);
    }

    if args.len() == 2 && (args[0] == "--json" || args[0] == "--request") {
        let payload = parse_json(&args[1])
            .map_err(|_| FunctionFailure::invalid_input("--json must contain valid JSON"))?;
        return validate_request_value(payload);
    }
    if let Some(raw) = args[0]
        .strip_prefix("--json=")
        .or_else(|| args[0].strip_prefix("--request="))
        && args.len() == 1
    {
        let payload = parse_json(raw)
            .map_err(|_| FunctionFailure::invalid_input("--json must contain valid JSON"))?;
        return validate_request_value(payload);
    }

    let mut request_id: Option<String> = None;
    let mut expected_version: Option<u64> = None;
    let mut input: BTreeMap<String, JsonValue> = BTreeMap::new();

    let mut index = 0;
    while index < args.len() {
        let token = &args[index];
        let value_after = |index: &mut usize| -> Result<String, FunctionFailure> {
            *index += 1;
            args.get(*index)
                .cloned()
                .ok_or_else(|| FunctionFailure::invalid_input(format!("missing value for {token}")))
        };

        if let Some(value) = token.strip_prefix("--request-id=") {
            request_id = Some(value.to_string());
        } else if token == "--request-id" {
            request_id = Some(value_after(&mut index)?);
        } else if let Some(value) = token.strip_prefix("--expected-version=") {
            expected_version = Some(parse_version(value)?);
        } else if token == "--expected-version" {
            expected_version = Some(parse_version(&value_after(&mut index)?)?);
        } else if token == "--set" || token == "--field" {
            insert_pair(&mut input, &value_after(&mut index)?)?;
        } else if let Some(pair) = token
            .strip_prefix("--set=")
            .or_else(|| token.strip_prefix("--field="))
        {
            insert_pair(&mut input, pair)?;
        } else if let Some(flag) = token.strip_prefix("--") {
            if let Some((key, value)) = flag.split_once('=') {
                insert_value(&mut input, key, parse_literal(value))?;
            } else {
                let value = value_after(&mut index)?;
                insert_value(&mut input, flag, parse_literal(&value))?;
            }
        } else if let Some((key, value)) = token.split_once('=') {
            insert_value(&mut input, key, parse_literal(value))?;
        } else {
            return Err(FunctionFailure::invalid_input(format!(
                "unexpected argument '{token}'"
            )));
        }
        index += 1;
    }

    let request_id = request_id
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| FunctionFailure::invalid_input("request_id is required"))?;
    let expected_version = expected_version
        .ok_or_else(|| FunctionFailure::invalid_input("expected_version is required"))?;

    Ok(Request {
        request_id,
        expected_version,
        input,
    })
}

fn parse_version(raw: &str) -> Result<u64, FunctionFailure> {
    raw.parse()
        .map_err(|_| FunctionFailure::invalid_input("expected_version must be an integer"))
}

fn insert_pair(input: &mut BTreeMap<String, JsonValue>, pair: &str) -> Result<(), FunctionFailure> {
    let (key, value) = pair.split_once('=').ok_or_else(|| {
        FunctionFailure::invalid_input(format!("expected key=value, got '{pair}'"))
    })?;
    insert_value(input, key, parse_literal(value))
}

fn insert_value(
    input: &mut BTreeMap<String, JsonValue>,
    path: &str,
    value: JsonValue,
) -> Result<(), FunctionFailure> {
    let segments: Vec<&str> = path.split('.').collect();
    insert_segments(input, &segments, value)
}

fn insert_segments(
    current: &mut BTreeMap<String, JsonValue>,
    segments: &[&str],
    value: JsonValue,
) -> Result<(), FunctionFailure> {
    let (first, rest) = segments
        .split_first()
        .ok_or_else(|| FunctionFailure::invalid_input("empty field name"))?;
    if rest.is_empty() {
        current.insert((*first).to_string(), value);
        return Ok(());
    }
    let nested = current
        .entry((*first).to_string())
        .or_insert_with(|| JsonValue::Object(BTreeMap::new()));
    let object = nested.as_object_mut().ok_or_else(|| {
        FunctionFailure::invalid_input(format!("field '{first}' is not an object"))
    })?;
    insert_segments(object, rest, value)
}

fn parse_literal(raw: &str) -> JsonValue {
    let trimmed = raw.trim();
    let looks_json = trimmed == "true"
        || trimmed == "false"
        || trimmed == "null"
        || trimmed.starts_with('"')
        || trimmed.starts_with('[')
        || trimmed.starts_with('{')
        || trimmed.starts_with('-')
        || trimmed.as_bytes().first().is_some_and(u8::is_ascii_digit);
    if looks_json && let Ok(parsed) = parse_json(trimmed) {
        return parsed;
    }
    JsonValue::String(raw.to_string())
}
