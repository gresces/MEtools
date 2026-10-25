//! Server configuration and command-line parsing.

use anyhow::{Context, Result, bail};
use chrono::FixedOffset;
use clap::{Args, Parser, Subcommand};
use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;

pub const DEFAULT_BIND: &str = "127.0.0.1:8787";
pub const DEFAULT_UTC_OFFSET_HOURS: i32 = 8;
pub const DEFAULT_SESSION_TTL_HOURS: u64 = 24 * 30;
/// Password used when neither `--password` nor `--password-file` is provided.
pub const DEFAULT_PASSWORD: &str = "password";
const MIN_PASSWORD_LEN: usize = 8;

/// Top-level command-line interface.
#[derive(Debug, Parser)]
#[command(
    name = "me-server",
    version,
    about = "ME personal data server, HTTP API, and dashboard",
    long_about = "Run the ME personal data server. Data lives in a SQLite document database selected with --db; writes go through the HTTP API."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Start the HTTP server and dashboard.
    Serve(ServeArgs),
    /// Create missing collections and validate existing ones.
    Init(InitArgs),
    /// Read and validate every known collection without modifying it.
    Check(CheckArgs),
    /// Import a legacy Data/ tree into the database.
    Import(ImportArgs),
    /// Write a consistent database snapshot with VACUUM INTO.
    Backup(BackupArgs),
    /// Generate a strong random password.
    Password,
}

#[derive(Debug, Args, Clone)]
pub struct ServeArgs {
    /// Address to listen on.
    #[arg(long, env = "ME_BIND", default_value = DEFAULT_BIND)]
    pub bind: SocketAddr,

    /// SQLite database path (defaults to $XDG_DATA_HOME/me/me.db).
    #[arg(long, env = "ME_DB")]
    pub db: Option<PathBuf>,

    /// Password for dashboard login and API access. Defaults to `password` on loopback.
    #[arg(long, env = "ME_SERVER_PASSWORD", hide_env_values = true)]
    pub password: Option<String>,

    /// File containing the password (whitespace is trimmed).
    #[arg(long, env = "ME_SERVER_PASSWORD_FILE")]
    pub password_file: Option<PathBuf>,

    /// Disable authentication. Only allowed for loopback binds unless --allow-insecure is set.
    #[arg(long)]
    pub no_auth: bool,

    /// Permit --no-auth on a non-loopback address. This exposes personal data.
    #[arg(long)]
    pub allow_insecure: bool,

    /// UTC offset used for "today" calculations and dashboard labels.
    #[arg(
        long,
        env = "ME_UTC_OFFSET_HOURS",
        default_value_t = DEFAULT_UTC_OFFSET_HOURS
    )]
    pub utc_offset_hours: i32,

    /// Dashboard session cookie lifetime in hours.
    #[arg(
        long,
        env = "ME_SESSION_TTL_HOURS",
        default_value_t = DEFAULT_SESSION_TTL_HOURS
    )]
    pub session_ttl_hours: u64,

    /// Mark the dashboard session cookie as Secure (enable when served over HTTPS).
    #[arg(long, env = "ME_SECURE_COOKIE")]
    pub secure_cookie: bool,
}

#[derive(Debug, Args, Clone)]
pub struct InitArgs {
    #[arg(long, env = "ME_DB")]
    pub db: Option<PathBuf>,
}

#[derive(Debug, Args, Clone)]
pub struct CheckArgs {
    #[arg(long, env = "ME_DB")]
    pub db: Option<PathBuf>,
}

#[derive(Debug, Args, Clone)]
pub struct ImportArgs {
    /// ME repository root or directory containing Data/.
    #[arg(long, env = "ME_IMPORT_FROM")]
    pub from: PathBuf,

    /// SQLite database path.
    #[arg(long, env = "ME_DB")]
    pub db: Option<PathBuf>,

    /// Replace collections that already exist in the database.
    #[arg(long)]
    pub replace: bool,
}

#[derive(Debug, Args, Clone)]
pub struct BackupArgs {
    /// SQLite database path.
    #[arg(long, env = "ME_DB")]
    pub db: Option<PathBuf>,

    /// Destination file for the snapshot; must not exist.
    #[arg(long)]
    pub to: PathBuf,
}

/// Session TTL in seconds.
pub fn session_ttl_seconds(hours: u64) -> u64 {
    hours.saturating_mul(3600)
}

/// Effective runtime configuration for the server.
#[derive(Debug, Clone)]
pub struct Config {
    pub bind: SocketAddr,
    pub db_path: PathBuf,
    pub password: Option<String>,
    pub no_auth: bool,
    pub allow_insecure: bool,
    pub utc_offset_hours: i32,
    pub session_ttl_secs: u64,
    pub secure_cookie: bool,
}

impl Config {
    pub fn from_args(args: &ServeArgs) -> Result<Self> {
        if !(-12..=14).contains(&args.utc_offset_hours) {
            bail!("--utc-offset-hours must be between -12 and 14");
        }
        if args.session_ttl_hours == 0 {
            bail!("--session-ttl-hours must be greater than zero");
        }
        if args.no_auth && (args.password.is_some() || args.password_file.is_some()) {
            bail!("--no-auth and --password are mutually exclusive");
        }

        let (password, uses_default_password) = if args.no_auth {
            (None, false)
        } else {
            let (password, is_default) = resolve_password(args)?;
            (Some(password), is_default)
        };

        let unauthenticated = password.is_none();
        if (unauthenticated || uses_default_password)
            && !args.bind.ip().is_loopback()
            && !args.allow_insecure
        {
            bail!(
                "refusing to serve personal data without explicit authentication on {}; \
                 set ME_SERVER_PASSWORD, use --password-file, or pass --allow-insecure explicitly",
                args.bind
            );
        }

        let db_path = resolve_db_path(args.db.clone());

        Ok(Self {
            bind: args.bind,
            db_path,
            password,
            no_auth: args.no_auth,
            allow_insecure: args.allow_insecure,
            utc_offset_hours: args.utc_offset_hours,
            session_ttl_secs: session_ttl_seconds(args.session_ttl_hours),
            secure_cookie: args.secure_cookie,
        })
    }

