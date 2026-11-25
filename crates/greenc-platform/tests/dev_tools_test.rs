//! Unit tests for pure developer-tool detection and BusyToolsStatus fail-safe semantics.

use greenc_platform::dev_tools::{
    detect_busy_dev_tools, parse_ps_output, BusyToolsStatus, DevTool, DevToolProbe,
    MockDevToolProbe, ProcessSnapshot,
};

#[test]
fn idle_gradle_daemon_and_workers_are_detected_as_busy() {
    let procs = vec![
        ProcessSnapshot::new(
            1001,
            "java",
            "/usr/lib/jvm/java-17/bin/java -Xmx4g -cp gradle-launcher.jar org.gradle.launcher.daemon.bootstrap.GradleDaemon 8.11.1",
        ),
        ProcessSnapshot::new(1002, "bash", "/bin/bash"),
    ];
    let busy = detect_busy_dev_tools(&procs);
    assert!(busy.contains(&DevTool::Gradle));
    assert_eq!(busy.len(), 1);

    // Worker, Wrapper, and KotlinCompileDaemon must also map to Gradle.
    for sig in [
        "org.gradle.wrapper.GradleWrapperMain assembleDebug",
        "org.gradle.process.internal.worker.GradleWorkerMain 'Gradle Worker Daemon 1'",
        "org.jetbrains.kotlin.daemon.KotlinCompileDaemon --daemon-runFilesPath /tmp",
    ] {
        let p = vec![ProcessSnapshot::new(2000, "java", format!("java {sig}"))];
        let b = detect_busy_dev_tools(&p);
        assert!(b.contains(&DevTool::Gradle), "failed to match {sig}");
    }
}

#[test]
fn detects_cargo_rustc_npm_pnpm_yarn_pip_uv_and_node_npm_cli() {
    let procs = vec![
        ProcessSnapshot::new(10, "cargo", "cargo test --workspace"),
        ProcessSnapshot::new(11, "rustc", "rustc --crate-name greenc_core"),
        ProcessSnapshot::new(
            12,
            "node",
            "/usr/bin/node /usr/lib/node_modules/npm/bin/npm-cli.js install",
        ),
        ProcessSnapshot::new(13, "python3", "/usr/bin/python3 -m pip install requests"),
        ProcessSnapshot::new(14, "uv", "uv sync"),
    ];
    let busy = detect_busy_dev_tools(&procs);
    assert!(busy.contains(&DevTool::Cargo));
    assert!(busy.contains(&DevTool::Npm));
    assert!(busy.contains(&DevTool::Pip));
    assert!(!busy.contains(&DevTool::Gradle));

    // pnpm / yarn direct and via node
    let js_procs = vec![
        ProcessSnapshot::new(20, "pnpm", "pnpm build"),
        ProcessSnapshot::new(21, "node", "node /opt/yarn/bin/yarn.js install"),
    ];
    assert!(detect_busy_dev_tools(&js_procs).contains(&DevTool::Npm));
}

#[test]
fn avoids_false_positives_on_unrelated_or_grep_processes() {
    let procs = vec![
        ProcessSnapshot::new(100, "grep", "grep -rn GradleDaemon /var/log"),
        ProcessSnapshot::new(101, "rg", "rg npm-cli.js src/"),
        ProcessSnapshot::new(102, "systemd", "/sbin/init"),
        ProcessSnapshot::new(103, "greenc-cli", "greenc-cli guard run --json"),
        ProcessSnapshot::new(104, "node", "node /app/server.js"),
        ProcessSnapshot::new(105, "python3", "python3 /app/manage.py runserver"),
    ];
    let busy = detect_busy_dev_tools(&procs);
    assert!(busy.is_empty(), "unexpected busy tools: {busy:?}");
}

#[test]
fn unknown_status_is_fail_safe_for_every_dev_tool() {
    let status = BusyToolsStatus::unknown("no /proc access");
    assert!(status.is_unknown());
    assert!(status.is_any_busy());
    for tool in DevTool::ALL {
        assert!(status.is_tool_busy(tool));
    }
}

#[test]
fn mock_probe_supports_mid_run_sequence_transition() {
    let probe = MockDevToolProbe::idle();
    probe.push_sequence([
        BusyToolsStatus::idle(),
        BusyToolsStatus::known([DevTool::Gradle].into_iter().collect()),
    ]);
    assert!(!probe.detect_busy_tools().is_tool_busy(DevTool::Gradle));
    assert!(probe.detect_busy_tools().is_tool_busy(DevTool::Gradle));
    // Falls back to persistent idle status after queue drains.
    assert!(!probe.detect_busy_tools().is_any_busy());
}

#[test]
fn parse_ps_output_skips_self_pid_and_classifies_gradle() {
    let sample = "\
  100 bash /bin/bash
  200 java /usr/bin/java -Xmx2g org.gradle.launcher.daemon.bootstrap.GradleDaemon 8.11.1
  300 cargo cargo build
";
    let snaps = parse_ps_output(sample, 300);
    assert_eq!(snaps.len(), 2);
    let busy = detect_busy_dev_tools(&snaps);
    assert!(busy.contains(&DevTool::Gradle));
    assert!(!busy.contains(&DevTool::Cargo));
}

#[test]
fn processes_that_only_mention_tool_names_are_not_busy() {
    let procs = vec![
        ProcessSnapshot::new(1, "bash", "bash -c pgrep -f GradleDaemon; npm-cli.js"),
        ProcessSnapshot::new(
            2,
            "the build agent",
            "the build agent -p review KotlinCompileDaemon and npm-cli handling",
        ),
        ProcessSnapshot::new(3, "claude", "claude GradleWrapperMain"),
    ];
    assert!(detect_busy_dev_tools(&procs).is_empty());

    let real = vec![
        ProcessSnapshot::new(
            4,
            "java",
            "/opt/jdk/bin/java org.gradle.launcher.daemon.bootstrap.GradleDaemon 8.11.1",
        ),
        ProcessSnapshot::new(
            5,
            "node",
            "node /usr/lib/node_modules/npm/bin/npm-cli.js ci",
        ),
    ];
    let busy = detect_busy_dev_tools(&real);
    assert!(busy.contains(&DevTool::Gradle) && busy.contains(&DevTool::Npm));
}
