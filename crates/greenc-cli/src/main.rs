//! Headless GreenC command-line interface binary (`contracts/cli-contract.md`, Task T067).

#![deny(unsafe_code)]

use clap::Parser;
use greenc_cli::{execute_cli, Cli, EXIT_CANCELLED};
use greenc_core::config::default_db_path;
use greenc_core::task::TaskControl;
#[cfg(not(feature = "store"))]
use greenc_storage::legacy_migration::auto_migrate_legacy_db;
use std::process::ExitCode;
use std::sync::Arc;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();

    // Short-circuit internal MSIX breakaway helper BEFORE touching SQLite or `default_db_path()`
    // so an unpackaged child process never creates a plain `%LOCALAPPDATA%\GreenC-Store` folder.
    if matches!(cli.command, greenc_cli::Commands::RestoreHelper) {
        let code = greenc_cli::run_restore_helper_stdio();
        let code_u8 = u8::try_from(code.clamp(0, 255)).unwrap_or(1);
        return ExitCode::from(code_u8);
    }

    let ctrl = Arc::new(TaskControl::new());

    // Install cooperative Ctrl+C handler -> guarantees source data stays intact and exits 130.
    {
        let ctrl_signal = Arc::clone(&ctrl);
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                ctrl_signal.cancel();
            }
        });
    }

    let db_path = default_db_path();
    #[cfg(not(feature = "store"))]
    let _ = auto_migrate_legacy_db(&db_path);
    if let Ok(db) = greenc_storage::db::GreenCDb::open(&db_path) {
        let _ = greenc_storage::vault_repair::repair_vault_paths_default(&db);
    }

    let ctrl_worker = Arc::clone(&ctrl);
    let outcome = tokio::task::spawn_blocking(move || execute_cli(cli, &db_path, &ctrl_worker))
        .await
        .unwrap_or_else(|e| greenc_cli::CliOutcome {
            exit_code: 1,
            stdout: String::new(),
            stderr: format!("CLI worker panicked: {e}"),
        });

    if !outcome.stdout.is_empty() {
        println!("{}", outcome.stdout);
    }
    if !outcome.stderr.is_empty() {
        eprintln!("{}", outcome.stderr);
    }

    let code_u8 = if outcome.exit_code == EXIT_CANCELLED {
        130u8
    } else {
        u8::try_from(outcome.exit_code.clamp(0, 255)).unwrap_or(1)
    };
    ExitCode::from(code_u8)
}