    /// Fixed UTC offset used by read models and dashboard dates.
    pub fn tz(&self) -> FixedOffset {
        FixedOffset::east_opt(self.utc_offset_hours * 3600)
            .unwrap_or_else(|| FixedOffset::east_opt(0).expect("zero offset"))
    }

    pub fn auth_required(&self) -> bool {
        self.password.is_some()
    }
}

/// Resolve the password from `--password` or `--password-file`.
///
/// Returns `(password, is_default)`. The default is only accepted on loopback
/// binds, which the caller enforces.
fn resolve_password(args: &ServeArgs) -> Result<(String, bool)> {
    let raw = if let Some(password) = &args.password {
        password.clone()
    } else if let Some(path) = &args.password_file {
        fs::read_to_string(path)
            .with_context(|| format!("could not read password file {}", path.display()))?
    } else {
        return Ok((DEFAULT_PASSWORD.to_string(), true));
    };

    let password = raw.trim().to_string();
    if password.len() < MIN_PASSWORD_LEN {
        bail!("password must be at least {MIN_PASSWORD_LEN} characters");
    }
    if password.len() > 512 {
        bail!("password is too long (maximum 512 characters)");
    }
    if password.chars().any(char::is_control) {
        bail!("password must not contain control characters");
    }
    Ok((password, false))
}

/// Resolve `--db`, falling back to the XDG data directory.
pub fn resolve_db_path(db: Option<PathBuf>) -> PathBuf {
    let path = db.unwrap_or_else(default_db_path);
    if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(&path))
            .unwrap_or(path)
    }
}

/// Default database path: `$XDG_DATA_HOME/me/me.db`, else `~/.local/share/me/me.db`.
pub fn default_db_path() -> PathBuf {
    default_db_path_from(std::env::var_os("XDG_DATA_HOME"), std::env::var_os("HOME"))
}

/// Pure variant of [`default_db_path`] used by tests.
pub fn default_db_path_from(
    xdg_data_home: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> PathBuf {
    let base = xdg_data_home
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.map(|value| PathBuf::from(value).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("me").join("me.db")
}

/// Generate a password with roughly 244 bits of randomness.
pub fn generate_password() -> String {
    let first = uuid::Uuid::new_v4().simple().to_string();
    let second = uuid::Uuid::new_v4().simple().to_string();
    format!("{first}{second}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args() -> ServeArgs {
        ServeArgs {
            bind: "127.0.0.1:8787".parse().expect("bind"),
            db: None,
            password: None,
            password_file: None,
            no_auth: false,
            allow_insecure: false,
            utc_offset_hours: 8,
            session_ttl_hours: 720,
            secure_cookie: false,
        }
    }

    #[test]
    fn no_auth_is_allowed_only_on_loopback() {
        let mut local = args();
        local.no_auth = true;
        assert!(Config::from_args(&local).is_ok());

        let mut public = args();
        public.bind = "0.0.0.0:8787".parse().expect("bind");
        public.no_auth = true;
        assert!(Config::from_args(&public).is_err());

        public.allow_insecure = true;
        assert!(Config::from_args(&public).is_ok());
    }

    #[test]
    fn default_password_applies_on_loopback_only() {
        let config = Config::from_args(&args()).expect("loopback default");
        assert_eq!(config.password.as_deref(), Some(DEFAULT_PASSWORD));
        assert!(config.auth_required());

        let mut public = args();
        public.bind = "0.0.0.0:8787".parse().expect("bind");
        assert!(Config::from_args(&public).is_err());

        public.allow_insecure = true;
        assert!(Config::from_args(&public).is_ok());
    }

    #[test]
    fn explicit_password_must_be_long_enough() {
        let mut weak = args();
        weak.password = Some("short".to_string());
        assert!(Config::from_args(&weak).is_err());

        let mut strong = args();
        strong.password = Some("0123456789abcdef".to_string());
        assert!(Config::from_args(&strong).is_ok());

        strong.bind = "0.0.0.0:8787".parse().expect("bind");
        assert!(Config::from_args(&strong).is_ok());
    }

    #[test]
    fn password_file_is_trimmed() {
        let path = std::env::temp_dir().join(format!("me-password-{}.txt", std::process::id()));
        fs::write(&path, "  file-password-123  \n").expect("fixture");
        let mut value = args();
        value.password_file = Some(path.clone());
        let config = Config::from_args(&value).expect("password file");
        assert_eq!(config.password.as_deref(), Some("file-password-123"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn default_db_path_prefers_xdg_data_home() {
        let path = default_db_path_from(
            Some(std::ffi::OsString::from("/xdg")),
            Some(std::ffi::OsString::from("/home/me")),
        );
        assert_eq!(path, PathBuf::from("/xdg/me/me.db"));

        let fallback = default_db_path_from(None, Some(std::ffi::OsString::from("/home/me")));
        assert_eq!(fallback, PathBuf::from("/home/me/.local/share/me/me.db"));
    }

    #[test]
    fn no_auth_and_password_conflict() {
        let mut value = args();
        value.no_auth = true;
        value.password = Some("0123456789abcdef".to_string());
        assert!(Config::from_args(&value).is_err());
    }
}
