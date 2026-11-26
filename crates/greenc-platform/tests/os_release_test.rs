use greenc_platform::{classify_windows, os_release};

#[test]
fn classify_windows_splits_at_build_22000() {
    assert_eq!(classify_windows(0), "windows10");
    assert_eq!(classify_windows(19045), "windows10");
    assert_eq!(classify_windows(21999), "windows10");
    assert_eq!(classify_windows(22000), "windows11");
    assert_eq!(classify_windows(22631), "windows11");
    assert_eq!(classify_windows(26100), "windows11");
}

#[test]
fn os_release_returns_valid_contract_os_and_build() {
    let (os, build) = os_release();
    assert!(
        matches!(os, "windows10" | "windows11" | "macos" | "linux"),
        "unexpected os: {os}"
    );
    #[cfg(target_os = "linux")]
    {
        assert_eq!(os, "linux");
        assert_eq!(build, 0);
    }
    #[cfg(target_os = "macos")]
    {
        assert_eq!(os, "macos");
        assert!(build > 0);
    }
    #[cfg(target_os = "windows")]
    {
        assert_eq!(os, classify_windows(build));
    }
}
