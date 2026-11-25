//! Read-only large build-output suggester (`GC-GUARD` Item 6: `greenc-cli guard suggest`).
//!
//! Strictly READ-ONLY (never modifies or deletes files). Discovers regenerable, git-ignored
//! build output directories (`build`, `target`, `.gradle`, `node_modules`) inside git
//! repositories and worktrees that have been idle for >= 3 days while their associated build tool
//! is not running, alongside current Canary Vault disk usage.
//!
//! Safety bounds:
//! - Bounded directory traversal depth (`max_depth`, default 6).
//! - Bounded wall-clock scan duration (`max_duration`, default 10s).
//! - Never follows symlinks.
//! - Never crosses filesystem / drive boundaries (`st_dev` on Unix, volume prefix on Windows).

use crate::cache_units::effective_last_used;
use greenc_core::models::is_same_volume;
use greenc_core::task::TaskControl;
use greenc_platform::dev_tools::{BusyToolsStatus, DevTool, DevToolProbe};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// Default minimum idle age (3 days) for build-output suggestions.
pub const DEFAULT_SUGGEST_MIN_STALE_DAYS: u64 = 3;
/// Default maximum directory traversal depth.
pub const DEFAULT_SUGGEST_MAX_DEPTH: usize = 6;
/// Default wall-clock scan timeout.
pub const DEFAULT_SUGGEST_MAX_DURATION_SECS: u64 = 10;

/// Configuration for [`suggest_large_build_outputs`].
#[derive(Debug, Clone)]
pub struct SuggestOptions {
    pub roots: Vec<PathBuf>,
    pub drive_filter: Option<PathBuf>,
    pub vault_dir: PathBuf,
    pub min_stale_days: u64,
    pub max_depth: usize,
    pub max_duration: Duration,
    pub now: SystemTime,
}

impl SuggestOptions {
    pub fn with_defaults(roots: Vec<PathBuf>, vault_dir: PathBuf) -> Self {
        Self {
            roots,
            drive_filter: None,
            vault_dir,
            min_stale_days: DEFAULT_SUGGEST_MIN_STALE_DAYS,
            max_depth: DEFAULT_SUGGEST_MAX_DEPTH,
            max_duration: Duration::from_secs(DEFAULT_SUGGEST_MAX_DURATION_SECS),
            now: SystemTime::now(),
        }
    }
}

/// One read-only suggestion for a stale git-ignored build output directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildOutputSuggestion {
    pub path: PathBuf,
    pub repo_root: PathBuf,
    pub kind: String,
    pub size_bytes: u64,
    pub stale_days: u64,
    pub git_ignored: bool,
    pub associated_tool: String,
}

/// Summary of Canary Vault disk footprint.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct VaultSpaceSummary {
    pub path: PathBuf,
    pub total_bytes: u64,
    pub archive_count: usize,
}

/// Complete read-only suggestion report returned by `guard suggest`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuggestReport {
    pub suggestions: Vec<BuildOutputSuggestion>,
    pub total_suggestable_bytes: u64,
    pub vault: VaultSpaceSummary,
    pub truncated_by_time_limit: bool,
    pub busy_tools: Vec<String>,
    pub busy_tools_unknown: bool,
}

#[cfg(unix)]
fn device_id(meta: &std::fs::Metadata) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    Some(meta.dev())
}

#[cfg(not(unix))]
fn device_id(_meta: &std::fs::Metadata) -> Option<u64> {
    None
}

fn is_build_output_dir_name(name: &str) -> Option<(&'static str, DevTool)> {
    match name {
        "build" => Some(("build", DevTool::Gradle)),
        "target" => Some(("target", DevTool::Cargo)),
        ".gradle" => Some((".gradle", DevTool::Gradle)),
        "node_modules" => Some(("node_modules", DevTool::Npm)),
        _ => None,
    }
}

fn should_skip_walk_dir(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | ".hg"
            | ".svn"
            | ".cache"
            | ".local"
            | ".cargo"
            | ".rustup"
            | ".npm"
            | ".nvm"
            | ".pyenv"
            | ".venv"
            | "venv"
            | "__pycache__"
            | "proc"
            | "sys"
            | "dev"
            | "run"
    )
}

