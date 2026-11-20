//! SQLite WAL persistence layer, Schema v2 migrations, and legacy upgrade engine.

#![deny(unsafe_code)]

pub mod db;
pub mod legacy_migration;
pub mod migrations;
pub mod settings;
pub mod vault_repair;

pub use db::GreenCDb;
pub use legacy_migration::{
    auto_migrate_legacy_db, auto_migrate_legacy_db_with_candidates, get_legacy_db_candidates,
    migrate_legacy_db, move_vault, resolve_default_db_path, MoveVaultSummary,
};
pub use migrations::{run_migrations, CURRENT_SCHEMA_VERSION};
pub use settings::{AppSettings, ThemeMode};
