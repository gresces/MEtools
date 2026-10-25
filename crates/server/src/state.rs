//! Shared application state and the static program registry.

use chrono::{DateTime, FixedOffset, Utc};
use shared::{CommandSpec, ProgramSpec};
use std::sync::Arc;

use me_store::SqliteStore;

use crate::config::Config;

/// Read/write command programs exposed by the server.
pub struct ProgramRegistry {
    programs: Vec<&'static ProgramSpec>,
}

impl ProgramRegistry {
    pub fn new() -> Self {
        Self {
            programs: vec![
                personal::program(),
                health::program(),
                life::program(),
                goal::program(),
            ],
        }
    }

    pub fn all(&self) -> &[&'static ProgramSpec] {
        &self.programs
    }

    pub fn get(&self, name: &str) -> Option<&'static ProgramSpec> {
        self.programs
            .iter()
            .copied()
            .find(|program| program.name == name)
    }

    pub fn command(
        &self,
        program: &str,
        command: &str,
    ) -> Option<(&'static ProgramSpec, &'static CommandSpec)> {
        let program = self.get(program)?;
        let command = program.commands.iter().find(|spec| spec.name == command)?;
        Some((program, command))
    }
}

impl Default for ProgramRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// State shared by HTTP handlers.
pub struct AppState {
    pub config: Config,
    pub store: Arc<SqliteStore>,
    pub programs: ProgramRegistry,
    pub started_at: DateTime<FixedOffset>,
}

impl AppState {
    pub fn new(config: Config, store: Arc<SqliteStore>) -> Self {
        let started_at = Utc::now().with_timezone(&config.tz());
        Self {
            config,
            store,
            programs: ProgramRegistry::new(),
            started_at,
        }
    }
}

pub type SharedState = Arc<AppState>;