/// Checks whether `candidate` inside `repo_root` is ignored by git.
///
/// Uses `git -C <repo_root> check-ignore -q <rel_path>` when `git` is available, falling back
/// to inspecting `.gitignore` entries when `git` is unavailable.
fn is_git_ignored(repo_root: &Path, candidate: &Path) -> bool {
    let Ok(rel) = candidate.strip_prefix(repo_root) else {
        return false;
    };
    let rel_str = rel.to_string_lossy().replace('\\', "/");
    if rel_str.is_empty() {
        return false;
    }

    if let Ok(output) = std::process::Command::new("git")
        .stdin(std::process::Stdio::null())
        .arg("-C")
        .arg(repo_root)
        .args(["check-ignore", "-q", &rel_str])
        .output()
    {
        if output.status.success() {
            return true;
        }
        // Exit code 1 means definitively NOT ignored by git.
        if output.status.code() == Some(1) {
            return false;
        }
    }

    // Fallback when git binary is unavailable or repo metadata is minimal: check .gitignore files
    // along ancestors from `candidate` up to `repo_root`.
    let candidate_name = candidate
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut cur = candidate.parent();
    while let Some(dir) = cur {
        let gi = dir.join(".gitignore");
        if let Ok(text) = std::fs::read_to_string(&gi) {
            for line in text.lines() {
                let pat = line.trim();
                if pat.is_empty() || pat.starts_with('#') || pat.starts_with('!') {
                    continue;
                }
                let cleaned = pat
                    .trim_start_matches("**/")
                    .trim_start_matches('/')
                    .trim_end_matches('/');
                if cleaned == candidate_name || cleaned == rel_str {
                    return true;
                }
            }
        }
        if dir == repo_root {
            break;
        }
        cur = dir.parent();
    }
    false
}

/// Measures directory total bytes and newest `max(mtime, reliable_atime)` for files (and `mtime` for dirs)
/// without following symlinks or crossing devices, bounded by `deadline`.
fn measure_dir_size_and_latest(
    dir: &Path,
    expected_dev: Option<u64>,
    now: SystemTime,
    deadline: Instant,
    ctrl: &TaskControl,
) -> Option<(u64, SystemTime, bool)> {
    let root_meta = std::fs::symlink_metadata(dir).ok()?;
    if root_meta.file_type().is_symlink() || !root_meta.is_dir() {
        return None;
    }
    if expected_dev.is_some() && device_id(&root_meta) != expected_dev {
        return None;
    }

    let mut total_bytes: u64 = 0;
    let mut newest = root_meta.modified().unwrap_or(now);
    let mut stack: Vec<(PathBuf, usize)> = vec![(dir.to_path_buf(), 0)];

    while let Some((cur, depth)) = stack.pop() {
        if ctrl.is_cancelled() || Instant::now() >= deadline {
            return Some((total_bytes, newest, true));
        }
        if depth > 24 {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(&cur) else {
            continue;
        };
        for entry in rd.flatten() {
            if Instant::now() >= deadline {
                return Some((total_bytes, newest, true));
            }
            let p = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&p) else {
                continue;
            };
            if meta.file_type().is_symlink() {
                continue;
            }
            if expected_dev.is_some() && device_id(&meta) != expected_dev {
                continue;
            }
            if meta.is_dir() {
                let dm = meta.modified().unwrap_or(now);
                if dm > newest {
                    newest = dm;
                }
                stack.push((p, depth + 1));
            } else if meta.is_file() {
                total_bytes = total_bytes.saturating_add(meta.len());
                let used = effective_last_used(&meta, now);
                if used > newest {
                    newest = used;
                }
            }
        }
    }

    Some((total_bytes, newest, false))
}

/// Measures Canary Vault footprint (non-recursive + 1 level safe, no symlinks).
pub fn measure_vault_summary(vault_dir: &Path) -> VaultSpaceSummary {
    let mut summary = VaultSpaceSummary {
        path: vault_dir.to_path_buf(),
        total_bytes: 0,
        archive_count: 0,
    };
    let Ok(rd) = std::fs::read_dir(vault_dir) else {
        return summary;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&p) else {
            continue;
        };
        if meta.file_type().is_symlink() || !meta.is_file() {
            continue;
        }
        summary.total_bytes = summary.total_bytes.saturating_add(meta.len());
        if p.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("zip"))
        {
            summary.archive_count += 1;
        }
    }
    summary
}

/// Scans `opts.roots` in read-only mode to suggest large stale git-ignored build output directories
/// and report Vault usage.
pub fn suggest_large_build_outputs(
    opts: &SuggestOptions,
    dev_tool_probe: &dyn DevToolProbe,
    ctrl: &TaskControl,
) -> SuggestReport {
    let busy_status = dev_tool_probe.detect_busy_tools();
    suggest_large_build_outputs_with_busy(opts, &busy_status, ctrl)
}

