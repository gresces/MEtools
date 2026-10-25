//! Blocking HTTP client for the me-server API.

use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use serde_json::Value;
use std::time::Duration;
use url::{Host, Url};

use crate::config::ClientConfig;

pub struct ApiClient {
    base: Url,
    token: Option<String>,
    http: Client,
}

#[derive(Debug)]
pub enum ApiError {
    Http { status: u16, body: Value },
    Transport(String),
    Decode(String),
}

impl ApiError {
    pub fn code(&self) -> &str {
        match self {
            Self::Http { body, .. } => body
                .get("error")
                .and_then(|error| error.get("code"))
                .and_then(Value::as_str)
                .unwrap_or("HTTP_ERROR"),
            Self::Transport(_) => "NETWORK_ERROR",
            Self::Decode(_) => "INVALID_RESPONSE",
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::Http { body, status } => body
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("server returned HTTP {status}")),
            Self::Transport(message) | Self::Decode(message) => message.clone(),
        }
    }

    pub fn body(&self) -> Value {
        match self {
            Self::Http { body, .. } => body.clone(),
            Self::Transport(message) => serde_json::json!({
                "status": "failed",
                "error": {"code": "NETWORK_ERROR", "message": message},
            }),
            Self::Decode(message) => serde_json::json!({
                "status": "failed",
                "error": {"code": "INVALID_RESPONSE", "message": message},
            }),
        }
    }

    pub fn exit_code(&self) -> u8 {
        if let Self::Http { status, body } = self {
            let code = body
                .get("error")
                .and_then(|error| error.get("code"))
                .and_then(Value::as_str)
                .unwrap_or("");
            return match code {
                "VERSION_CONFLICT" | "TIME_CONFLICT" => 4,
                "UNAUTHORIZED" | "FORBIDDEN" => 5,
                "NETWORK_ERROR" => 7,
                _ if *status >= 500 => 6,
                _ => 2,
            };
        }
        match self {
            Self::Transport(_) => 7,
            Self::Decode(_) => 6,
            Self::Http { .. } => unreachable!(),
        }
    }
}

fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        Some(Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        None => false,
    }
}

impl ApiClient {
    pub fn new(config: &ClientConfig) -> Result<Self, String> {
        let base =
            Url::parse(&config.server).map_err(|error| format!("invalid server URL: {error}"))?;
        // Loopback links are faster without compression; remote links benefit
        // from the smaller payload.
        let http = Client::builder()
            .gzip(!is_loopback(&base))
            .timeout(config.timeout.max(Duration::from_secs(1)))
            .build()
            .map_err(|error| format!("could not build HTTP client: {error}"))?;
        Ok(Self {
            base,
            token: config.token.clone(),
            http,
        })
    }

    pub fn get(&self, path: &str, query: &[(String, String)]) -> Result<Value, ApiError> {
        let url = self.url(path, query)?;
        let mut request = self.http.get(url).header(ACCEPT, "application/json");
        request = self.authorize(request);
        self.send(request)
    }

    pub fn post(&self, path: &str, body: &Value) -> Result<Value, ApiError> {
        let url = self.url(path, &[])?;
        let mut request = self
            .http
            .post(url)
            .header(ACCEPT, "application/json")
            .header(CONTENT_TYPE, "application/json")
            .json(body);
        request = self.authorize(request);
        self.send(request)
    }

    fn authorize(
        &self,
        request: reqwest::blocking::RequestBuilder,
    ) -> reqwest::blocking::RequestBuilder {
        let request = request.header(
            USER_AGENT,
            format!("me-client/{}", env!("CARGO_PKG_VERSION")),
        );
        match &self.token {
            Some(token) => request.header(AUTHORIZATION, format!("Bearer {token}")),
            None => request,
        }
    }

    fn url(&self, path: &str, query: &[(String, String)]) -> Result<Url, ApiError> {
        let mut url = self.base.clone();
        let (path, inline_query) = path
            .split_once('?')
            .map_or((path, ""), |(path, query)| (path, query));
        let base_path = url.path().trim_end_matches('/');
        let suffix = path.trim_start_matches('/');
        let joined = if base_path.is_empty() {
            format!("/{suffix}")
        } else {
            format!("{base_path}/{suffix}")
        };
        url.set_path(&joined);
        url.set_query(None);
        if !inline_query.is_empty() {
            url.set_query(Some(inline_query));
        }
        if !query.is_empty() {
            let mut pairs = url.query_pairs_mut();
            for (key, value) in query {
                pairs.append_pair(key, value);
            }
        }
        Ok(url)
    }

    fn send(&self, request: reqwest::blocking::RequestBuilder) -> Result<Value, ApiError> {
        let response = request
            .send()
            .map_err(|error| ApiError::Transport(format!("request failed: {error}")))?;
        let status = response.status();
        let text = response
            .text()
            .map_err(|error| ApiError::Decode(format!("could not read response: {error}")))?;

        let body: Value = if text.trim().is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text).map_err(|error| {
                ApiError::Decode(format!(
                    "server returned invalid JSON: {error}; body={text}"
                ))
            })?
        };

        if status.is_success() {
            Ok(body)
        } else {
            Err(ApiError::Http {
                status: status.as_u16(),
                body,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ClientConfig;

    #[test]
    fn compression_is_disabled_only_for_loopback() {
        assert!(is_loopback(&Url::parse("http://127.0.0.1:8787").unwrap()));
        assert!(is_loopback(&Url::parse("http://[::1]:8787").unwrap()));
        assert!(is_loopback(&Url::parse("http://localhost:8787").unwrap()));
        assert!(!is_loopback(&Url::parse("https://me.example.com").unwrap()));
    }

    #[test]
    fn url_keeps_path_and_query_separate() {
        let client = ApiClient::new(&ClientConfig {
            server: "http://127.0.0.1:8787".to_string(),
            token: None,
            timeout: Duration::from_secs(5),
        })
        .expect("client");
        let url = client
            .url("/api/v1/data/actions?include_archived=true", &[])
            .expect("url");
        assert_eq!(url.path(), "/api/v1/data/actions");
        assert_eq!(url.query(), Some("include_archived=true"));

        let url = client
            .url(
                "/api/v1/data/actions",
                &[("status".to_string(), "open".to_string())],
            )
            .expect("url");
        assert_eq!(url.path(), "/api/v1/data/actions");
        assert_eq!(url.query(), Some("status=open"));
    }
}
