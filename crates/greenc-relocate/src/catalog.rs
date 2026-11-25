//! Application catalog, developer tool configs, and advanced user folder definitions.

use greenc_core::models::RelocationStrategyKind;
use std::env;
use std::path::PathBuf;

pub const ADVANCED_THRESHOLD_BYTES: u64 = 200 * 1024 * 1024; // 200 MiB

/// Protected system directory names that must NEVER be relocated via advanced scan.
pub const ADVANCED_SKIP_LIST: &[&str] = &[
    "microsoft",
    "windows",
    "packages",
    "temp",
    "history",
    "system",
    "system32",
    "virtualstore",
    "crashdumps",
    "google",
    "mozilla",
    "edge",
    "onedrive",
    "assembly",
    "application data",
    "cryptneturlcache",
    "connecteddevicesplatform",
    "windowsapps",
    "greenc",
    "greenc-store",
    "zalo",
];

#[derive(Debug, Clone)]
pub struct AppCatalogProfile {
    pub app_id: String,
    pub name: String,
    pub strategy: RelocationStrategyKind,
    pub source_path: PathBuf,
    pub process_names: Vec<String>,
    pub how_description: String,
}

#[derive(Debug, Clone)]
pub struct ToolConfigProfile {
    pub tool_id: String,
    pub name: String,
    pub source_path: PathBuf,
    pub config_command: String,
    pub how_description: String,
}

/// Retrieves standard known desktop app profiles (OS-aware: Windows, macOS, Linux).
pub fn get_app_catalog() -> Vec<AppCatalogProfile> {
    #[cfg(windows)]
    {
        get_app_catalog_windows()
    }
    #[cfg(target_os = "macos")]
    {
        get_app_catalog_macos()
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        get_app_catalog_linux()
    }
}

