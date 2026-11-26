//! Opt-in diagnostics Tauri IPC commands, shared command recording wrappers,
//! and background batch sender (excluded from `store` builds).

use crate::state::AppState;
use chrono::Utc;
use greenc_core::error::GreenCError;
use greenc_diagnostics::{ConsentState, Diagnostics, Event, Outcome};
use std::sync::atomic::Ordering;
use std::time::Instant;
use tauri::{AppHandle, State};

/// Captures a command error along with its privacy-safe diagnostic code before
/// converting the error into a user-facing string.
pub struct CommandFailure {
    pub message: String,
    pub code: String,
    pub cancelled: bool,
}

impl CommandFailure {
    pub fn from_err(err: &GreenCError, message: String) -> Self {
        Self {
            message,
            code: err.diagnostic_code(),
            cancelled: matches!(err, GreenCError::Cancelled),
        }
    }

    pub fn from_msg(message: impl Into<String>, code: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: code.into(),
            cancelled: false,
        }
    }

    pub const fn outcome(&self) -> Outcome {
        if self.cancelled {
            Outcome::Cancelled
        } else {
            Outcome::Failed
        }
    }
}

/// Records a `scan` event for a completed command result while preserving the
/// exact return value and user-facing error message.
pub fn record_scan_result<T>(
    diag: &Diagnostics,
    name: &str,
    started: Instant,
    res: std::result::Result<T, CommandFailure>,
    classify_ok: impl FnOnce(&T) -> Outcome,
) -> std::result::Result<T, String> {
    let ms = started.elapsed().as_millis() as u64;
    match res {
        Ok(val) => {
            let outcome = classify_ok(&val);
            diag.record(
                Event::Scan {
                    name,
                    outcome,
                    code: "",
                    ms,
                },
                Utc::now(),
            );
            Ok(val)
        }
        Err(fail) => {
            diag.record(
                Event::Scan {
                    name,
                    outcome: fail.outcome(),
                    code: &fail.code,
                    ms,
                },
                Utc::now(),
            );
            Err(fail.message)
        }
    }
}

/// Records an `action` event for a completed command result while preserving the
/// exact return value and user-facing error message.
pub fn record_action_result<T>(
    diag: &Diagnostics,
    name: &str,
    started: Instant,
    res: std::result::Result<T, CommandFailure>,
    classify_ok: impl FnOnce(&T) -> Outcome,
) -> std::result::Result<T, String> {
    let ms = started.elapsed().as_millis() as u64;
    match res {
        Ok(val) => {
            let outcome = classify_ok(&val);
            diag.record(
                Event::Action {
                    name,
                    outcome,
                    code: "",
                    ms,
                },
                Utc::now(),
            );
            Ok(val)
        }
        Err(fail) => {
            diag.record(
                Event::Action {
                    name,
                    outcome: fail.outcome(),
                    code: &fail.code,
                    ms,
                },
                Utc::now(),
            );
            Err(fail.message)
        }
    }
}

#[tauri::command]
pub fn get_diagnostics_consent(state: State<'_, AppState>) -> ConsentState {
    state.diagnostics.consent()
}

#[tauri::command]
pub fn set_diagnostics_consent(granted: bool, state: State<'_, AppState>) -> ConsentState {
    state.diagnostics.set_consent(granted);
    state.diagnostics.consent()
}

#[tauri::command]
pub fn dismiss_diagnostics_prompt(state: State<'_, AppState>) -> ConsentState {
    state.diagnostics.prompt_dismissed();
    state.diagnostics.consent()
}

#[tauri::command]
pub fn preview_diagnostics(state: State<'_, AppState>) -> String {
    state.diagnostics.preview(Utc::now())
}

#[tauri::command]
pub fn diagnostics_ui_ready(state: State<'_, AppState>) {
    let now = Utc::now();
    state.diagnostics.record(Event::Active, now);
    if !state.is_start_minimized() && !state.start_recorded.swap(true, Ordering::SeqCst) {
        let ms = state.started_at.elapsed().as_millis() as u64;
        state.diagnostics.record(Event::Start { ms }, now);
    }
}

#[tauri::command]
pub fn diagnostics_screen(name: String, ms: u64, cached: bool, state: State<'_, AppState>) {
    state.diagnostics.record(
        Event::Screen {
            name: &name,
            cached,
            ms,
        },
        Utc::now(),
    );
}

#[tauri::command]
pub fn export_diagnostics_log(
    app: AppHandle,
    state: State<'_, AppState>,
) -> std::result::Result<String, String> {
    #[cfg(feature = "store")]
    {
        let _ = (app, state);
        Ok(String::new())
    }

    #[cfg(not(feature = "store"))]
    {
        use tauri::Manager;
        let base_dir = app
            .path()
            .download_dir()
            .unwrap_or_else(|_| state.app_dir.clone());
        let filename = format!(
            "GreenC-log-{}.txt",
            chrono::Local::now().format("%Y%m%d-%H%M")
        );
        let dest = base_dir.join(filename);
        state
            .diagnostics
            .export_log(&dest)
            .map_err(|e| e.to_string())?;
        Ok(dest.to_string_lossy().into_owned())
    }
}

#[tauri::command]
pub fn clear_diagnostics_log(state: State<'_, AppState>) {
    state.diagnostics.clear_log();
}

/// Spawns the background diagnostics sender task (30s startup delay, then hourly).
/// Not compiled in `store` builds.
#[cfg(not(feature = "store"))]
pub fn spawn_diagnostics_sender(diag: std::sync::Arc<Diagnostics>) {
    use greenc_diagnostics::{SendResult, ENDPOINT};
    use std::time::Duration;

    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(30)).await;
        let user_agent = format!("GreenC/{}", env!("CARGO_PKG_VERSION"));

        loop {
            if let Some(batch) = diag.take_due_batch(Utc::now()) {
                let result = match reqwest::Client::builder()
                    .timeout(Duration::from_secs(10))
                    .redirect(reqwest::redirect::Policy::none())
                    .user_agent(user_agent.clone())
                    .build()
                {
                    Ok(client) => match client
                        .post(ENDPOINT)
                        .header("Content-Type", "application/json")
                        .body(batch.body)
                        .send()
                        .await
                    {
                        Ok(resp) => match resp.status().as_u16() {
                            204 => SendResult::Accepted,
                            400 | 413 => SendResult::Rejected,
                            _ => SendResult::Retry,
                        },
                        Err(_) => SendResult::Retry,
                    },
                    Err(_) => SendResult::Retry,
                };
                diag.finish_batch(&batch.id, result, Utc::now());
            }

            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    });
}
