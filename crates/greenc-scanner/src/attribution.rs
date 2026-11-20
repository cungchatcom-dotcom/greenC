//! Write reason classification (all 11 `WriteReasonCode` variants) and hybrid process/Windows
//! Service attribution engine combining Restart Manager, SCM PID maps, and path signatures (Tasks T053, T055).

use greenc_core::models::WriteReasonCode;
use greenc_win32::restart_mgr::{NativeRestartManagerApi, RestartManagerApi};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Localized Vietnamese source label key for each `WriteReasonCode`.
pub fn reason_label_key(code: WriteReasonCode) -> &'static str {
    match code {
        WriteReasonCode::MediaChat => {
            "Media & File đính kèm phát sinh khi chat (Zalo / DingTalk / Chat App)"
        }
        WriteReasonCode::EappBundle => {
            "Gói Mini-App / EApp tải về & giải nén lưu cữu (DingTalk EAppFiles)"
        }
        WriteReasonCode::CacheTemp => "Bộ nhớ đệm (Cache / Temp / Resource Cache / ShaderCache)",
        WriteReasonCode::OldAppVersion => {
            "Bản cài đặt phiên bản cũ bị bỏ lại sau khi tự cập nhật (Update Leftover)"
        }
        WriteReasonCode::CrashDump => "File đổ lỗi bộ nhớ (Crash Dump .dmp / WER)",
        WriteReasonCode::LogGrowth => {
            "File nhật ký hệ thống / ứng dụng phình to (.log / .etl / .calf)"
        }
        WriteReasonCode::DbStorage => "Cơ sở dữ liệu & Nhật ký giao dịch (.db / .wal / .sqlite)",
        WriteReasonCode::ContainerVm => "Ảnh đĩa ảo Container / Máy ảo (.vhdx / WSL / Docker)",
        WriteReasonCode::MediaDownload => "Tập tin tải về / Bộ cài / Video dung lượng lớn",
        WriteReasonCode::OrphanLeftover => "Dữ liệu mồ côi còn sót lại sau khi gỡ cài đặt ứng dụng",
        WriteReasonCode::AppData => "Dữ liệu phát sinh trong quá trình vận hành ứng dụng",
    }
}

/// Classifies a filesystem path into one of the 11 canonical `WriteReasonCode` buckets.
pub fn classify_write_reason(path: &Path) -> (WriteReasonCode, &'static str) {
    let normalized = path.to_string_lossy().replace('/', "\\");
    let lower = normalized.to_lowercase();
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
        .unwrap_or_default();

    let code = if lower.contains("eappfiles\\download")
        || lower.contains("eappfiles\\unziped")
        || ext == "amr"
    {
        WriteReasonCode::EappBundle
    } else if lower.contains("zalodownloads\\resource")
        || lower.contains("zalodownloads\\filenoise")
        || lower.contains("zalodownloads\\picture")
        || lower.contains("zalodownloads\\video")
        || lower.contains("zalo received files")
        || lower.contains("\\zalodata\\media")
        || lower.contains("\\zalo data\\media")
        || lower.contains("\\telegram desktop\\tdata")
        || lower.contains("\\imagefiles\\")
        || lower.contains("\\videofiles\\")
        || lower.contains("\\audiofiles\\")
    {
        WriteReasonCode::MediaChat
    } else if lower.contains("programs\\zalo\\zalo-")
        || lower.contains("zalo-updater")
        || lower.contains("zalo\\pending")
        || lower.contains("media\\update")
        || lower.contains("\\app-") && lower.contains("\\packages\\")
    {
        WriteReasonCode::OldAppVersion
    } else if matches!(ext.as_str(), "dmp" | "hdmp" | "mdmp")
        || lower.contains("\\dumps\\")
        || lower.contains("\\crashdumps\\")
        || lower.contains("\\wer\\")
    {
        WriteReasonCode::CrashDump
    } else if matches!(ext.as_str(), "log" | "etl" | "evtx" | "calf" | "trace")
        || lower.contains("\\logs\\")
        || lower.contains("\\log\\")
        || lower.contains("\\holmeslogs\\")
        || lower.contains("\\updaterlogs\\")
    {
        WriteReasonCode::LogGrowth
    } else if lower.contains("\\orphan")
        || lower.contains("\\uninstalled_remnant")
        || lower.contains(".orphaned")
    {
        WriteReasonCode::OrphanLeftover
    } else if lower.contains("\\cache\\")
        || lower.contains("\\code cache\\")
        || lower.contains("\\gpucache\\")
        || lower.contains("\\dawncache\\")
        || lower.contains("\\shadercache\\")
        || lower.contains("\\resource_cache\\")
        || lower.contains("\\_cacache\\")
        || lower.contains("\\.npm\\")
        || lower.contains("\\pip\\cache")
        || lower.contains("\\uv\\cache")
        || lower.contains("\\temp\\")
        || lower.contains("\\softwaredistribution\\download")
        || matches!(ext.as_str(), "tmp" | "temp" | "cache")
    {
        WriteReasonCode::CacheTemp
    } else if matches!(
        ext.as_str(),
        "db" | "sqlite" | "sqlite3" | "db-wal" | "db-shm" | "mdf" | "ldf" | "ldb" | "wal"
    ) || lower.contains("\\dbfiles\\")
        || lower.contains("\\database\\")
    {
        WriteReasonCode::DbStorage
    } else if matches!(ext.as_str(), "vhdx" | "vhd" | "vmdk" | "qcow2") {
        WriteReasonCode::ContainerVm
    } else if matches!(
        ext.as_str(),
        "iso" | "zip" | "7z" | "rar" | "mp4" | "mkv" | "mov" | "msi" | "exe"
    ) || lower.contains("\\downloads\\")
    {
        WriteReasonCode::MediaDownload
    } else {
        WriteReasonCode::AppData
    };

    (code, reason_label_key(code))
}