#[cfg(windows)]
fn get_app_catalog_windows() -> Vec<AppCatalogProfile> {
    let mut catalog = Vec::new();
    let appdata = env::var("APPDATA").map(PathBuf::from).unwrap_or_default();
    let local_appdata = env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_default();

    // 1. Telegram Desktop
    catalog.push(AppCatalogProfile {
        app_id: "telegram".to_string(),
        name: "Telegram Desktop".to_string(),
        strategy: RelocationStrategyKind::Junction,
        source_path: appdata.join("Telegram Desktop"),
        process_names: vec!["telegram.exe".to_string()],
        how_description: "Liên kết dữ liệu chat và media bằng NTFS Junction".to_string(),
    });

    // 2. Discord
    catalog.push(AppCatalogProfile {
        app_id: "discord".to_string(),
        name: "Discord".to_string(),
        strategy: RelocationStrategyKind::Junction,
        source_path: appdata.join("discord"),
        process_names: vec!["discord.exe".to_string()],
        how_description: "Liên kết bộ nhớ đệm và dữ liệu ứng dụng Discord".to_string(),
    });

    // 3. Slack
    catalog.push(AppCatalogProfile {
        app_id: "slack".to_string(),
        name: "Slack".to_string(),
        strategy: RelocationStrategyKind::Junction,
        source_path: appdata.join("Slack"),
        process_names: vec!["slack.exe".to_string()],
        how_description: "Liên kết dữ liệu không gian làm việc Slack".to_string(),
    });

    // 4. CapCut
    catalog.push(AppCatalogProfile {
        app_id: "capcut".to_string(),
        name: "CapCut Video Editor".to_string(),
        strategy: RelocationStrategyKind::Junction,
        source_path: local_appdata.join("CapCut"),
        process_names: vec!["capcut.exe".to_string()],
        how_description: "Liên kết thư viện dự án, hiệu ứng và bản nháp video".to_string(),
    });

    // 5. JetBrains IDEs
    catalog.push(AppCatalogProfile {
        app_id: "jetbrains".to_string(),
        name: "JetBrains IDEs Data".to_string(),
        strategy: RelocationStrategyKind::Junction,
        source_path: appdata.join("JetBrains"),
        process_names: vec![
            "idea64.exe".to_string(),
            "clion64.exe".to_string(),
            "pycharm64.exe".to_string(),
            "webstorm64.exe".to_string(),
            "rider64.exe".to_string(),
        ],
        how_description: "Liên kết cấu hình và chỉ mục mã nguồn JetBrains".to_string(),
    });

    // 6. Zalo PC: automatic NTFS Junction move for `<ROOT>\Zalo Data` on system drive
    //    (standard desktop builds); falls back to Guided when config is missing or on Store builds.
    let zalo_opts = greenc_scanner::zalo::ZaloDiscoveryOptions::default();
    let zalo_disc = greenc_scanner::zalo::discover_zalo_installation(&zalo_opts);
    let (zalo_strategy, zalo_source, zalo_how) = match zalo_disc {
        Some(ref d)
            if d.data_dir.is_some()
                && d.data_on_system_drive
                && !d.data_is_known_relocation
                && crate::zalo_move::is_zalo_auto_move_supported() =>
        {
            (
                RelocationStrategyKind::Junction,
                d.data_dir.clone().unwrap_or_default(),
                "Đóng Zalo an toàn, chuyển dữ liệu chat sang ổ khác bằng liên kết thư mục, rồi mở lại Zalo".to_string(),
            )
        }
        Some(ref d) if d.data_dir.is_some() => (
            RelocationStrategyKind::Guided,
            d.data_dir.clone().unwrap_or_default(),
            "Hướng dẫn đổi thư mục lưu trữ file trong Cài đặt Zalo".to_string(),
        ),
        _ => (
            RelocationStrategyKind::Guided,
            local_appdata.join("Programs").join("Zalo"),
            "Hướng dẫn đổi thư mục lưu trữ file trong Cài đặt Zalo".to_string(),
        ),
    };
    catalog.push(AppCatalogProfile {
        app_id: "zalo".to_string(),
        name: "Zalo PC".to_string(),
        strategy: zalo_strategy,
        source_path: zalo_source,
        process_names: vec![
            "zalo.exe".to_string(),
            "zalocall.exe".to_string(),
            "zalocap.exe".to_string(),
        ],
        how_description: zalo_how,
    });

    // 7. Guided Apps: WSL2
    catalog.push(AppCatalogProfile {
        app_id: "wsl2".to_string(),
        name: "WSL2 Virtual Distros".to_string(),
        strategy: RelocationStrategyKind::Guided,
        source_path: local_appdata.join("Packages"),
        process_names: vec![
            "wsl.exe".to_string(),
            "wslhost.exe".to_string(),
            "vmmem.exe".to_string(),
            "vmmemwsl.exe".to_string(),
            "vmwp.exe".to_string(),
        ],
        how_description: "Hướng dẫn xuất và nhập phân vùng Linux sang ổ đĩa khác".to_string(),
    });

    // 8. Guided Apps: Docker Desktop
    catalog.push(AppCatalogProfile {
        app_id: "docker".to_string(),
        name: "Docker Desktop Containers".to_string(),
        strategy: RelocationStrategyKind::Guided,
        source_path: local_appdata.join("Docker").join("wsl"),
        process_names: vec![
            "docker.exe".to_string(),
            "docker desktop.exe".to_string(),
            "com.docker.backend.exe".to_string(),
            "com.docker.proxy.exe".to_string(),
        ],
        how_description: "Hướng dẫn di chuyển ổ đĩa ảo chứa container và images".to_string(),
    });

    catalog
}

