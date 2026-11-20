fn main() {
    tauri_build::build();

    // Link the generated Windows Common-Controls v6 manifest into integration test binaries
    // as well (`tauri`/`wry` imports `comctl32.dll!TaskDialogIndirect`, which requires v6).
    if std::env::var_os("CARGO_CFG_TARGET_OS").as_deref() == Some(std::ffi::OsStr::new("windows")) {
        if let Some(out_dir) = std::env::var_os("OUT_DIR") {
            let out_path = std::path::PathBuf::from(out_dir);
            for candidate in ["libresource.a", "resource.lib"] {
                let res = out_path.join(candidate);
                if res.exists() {
                    println!("cargo:rustc-link-arg-tests={}", res.display());
                    break;
                }
            }
        }
    }
}