/// Same as [`suggest_large_build_outputs`] with a pre-probed [`BusyToolsStatus`].
pub fn suggest_large_build_outputs_with_busy(
    opts: &SuggestOptions,
    busy_status: &BusyToolsStatus,
    ctrl: &TaskControl,
) -> SuggestReport {
    let deadline = Instant::now() + opts.max_duration;
    let min_age = Duration::from_secs(opts.min_stale_days.saturating_mul(86_400));
    let vault = measure_vault_summary(&opts.vault_dir);

    let mut suggestions = Vec::new();
    let mut seen_paths = HashSet::new();
    let mut truncated = false;

    for root in &opts.roots {
        if ctrl.is_cancelled() || Instant::now() >= deadline {
            truncated = true;
            break;
        }
        let Ok(root_meta) = std::fs::symlink_metadata(root) else {
            continue;
        };
        if root_meta.file_type().is_symlink() || !root_meta.is_dir() {
            continue;
        }
        if let Some(ref filter) = opts.drive_filter {
            if !is_same_volume(root, filter) {
                continue;
            }
        }
        let root_dev = device_id(&root_meta);

        // Stack items: (dir_path, depth, active_git_repo_root)
        let initial_repo = if root.join(".git").exists() {
            Some(root.clone())
        } else {
            None
        };
        let mut stack: Vec<(PathBuf, usize, Option<PathBuf>)> =
            vec![(root.clone(), 0, initial_repo)];

        while let Some((dir, depth, repo_root)) = stack.pop() {
            if ctrl.is_cancelled() || Instant::now() >= deadline {
                truncated = true;
                break;
            }
            if depth > opts.max_depth {
                continue;
            }
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };

            for entry in rd.flatten() {
                if ctrl.is_cancelled() || Instant::now() >= deadline {
                    truncated = true;
                    break;
                }
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();
                let Ok(meta) = std::fs::symlink_metadata(&path) else {
                    continue;
                };
                if meta.file_type().is_symlink() || !meta.is_dir() {
                    continue;
                }
                if root_dev.is_some() && device_id(&meta) != root_dev {
                    continue;
                }
                if let Some(ref filter) = opts.drive_filter {
                    if !is_same_volume(&path, filter) {
                        continue;
                    }
                }

                // If inside a git repo/worktree and this directory is a regenerable build output:
                if let Some(ref repo) = repo_root {
                    if let Some((kind_str, default_tool)) = is_build_output_dir_name(&name) {
                        // Refine associated tool: `target` in a Rust repo -> Cargo; otherwise Gradle.
                        let tool = if kind_str == "target"
                            && !dir.join("Cargo.toml").exists()
                            && !repo.join("Cargo.toml").exists()
                        {
                            DevTool::Gradle
                        } else {
                            default_tool
                        };

                        if !busy_status.is_tool_busy(tool)
                            && is_git_ignored(repo, &path)
                            && seen_paths.insert(path.clone())
                        {
                            if let Some((size_bytes, newest, timed_out)) =
                                measure_dir_size_and_latest(
                                    &path, root_dev, opts.now, deadline, ctrl,
                                )
                            {
                                if timed_out {
                                    truncated = true;
                                }
                                let age = opts.now.duration_since(newest).unwrap_or(Duration::ZERO);
                                if size_bytes > 0 && age >= min_age {
                                    suggestions.push(BuildOutputSuggestion {
                                        path: path.clone(),
                                        repo_root: repo.clone(),
                                        kind: kind_str.to_string(),
                                        size_bytes,
                                        stale_days: age.as_secs() / 86_400,
                                        git_ignored: true,
                                        associated_tool: tool.as_str().to_string(),
                                    });
                                }
                            }
                        }
                        // Never descend inside build output directories (`node_modules`, `target`, `build`, `.gradle`).
                        continue;
                    }
                }

                if should_skip_walk_dir(&name) {
                    continue;
                }

                // Check if this subdirectory is itself a git repo or git worktree (`.git` dir OR file!).
                let child_repo = if path.join(".git").exists() {
                    Some(path.clone())
                } else {
                    repo_root.clone()
                };

                if depth < opts.max_depth {
                    stack.push((path, depth + 1, child_repo));
                }
            }
        }
    }

    suggestions.sort_by_key(|a| std::cmp::Reverse(a.size_bytes));
    let total_suggestable_bytes: u64 = suggestions.iter().map(|s| s.size_bytes).sum();

    SuggestReport {
        suggestions,
        total_suggestable_bytes,
        vault,
        truncated_by_time_limit: truncated,
        busy_tools: busy_status
            .busy_tools()
            .into_iter()
            .map(|t| t.as_str().to_string())
            .collect(),
        busy_tools_unknown: busy_status.is_unknown(),
    }
}