#[cfg(target_os = "macos")]
fn get_app_catalog_macos() -> Vec<AppCatalogProfile> {
    let home = env::var("HOME").map(PathBuf::from).unwrap_or_default();
    let app_support = home.join("Library").join("Application Support");
    let dev = home.join("Library").join("Developer");

    vec![
        AppCatalogProfile {
            app_id: "telegram".to_string(),
            name: "Telegram Desktop".to_string(),
            strategy: RelocationStrategyKind::Junction,
            source_path: app_support.join("Telegram Desktop"),
            process_names: vec!["Telegram".to_string()],
            how_description: "Liên kết dữ liệu chat và media bằng Symlink".to_string(),
        },
        AppCatalogProfile {
            app_id: "discord".to_string(),
            name: "Discord".to_string(),
            strategy: RelocationStrategyKind::Junction,
            source_path: app_support.join("discord"),
            process_names: vec!["Discord".to_string()],
            how_description: "Liên kết bộ nhớ đệm và dữ liệu ứng dụng Discord bằng Symlink"
                .to_string(),
        },
        AppCatalogProfile {
            app_id: "slack".to_string(),
            name: "Slack".to_string(),
            strategy: RelocationStrategyKind::Junction,
            source_path: app_support.join("Slack"),
            process_names: vec!["Slack".to_string()],
            how_description: "Liên kết dữ liệu không gian làm việc Slack bằng Symlink".to_string(),
        },
        AppCatalogProfile {
            app_id: "jetbrains".to_string(),
            name: "JetBrains IDEs Data".to_string(),
            strategy: RelocationStrategyKind::Junction,
            source_path: app_support.join("JetBrains"),
            process_names: vec![
                "idea".to_string(),
                "pycharm".to_string(),
                "webstorm".to_string(),
            ],
            how_description: "Liên kết cấu hình và chỉ mục mã nguồn JetBrains bằng Symlink"
                .to_string(),
        },
        AppCatalogProfile {
            app_id: "xcode_derived".to_string(),
            name: "Xcode DerivedData".to_string(),
            strategy: RelocationStrategyKind::Junction,
            source_path: dev.join("Xcode").join("DerivedData"),
            process_names: vec!["Xcode".to_string()],
            how_description: "Di chuyển Xcode DerivedData sang ổ đĩa ngoài bằng Symlink"
                .to_string(),
        },
        AppCatalogProfile {
            app_id: "docker".to_string(),
            name: "Docker Desktop Containers".to_string(),
            strategy: RelocationStrategyKind::Guided,
            source_path: home.join("Library/Containers/com.docker.docker/Data"),
            process_names: vec!["Docker".to_string(), "com.docker.backend".to_string()],
            how_description:
                "Đổi vị trí Disk image location trong Docker Desktop Settings → Resources"
                    .to_string(),
        },
    ]
}

#[cfg(all(unix, not(target_os = "macos")))]
fn get_app_catalog_linux() -> Vec<AppCatalogProfile> {
    let home = env::var("HOME").map(PathBuf::from).unwrap_or_default();
    let config = env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home.join(".config"));
    let data = env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home.join(".local/share"));

    vec![
        AppCatalogProfile {
            app_id: "telegram".to_string(),
            name: "Telegram Desktop".to_string(),
            strategy: RelocationStrategyKind::Junction,
            source_path: data.join("TelegramDesktop"),
            process_names: vec!["telegram-desktop".to_string()],
            how_description: "Liên kết dữ liệu chat và media bằng Symlink".to_string(),
        },
        AppCatalogProfile {
            app_id: "discord".to_string(),
            name: "Discord".to_string(),
            strategy: RelocationStrategyKind::Junction,
            source_path: config.join("discord"),
            process_names: vec!["Discord".to_string()],
            how_description: "Liên kết bộ nhớ đệm và dữ liệu ứng dụng Discord bằng Symlink"
                .to_string(),
        },
        AppCatalogProfile {
            app_id: "slack".to_string(),
            name: "Slack".to_string(),
            strategy: RelocationStrategyKind::Junction,
            source_path: config.join("Slack"),
            process_names: vec!["slack".to_string()],
            how_description: "Liên kết dữ liệu không gian làm việc Slack bằng Symlink".to_string(),
        },
        AppCatalogProfile {
            app_id: "jetbrains".to_string(),
            name: "JetBrains IDEs Data".to_string(),
            strategy: RelocationStrategyKind::Junction,
            source_path: config.join("JetBrains"),
            process_names: vec![
                "idea".to_string(),
                "pycharm".to_string(),
                "webstorm".to_string(),
            ],
            how_description: "Liên kết cấu hình và chỉ mục mã nguồn JetBrains bằng Symlink"
                .to_string(),
        },
        AppCatalogProfile {
            app_id: "docker".to_string(),
            name: "Docker Data Root".to_string(),
            strategy: RelocationStrategyKind::Guided,
            source_path: PathBuf::from("/var/lib/docker"),
            process_names: vec!["dockerd".to_string()],
            how_description:
                "Cấu hình `data-root` trong `/etc/docker/daemon.json` sang phân vùng dữ liệu"
                    .to_string(),
        },
    ]
}

