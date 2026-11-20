//! greenc-core: Foundational types, constants, errors, and task control for GreenC v2.

#![deny(unsafe_code)]

pub mod config;
pub mod error;
pub mod models;
pub mod task;

pub use config::*;
pub use error::{GreenCError, Result};
pub use models::*;
pub use task::{ProgressReporter, TaskControl, TaskProgress, WalkProgressCallback};
