//! Library surface for the `me-client` command-line tool.

pub mod api;
pub mod config;
pub mod util;

pub use api::{ApiClient, ApiError};
pub use config::{ClientConfig, resolve};
