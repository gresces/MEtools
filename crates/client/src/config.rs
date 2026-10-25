//! Client configuration resolution.

use serde::Deserialize;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use url::Url;

pub const DEFAULT_SERVER: &str = "http://127.0.0.1:8787";
pub const DEFAULT_TIMEOUT_SECS: u64 = 30;

#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub server: String,
    pub token: Option<String>,
    pub timeout: Duration,
}

#[derive(Debug, Default, Deserialize)]
struct FileConfig {
    server: Option<String>,
    token: Option<String>,
    token_file: Option<PathBuf>,
    timeout_secs: Option<u64>,
}

/// Resolve CLI flags, environment, config file, and defaults into a client config.
pub fn resolve(
    server: Option<String>,
    token: Option<String>,
    token_file: Option<PathBuf>,
    timeout_secs: Option<u64>,
) -> Result<ClientConfig, String> {
    let file = load_file_config()?;

    let server = server
        .or_else(|| env::var("ME_SERVER_URL").ok())
        .or(file.server)
        .unwrap_or_else(|| DEFAULT_SERVER.to_string());
    let server = normalize_server(&server)?;

    let resolved_token = if let Some(token) = token {
        Some(token.trim().to_string()).filter(|value| !value.is_empty())
    } else if let Some(path) = token_file.or(file.token_file) {
        Some(read_token_file(&path)?)
    } else if let Some(token) = file.token {
        Some(token.trim().to_string()).filter(|value| !value.is_empty())
    } else {
        env::var("ME_SERVER_TOKEN")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    };

    let timeout_secs = timeout_secs
        .or(file.timeout_secs)
        .unwrap_or(DEFAULT_TIMEOUT_SECS)
        .clamp(1, 600);

    Ok(ClientConfig {
        server,
        token: resolved_token,
        timeout: Duration::from_secs(timeout_secs),
    })
}

fn normalize_server(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim().trim_end_matches('/');
    let url = Url::parse(trimmed).map_err(|error| format!("invalid server URL: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("server URL must use http or https".to_string());
    }
    if url.host_str().is_none() {
        return Err("server URL must include a host".to_string());
    }
    Ok(trimmed.to_string())
}

fn read_token_file(path: &Path) -> Result<String, String> {
    let raw = fs::read_to_string(path)
        .map_err(|error| format!("could not read token file {}: {error}", path.display()))?;
    let token = raw.trim().to_string();
    if token.is_empty() {
        return Err(format!("token file {} is empty", path.display()));
    }
    Ok(token)
}

/// Load the first config file found: `$ME_CLIENT_CONFIG`, `./.me-client.json`,
/// `$XDG_CONFIG_HOME/me/client.json`, or `~/.config/me/client.json`.
fn load_file_config() -> Result<FileConfig, String> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(path) = env::var_os("ME_CLIENT_CONFIG") {
        candidates.push(PathBuf::from(path));
    }
    candidates.push(PathBuf::from(".me-client.json"));
    if let Some(config_home) = env::var_os("XDG_CONFIG_HOME") {
        candidates.push(PathBuf::from(config_home).join("me").join("client.json"));
    } else if let Some(home) = env::var_os("HOME") {
        candidates.push(
            PathBuf::from(home)
                .join(".config")
                .join("me")
                .join("client.json"),
        );
    }

    for path in candidates {
        if !path.is_file() {
            continue;
        }
        let raw = fs::read_to_string(&path)
            .map_err(|error| format!("could not read {}: {error}", path.display()))?;
        let parsed: FileConfig = serde_json::from_str(&raw)
            .map_err(|error| format!("{} is not valid JSON: {error}", path.display()))?;
        return Ok(parsed);
    }

    Ok(FileConfig::default())
}