/// Retrieves developer tool config cache profiles (OS-aware: Windows, macOS, Linux).
pub fn get_tool_catalog() -> Vec<ToolConfigProfile> {
    let mut catalog = Vec::new();
    #[cfg(windows)]
    let user_profile = env::var("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_default();
    #[cfg(not(windows))]
    let user_profile = env::var("HOME").map(PathBuf::from).unwrap_or_default();

    #[cfg(windows)]
    let local_appdata = env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_default();
    #[cfg(target_os = "macos")]
    let local_appdata = user_profile.join("Library").join("Caches");
    #[cfg(all(unix, not(target_os = "macos")))]
    let local_appdata = env::var("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| user_profile.join(".cache"));

    // 1. npm
    catalog.push(ToolConfigProfile {
        tool_id: "npm".to_string(),
        name: "npm Cache".to_string(),
        source_path: user_profile.join(".npm"),
        config_command: "npm config set cache".to_string(),
        how_description: "Cấu hình lại đường dẫn bộ nhớ đệm gói Node.js qua lệnh npm".to_string(),
    });

    // 2. pip
    catalog.push(ToolConfigProfile {
        tool_id: "pip".to_string(),
        name: "pip Python Cache".to_string(),
        source_path: local_appdata.join("pip").join("cache"),
        config_command: "pip config set global.cache-dir".to_string(),
        how_description: "Cấu hình thư mục bộ nhớ đệm wheel của pip Python".to_string(),
    });

    // 3. Cargo (Rust)
    catalog.push(ToolConfigProfile {
        tool_id: "cargo".to_string(),
        name: "Cargo Rust Crates".to_string(),
        source_path: user_profile.join(".cargo"),
        config_command: "setx CARGO_HOME".to_string(),
        how_description: "Thiết lập biến môi trường CARGO_HOME sang ổ đĩa đích".to_string(),
    });

    // 4. uv (Python Package Manager)
    catalog.push(ToolConfigProfile {
        tool_id: "uv".to_string(),
        name: "uv Fast Python Cache".to_string(),
        source_path: local_appdata.join("uv"),
        config_command: "setx UV_CACHE_DIR".to_string(),
        how_description: "Thiết lập biến môi trường UV_CACHE_DIR lưu trữ wheel cache".to_string(),
    });

    // 5. pnpm
    catalog.push(ToolConfigProfile {
        tool_id: "pnpm".to_string(),
        name: "pnpm Global Store".to_string(),
        source_path: local_appdata.join("pnpm").join("store"),
        config_command: "pnpm config set store-dir".to_string(),
        how_description: "Chuyển kho lưu trữ dùng chung pnpm store sang ổ đĩa dữ liệu".to_string(),
    });

    // 6. Yarn
    catalog.push(ToolConfigProfile {
        tool_id: "yarn".to_string(),
        name: "Yarn Cache".to_string(),
        source_path: local_appdata.join("Yarn").join("Cache"),
        config_command: "yarn config set cache-folder".to_string(),
        how_description: "Định tuyến thư mục lưu trữ gói của Yarn".to_string(),
    });

    // 7. NuGet
    catalog.push(ToolConfigProfile {
        tool_id: "nuget".to_string(),
        name: "NuGet .NET Cache".to_string(),
        source_path: user_profile.join(".nuget").join("packages"),
        config_command: "nuget config -set repositoryPath".to_string(),
        how_description: "Cấu hình kho gói .NET NuGet sang ổ đĩa khác".to_string(),
    });

    // 8. Gradle
    catalog.push(ToolConfigProfile {
        tool_id: "gradle".to_string(),
        name: "Gradle Cache".to_string(),
        source_path: user_profile.join(".gradle").join("caches"),
        config_command: "setx GRADLE_USER_HOME".to_string(),
        how_description: "Chuyển toàn bộ GRADLE_USER_HOME sang ổ đĩa dữ liệu".to_string(),
    });

    // 9. Ollama Models
    catalog.push(ToolConfigProfile {
        tool_id: "ollama".to_string(),
        name: "Ollama LLM Models".to_string(),
        source_path: user_profile.join(".ollama").join("models"),
        config_command: "setx OLLAMA_MODELS".to_string(),
        how_description: "Chuyển kho mô hình LLM AI (GGUF) sang ổ đĩa dung lượng lớn".to_string(),
    });

    // 10. HuggingFace Cache
    catalog.push(ToolConfigProfile {
        tool_id: "huggingface".to_string(),
        name: "Hugging Face Cache".to_string(),
        source_path: user_profile.join(".cache").join("huggingface"),
        config_command: "setx HF_HOME".to_string(),
        how_description: "Thiết lập biến môi trường HF_HOME lưu trữ weights và datasets"
            .to_string(),
    });

    catalog
}