/// Resolved actor responsible for a disk write event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttributionMatch {
    pub process_name: String,
    pub pid: u32,
    pub service_name: String,
    pub service_display_name: String,
    pub reason_code: WriteReasonCode,
    pub reason_detail: String,
}

/// Resolves which process or Windows Service owns disk writes at a given path.
pub struct AttributionEngine {
    rm_api: Arc<dyn RestartManagerApi>,
    pid_to_service: HashMap<u32, (String, String)>,
}

impl std::fmt::Debug for AttributionEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AttributionEngine")
            .field("cached_services", &self.pid_to_service.len())
            .finish()
    }
}

impl Default for AttributionEngine {
    fn default() -> Self {
        Self::new(Arc::new(NativeRestartManagerApi))
    }
}

impl AttributionEngine {
    pub fn new(rm_api: Arc<dyn RestartManagerApi>) -> Self {
        let mut engine = Self {
            rm_api,
            pid_to_service: HashMap::new(),
        };
        engine.refresh_service_map();
        engine
    }

    /// Refreshes the cached `PID -> (service_short_name, display_name)` table from SCM.
    pub fn refresh_service_map(&mut self) {
        if let Ok(map) = self.rm_api.map_pids_to_services() {
            self.pid_to_service = map;
        }
    }

    /// Attributes `path` by querying Restart Manager first (when `probe_live_locks` is enabled)
    /// and falling back to deterministic Windows path signatures.
    pub fn attribute_path(&self, path: &Path, probe_live_locks: bool) -> AttributionMatch {
        let (reason_code, reason_label) = classify_write_reason(path);

        if probe_live_locks && path.exists() {
            if let Ok(locks) = self.rm_api.get_locking_processes(&[path]) {
                if let Some(first) = locks.into_iter().next() {
                    let (svc_short, svc_display) = if !first.service_short_name.is_empty() {
                        let disp = self
                            .pid_to_service
                            .get(&first.pid)
                            .map(|(_, d)| d.clone())
                            .unwrap_or_else(|| first.service_short_name.clone());
                        (first.service_short_name, disp)
                    } else if let Some((s, d)) = self.pid_to_service.get(&first.pid) {
                        (s.clone(), d.clone())
                    } else {
                        (String::new(), first.app_name)
                    };

                    return AttributionMatch {
                        process_name: first.process_name,
                        pid: first.pid,
                        service_name: svc_short,
                        service_display_name: svc_display,
                        reason_code,
                        reason_detail: reason_label.to_string(),
                    };
                }
            }
        }

        let norm = path.to_string_lossy().replace('/', "\\");
        let lower = norm.to_lowercase();

        if lower.contains("zalo") {
            return AttributionMatch {
                process_name: "Zalo.exe".to_string(),
                pid: 0,
                service_name: String::new(),
                service_display_name: "Zalo Desktop (Chat & Media)".to_string(),
                reason_code: if reason_code == WriteReasonCode::AppData {
                    WriteReasonCode::MediaChat
                } else {
                    reason_code
                },
                reason_detail: reason_label.to_string(),
            };
        }

        if lower.contains("dingtalk") {
            return AttributionMatch {
                process_name: "DingTalk.exe".to_string(),
                pid: 0,
                service_name: String::new(),
                service_display_name: "DingTalk Desktop (EApp & Media Cache)".to_string(),
                reason_code,
                reason_detail: reason_label.to_string(),
            };
        }

        if lower.contains("\\telegram desktop") {
            return AttributionMatch {
                process_name: "Telegram.exe".to_string(),
                pid: 0,
                service_name: String::new(),
                service_display_name: "Telegram Desktop".to_string(),
                reason_code,
                reason_detail: reason_label.to_string(),
            };
        }

        if lower.contains("\\google\\chrome\\") {
            return AttributionMatch {
                process_name: "chrome.exe".to_string(),
                pid: 0,
                service_name: String::new(),
                service_display_name: "Google Chrome Browser".to_string(),
                reason_code,
                reason_detail: reason_label.to_string(),
            };
        }

        if lower.contains("\\microsoft\\edge\\") {
            return AttributionMatch {
                process_name: "msedge.exe".to_string(),
                pid: 0,
                service_name: String::new(),
                service_display_name: "Microsoft Edge Browser".to_string(),
                reason_code,
                reason_detail: reason_label.to_string(),
            };
        }

        if lower.contains("\\windows\\softwaredistribution\\")
            || lower.contains("\\windows\\logs\\cbs\\")
        {
            return AttributionMatch {
                process_name: "svchost.exe (wuauserv)".to_string(),
                pid: 0,
                service_name: "wuauserv".to_string(),
                service_display_name: "Windows Update Service".to_string(),
                reason_code,
                reason_detail: reason_label.to_string(),
            };
        }

        if lower.contains("\\docker\\") || lower.contains("docker-desktop") {
            return AttributionMatch {
                process_name: "com.docker.backend.exe".to_string(),
                pid: 0,
                service_name: "com.docker.service".to_string(),
                service_display_name: "Docker Desktop Engine".to_string(),
                reason_code,
                reason_detail: reason_label.to_string(),
            };
        }

        if lower.contains("\\windows\\temp\\") || lower.contains("\\appdata\\local\\temp\\") {
            return AttributionMatch {
                process_name: "Windows Temp / App Installer".to_string(),
                pid: 0,
                service_name: "System/Temp".to_string(),
                service_display_name: "Bộ nhớ tạm hệ thống & trình cài đặt".to_string(),
                reason_code,
                reason_detail: reason_label.to_string(),
            };
        }

        let components: Vec<String> = PathBuf::from(&norm)
            .components()
            .map(|c| c.as_os_str().to_string_lossy().to_string())
            .collect();

        for (idx, part) in components.iter().enumerate() {
            let p_low = part.to_lowercase();
            if matches!(
                p_low.as_str(),
                "roaming" | "local" | "locallow" | "programdata" | "programs"
            ) && idx + 1 < components.len()
            {
                let app_folder = &components[idx + 1];
                return AttributionMatch {
                    process_name: format!("{app_folder}.exe"),
                    pid: 0,
                    service_name: String::new(),
                    service_display_name: format!("Ứng dụng {app_folder}"),
                    reason_code,
                    reason_detail: reason_label.to_string(),
                };
            }
        }

        let fallback_folder = components.get(1).cloned().unwrap_or_else(|| norm.clone());
        AttributionMatch {
            process_name: format!("{fallback_folder} (User/App)"),
            pid: 0,
            service_name: String::new(),
            service_display_name: format!("Thư mục {fallback_folder}"),
            reason_code,
            reason_detail: reason_label.to_string(),
        }
    }
}
