//! One-shot import of a legacy `Data/` tree into the document store.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use shared::{Document, DocumentStore, JsonValue, parse_json};

/// Import every known collection from `from` (a ME repository root or a `Data/` directory).
///
/// Existing collections are left untouched unless `replace` is set.
pub fn import_data_dir(
    store: &dyn DocumentStore,
    from: &Path,
    replace: bool,
) -> Result<Vec<Value>> {
    let data = if from.join("Data").is_dir() {
        from.join("Data")
    } else {
        from.to_path_buf()
    };
    let personal = data.join("personal");
    let goal = data.join("goal");

    let mut report = Vec::new();
    let mut entries = Vec::new();

    for (collection, path) in [
        ("profile", personal.join("profile.json")),
        ("actions", personal.join("tasks.json")),
        ("health", personal.join("health.json")),
        ("life", personal.join("life.json")),
        ("goals", goal.join("goals.json")),
    ] {
        if let Some(document) = read_document(&path)? {
            entries.push((collection, document));
        }
    }

    if let Some(projects) = read_projects(&data.join("projects"))? {
        entries.push(("projects", projects));
    }

    let mut prepared = Vec::new();
    for (collection, document) in entries {
        let document = match collection {
            "actions" => {
                let version = document
                    .get("schema_version")
                    .and_then(JsonValue::as_u64)
                    .unwrap_or(0);
                if version < personal::SCHEMA_VERSION {
                    personal::migrate_document(&document).with_context(|| {
                        format!(
                            "could not migrate actions to schema_version={}",
                            personal::SCHEMA_VERSION
                        )
                    })?
                } else {
                    document
                }
            }
            "goals" => goal::migrate_legacy_document(&document)
                .with_context(|| "could not migrate goals to schema_version=2".to_string())?,
            _ => document,
        };
        prepared.push((collection.to_string(), document));
    }
    validate_prepared(&prepared)?;

    for (collection, document) in prepared {
        let inserted = if replace {
            store
                .replace(&collection, &document)
                .with_context(|| format!("could not replace collection {collection}"))?;
            true
        } else {
            store
                .insert_initial(&collection, &document)
                .with_context(|| format!("could not insert collection {collection}"))?
        };

        if !inserted {
            bail!("collection {collection} already exists in the database; rerun with --replace");
        }

        report.push(json!({
            "collection": collection,
            "version": document.get("version").and_then(JsonValue::as_u64),
            "count": document
                .get("items")
                .and_then(JsonValue::as_array)
                .map(|items| items.len()),
            "source": source_path(&data, &collection),
        }));
    }

    Ok(report)
}

/// Validate every collection before writing any of them, so a failed import
/// leaves the database untouched.
fn validate_prepared(prepared: &[(String, Document)]) -> Result<()> {
    let actions = prepared
        .iter()
        .find(|(collection, _)| collection == "actions")
        .map(|(_, document)| document);

    for (collection, document) in prepared {
        let result = match collection.as_str() {
            "actions" => personal::validate_action_document(document),
            "health" => health::validate_document(document).map(|_| ()),
            "life" => life::validate_document(document).map(|_| ()),
            "goals" => goal::validate_document(document, actions).map(|_| ()),
            _ => Ok(()),
        };
        result.with_context(|| format!("collection {collection} failed validation"))?;
    }

    Ok(())
}

fn read_document(path: &Path) -> Result<Option<Document>> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("could not read {}", path.display()));
        }
    };
    let value = parse_json(&raw)
        .map_err(|_| anyhow::anyhow!("{} does not contain valid JSON", path.display()))?;
    let mut document = value
        .as_object()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("{} must contain a JSON object", path.display()))?;
    document
        .entry("version".to_string())
        .or_insert(JsonValue::from(1_u64));
    Ok(Some(document))
}

fn read_projects(projects_dir: &Path) -> Result<Option<Document>> {
    if !projects_dir.is_dir() {
        return Ok(None);
    }

    let mut items = Vec::new();
    for entry in std::fs::read_dir(projects_dir)
        .with_context(|| format!("could not read {}", projects_dir.display()))?
    {
        let entry = entry?;
        if !entry.path().is_dir() {
            continue;
        }
        let project_file = entry.path().join("project.json");
        let raw = match std::fs::read_to_string(&project_file) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("could not read {}", project_file.display()));
            }
        };
        let value = parse_json(&raw).map_err(|_| {
            anyhow::anyhow!("{} does not contain valid JSON", project_file.display())
        })?;
        let mut project = value.as_object().cloned().ok_or_else(|| {
            anyhow::anyhow!("{} must contain a JSON object", project_file.display())
        })?;
        project
            .entry("id".to_string())
            .or_insert_with(|| JsonValue::from(entry.file_name().to_string_lossy().into_owned()));
        items.push(JsonValue::Object(project));
    }

    if items.is_empty() {
        return Ok(None);
    }

    let mut document = Document::new();
    document.insert("version".to_string(), JsonValue::from(1_u64));
    document.insert(
        "updated_at".to_string(),
        JsonValue::from(shared::now_iso8601()),
    );
    document.insert("updated_by".to_string(), JsonValue::from("import"));
    document.insert("items".to_string(), JsonValue::Array(items));
    Ok(Some(document))
}

fn source_path(data: &Path, collection: &str) -> PathBuf {
    match collection {
        "goals" => data.join("goal/goals.json"),
        "actions" => data.join("personal/tasks.json"),
        "projects" => data.join("projects"),
        _ => data.join("personal").join(format!("{collection}.json")),
    }
}
