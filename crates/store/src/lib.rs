//! Document store backends implementing [`shared::DocumentStore`].

mod memory;
mod sqlite;

pub use memory::MemoryStore;
pub use sqlite::SqliteStore;
