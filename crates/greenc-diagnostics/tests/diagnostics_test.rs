use chrono::{DateTime, TimeZone, Utc};
use greenc_diagnostics::{ConsentState, Diagnostics, Env, Event, CONSENT_VERSION, ENDPOINT};
use tempfile::TempDir;

fn sample_env() -> Env {
    Env {
        version: "2.2.3".to_string(),
        os: "windows11".to_string(),
        build: 22631,
        system_disk_bytes: Some(500 * 1024 * 1024 * 1024),
    }
}

fn ts(y: i32, m: u32, d: u32, h: u32, min: u32, s: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(y, m, d, h, min, s).unwrap()
}

#[test]
fn diagnostics_is_send_and_sync_and_constants_match_contract() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Diagnostics>();
    assert_eq!(ENDPOINT, "https://greenc-log.cungchat.com/v1/counters");
    assert_eq!(CONSENT_VERSION, 1);
}

#[cfg(not(feature = "store"))]
mod standard {
    use super::*;
    use chrono::Duration;
    use greenc_diagnostics::{Outcome, SendResult};
    use serde_json::Value;
    use std::collections::BTreeSet;
    use std::fs;

    #[test]
    fn default_unanswered_state_does_not_count_or_send_but_writes_log() {
        let tmp = TempDir::new().unwrap();
        let diag = Diagnostics::open(tmp.path().to_path_buf(), sample_env());
        assert_eq!(
            diag.consent(),
            ConsentState {
                available: true,
                granted: false,
                should_ask: true,
            }
        );

        let now = ts(2026, 11, 26, 8, 0, 0);
        diag.record(
            Event::Scan {
                name: "cleanup",
                outcome: Outcome::Ok,
                code: "",
                ms: 3_000,
            },
            now,
        );
        assert!(diag.take_due_batch(now).is_none());

        let exported = tmp.path().join("exported.txt");
        diag.export_log(&exported).unwrap();
        let content = fs::read_to_string(&exported).unwrap();
        assert_eq!(content.trim(), "2026-11-26T08:00:00Z scan cleanup ok - lt5s");
    }

    #[test]
    fn prompt_dismissed_asks_at_most_twice_and_persists() {
        let tmp = TempDir::new().unwrap();
        let diag = Diagnostics::open(tmp.path().to_path_buf(), sample_env());
        assert!(diag.consent().should_ask);

        diag.prompt_dismissed();
        assert!(diag.consent().should_ask);

        diag.prompt_dismissed();
        assert!(!diag.consent().should_ask);
        assert!(!diag.consent().granted);

        // Reload from disk: still should_ask == false.
        let reloaded = Diagnostics::open(tmp.path().to_path_buf(), sample_env());
        assert!(!reloaded.consent().should_ask);
        assert!(!reloaded.consent().granted);
    }

    #[test]
    fn older_consent_version_becomes_unanswered() {
        let tmp = TempDir::new().unwrap();
        let diag = Diagnostics::open(tmp.path().to_path_buf(), sample_env());
        diag.set_consent(true);
        assert!(diag.consent().granted);

        // Downgrade consent_version in state.json.
        let state_path = tmp.path().join("state.json");
        let raw = fs::read_to_string(&state_path).unwrap();
        let mut val: Value = serde_json::from_str(&raw).unwrap();
        // Patch consent_version to 0 regardless of exact nesting key name.
        fn patch_version(v: &mut Value) {
            match v {
                Value::Object(map) => {
                    for (k, val) in map.iter_mut() {
                        if k == "consent_version" || k == "version" {
                            if val.is_u64() {
                                *val = Value::from(0);
                            }
                        } else {
                            patch_version(val);
                        }
                    }
                }
                Value::Array(arr) => {
                    for item in arr {
                        patch_version(item);
                    }
                }
                _ => {}
            }
        }
        patch_version(&mut val);
        fs::write(&state_path, serde_json::to_string(&val).unwrap()).unwrap();

        let reloaded = Diagnostics::open(tmp.path().to_path_buf(), sample_env());
        assert_eq!(
            reloaded.consent(),
            ConsentState {
                available: true,
                granted: false,
                should_ask: true,
            }
        );
    }

    #[test]
    fn corrupt_state_file_is_treated_as_unanswered_and_drops_queue() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("state.json"), "{corrupt json").unwrap();

        let diag = Diagnostics::open(tmp.path().to_path_buf(), sample_env());
        assert_eq!(
            diag.consent(),
            ConsentState {
                available: true,
                granted: false,
                should_ask: true,
            }
        );
        assert!(diag.take_due_batch(ts(2026, 11, 26, 8, 0, 0)).is_none());
    }

    #[test]
    fn active_event_counted_once_per_utc_day_with_build_and_disk_buckets() {
        let tmp = TempDir::new().unwrap();
        let diag = Diagnostics::open(tmp.path().to_path_buf(), sample_env());
        diag.set_consent(true);

        let d1_a = ts(2026, 11, 26, 8, 0, 0);
        let d1_b = ts(2026, 11, 26, 23, 59, 0);
        let d2 = ts(2026, 11, 27, 0, 0, 1);

        diag.record(Event::Active, d1_a);
        diag.record(Event::Active, d1_b);
        diag.record(Event::Active, d2);

        let batch = diag.take_due_batch(d2).expect("batch should be due");
        let v: Value = serde_json::from_str(&batch.body).unwrap();
        let rows = v["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        for row in rows {
            assert_eq!(row["event"], "active");
            assert_eq!(row["count"], 1);
            assert_eq!(row["build"], 22631);
            assert_eq!(row["disk"], "lt512");
            assert_eq!(row["name"], "");
            assert_eq!(row["outcome"], "");
            assert_eq!(row["code"], "");
            assert_eq!(row["duration"], "");
        }
    }

    #[test]
    fn batch_json_matches_exact_contract_keys_and_duration_and_disk_boundaries() {
        let cases: &[(Option<u64>, &str)] = &[
            (None, ""),
            (Some(128 * 1024 * 1024 * 1024 - 1), "lt128"),
            (Some(128 * 1024 * 1024 * 1024), "lt256"),
            (Some(256 * 1024 * 1024 * 1024), "lt512"),
            (Some(512 * 1024 * 1024 * 1024), "lt1024"),
            (Some(1024 * 1024 * 1024 * 1024), "ge1024"),
        ];
        for (bytes, expected_disk) in cases {
            let tmp = TempDir::new().unwrap();
            let mut env = sample_env();
            env.system_disk_bytes = *bytes;
            let diag = Diagnostics::open(tmp.path().to_path_buf(), env);
            diag.set_consent(true);
            let now = ts(2026, 11, 26, 10, 0, 0);
            diag.record(Event::Active, now);
            let b = diag.take_due_batch(now).unwrap();
            let v: Value = serde_json::from_str(&b.body).unwrap();
            assert_eq!(v["rows"][0]["disk"], *expected_disk);
        }

        let tmp = TempDir::new().unwrap();
        let diag = Diagnostics::open(tmp.path().to_path_buf(), sample_env());
        diag.set_consent(true);
        let now = ts(2026, 11, 26, 10, 0, 0);
        let dur_cases = [
            (0, "lt500ms"),
            (499, "lt500ms"),
            (500, "lt2s"),
            (1_999, "lt2s"),
            (2_000, "lt5s"),
            (4_999, "lt5s"),
            (5_000, "lt15s"),
            (14_999, "lt15s"),
            (15_000, "lt60s"),
            (59_999, "lt60s"),
            (60_000, "lt5m"),
            (299_999, "lt5m"),
            (300_000, "ge5m"),
        ];
        for (ms, _) in dur_cases {
            diag.record(Event::Start { ms }, now);
        }
        let batch = diag.take_due_batch(now).unwrap();
        let v: Value = serde_json::from_str(&batch.body).unwrap();

        let top_keys: BTreeSet<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(top_keys, BTreeSet::from(["schema", "batch", "rows"]));
        assert_eq!(v["schema"], 1);
        assert_eq!(v["batch"].as_str().unwrap(), batch.id);
        assert!(uuid::Uuid::parse_str(&batch.id).is_ok());

        let expected_row_keys = BTreeSet::from([
            "day", "version", "os", "event", "name", "outcome", "code", "duration", "build",
            "disk", "count",
        ]);
        let mut counts = std::collections::BTreeMap::new();
        for row in v["rows"].as_array().unwrap() {
            let row_keys: BTreeSet<&str> =
                row.as_object().unwrap().keys().map(String::as_str).collect();
            assert_eq!(row_keys, expected_row_keys);
            assert_eq!(row["build"], 0);
            assert_eq!(row["disk"], "");
            counts.insert(
                row["duration"].as_str().unwrap().to_string(),
                row["count"].as_u64().unwrap(),
            );
        }
        assert_eq!(counts.get("lt500ms"), Some(&2));
        assert_eq!(counts.get("lt2s"), Some(&2));
        assert_eq!(counts.get("lt5s"), Some(&2));
        assert_eq!(counts.get("lt15s"), Some(&2));
        assert_eq!(counts.get("lt60s"), Some(&2));
        assert_eq!(counts.get("lt5m"), Some(&2));
        assert_eq!(counts.get("ge5m"), Some(&1));
    }

    #[test]
    fn queue_limits_count_cap_date_window_and_200_row_32k_batch_limit() {
        let tmp = TempDir::new().unwrap();
        let diag = Diagnostics::open(tmp.path().to_path_buf(), sample_env());
        diag.set_consent(true);

        let now = ts(2026, 11, 26, 12, 0, 0);
        // Count stops at 1000.
        for _ in 0..1_050 {
            diag.record(
                Event::Scan {
                    name: "cleanup",
                    outcome: Outcome::Ok,
                    code: "",
                    ms: 100,
                },
                now,
            );
        }

        // Old (>7 days) and far-future (>1 day) rows are dropped.
        diag.record(
            Event::Start { ms: 100 },
            now - Duration::days(8),
        );
        diag.record(
            Event::Start { ms: 2_000 },
            now + Duration::days(2),
        );
        // Exactly 7 days ago and +1 day are kept.
        diag.record(
            Event::Start { ms: 5_000 },
            now - Duration::days(7),
        );
        diag.record(
            Event::Start { ms: 15_000 },
            now + Duration::days(1),
        );

        let b = diag.take_due_batch(now).unwrap();
        assert!(b.body.len() <= 32_768);
        let v: Value = serde_json::from_str(&b.body).unwrap();
        let rows = v["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 3);
        let scan_row = rows.iter().find(|r| r["event"] == "scan").unwrap();
        assert_eq!(scan_row["count"], 1000);
        assert!(rows.iter().all(|r| r["day"] != "2026-11-18"));
        assert!(rows.iter().all(|r| r["day"] != "2026-11-28"));
        diag.finish_batch(&b.id, SendResult::Accepted, now);

        // Max 2000 distinct keys and max 200 rows / 32768 bytes per batch.
        let t2 = now + Duration::hours(21);
        for i in 0..2_050u32 {
            let code = format!("Win32:{i}");
            diag.record(
                Event::Action {
                    name: "cleanup",
                    outcome: Outcome::Failed,
                    code: &code,
                    ms: 100,
                },
                t2,
            );
        }

        // Verify persisted queue capped at exactly 2000 distinct keys.
        let persisted: Value =
            serde_json::from_str(&fs::read_to_string(tmp.path().join("state.json")).unwrap())
                .unwrap();
        assert_eq!(persisted["queue"].as_array().unwrap().len(), 2_000);

        // First batch respects both 200 rows and 32768 byte limits.
        let first_batch = diag.take_due_batch(t2).unwrap();
        assert!(first_batch.body.len() <= 32_768);
        let parsed_first: Value = serde_json::from_str(&first_batch.body).unwrap();
        let first_count = parsed_first["rows"].as_array().unwrap().len();
        assert!(first_count > 0 && first_count <= 200);
        diag.finish_batch(&first_batch.id, SendResult::Accepted, t2);

        // Separate instance with 250 distinct keys drains in 2 batches within 21h.
        let tmp2 = TempDir::new().unwrap();
        let diag2 = Diagnostics::open(tmp2.path().to_path_buf(), sample_env());
        diag2.set_consent(true);
        for i in 0..250u32 {
            let code = format!("Win32:{i}");
            diag2.record(
                Event::Action {
                    name: "cleanup",
                    outcome: Outcome::Failed,
                    code: &code,
                    ms: 100,
                },
                t2,
            );
        }
        let mut drained = 0usize;
        let mut cursor = t2;
        while let Some(batch) = diag2.take_due_batch(cursor) {
            assert!(batch.body.len() <= 32_768);
            let parsed: Value = serde_json::from_str(&batch.body).unwrap();
            let count = parsed["rows"].as_array().unwrap().len();
            assert!(count > 0 && count <= 200);
            drained += count;
            diag2.finish_batch(&batch.id, SendResult::Accepted, cursor);
            cursor += Duration::hours(21);
        }
        assert_eq!(drained, 250);
    }

    #[test]
    fn due_timing_retry_preserves_batch_id_and_backward_clock_is_due_once() {
        let tmp = TempDir::new().unwrap();
        let diag = Diagnostics::open(tmp.path().to_path_buf(), sample_env());
        diag.set_consent(true);

        let t0 = ts(2026, 11, 26, 10, 0, 0);
        diag.record(Event::Start { ms: 100 }, t0);

        let b1 = diag.take_due_batch(t0).unwrap();
        // Retry keeps exact id and body.
        diag.finish_batch(&b1.id, SendResult::Retry, t0 + Duration::seconds(2));

        // 19 hours later: not due yet.
        assert!(diag.take_due_batch(t0 + Duration::hours(19)).is_none());

        // 20 hours later: due with SAME id and body.
        let t_retry = t0 + Duration::hours(20) + Duration::seconds(2);
        let b2 = diag.take_due_batch(t_retry).unwrap();
        assert_eq!(b2.id, b1.id);
        assert_eq!(b2.body, b1.body);
        diag.finish_batch(&b2.id, SendResult::Rejected, t_retry);
        assert!(diag.take_due_batch(t_retry + Duration::hours(25)).is_none());

        // Backward clock: last_attempt_at is in the future -> treated as due ONCE.
        let t_back = t_retry - Duration::hours(5);
        diag.record(Event::Start { ms: 600 }, t_back);
        let b3 = diag.take_due_batch(t_back).expect("backward clock should be due");
        diag.finish_batch(&b3.id, SendResult::Retry, t_back);
        // Immediately after attempt at t_back, NOT due continuously.
        assert!(diag.take_due_batch(t_back + Duration::minutes(1)).is_none());
    }

    #[test]
    fn revoking_consent_while_batch_in_flight_drops_everything_and_ignores_finish() {
        let tmp = TempDir::new().unwrap();
        let diag = Diagnostics::open(tmp.path().to_path_buf(), sample_env());
        diag.set_consent(true);

        let now = ts(2026, 11, 26, 10, 0, 0);
        diag.record(Event::Start { ms: 300 }, now);
        diag.record_panic("crates/greenc-app/src/lib.rs", 42);

        let in_flight = diag.take_due_batch(now).unwrap();

        // User revokes consent while POST is in flight.
        diag.set_consent(false);
        assert!(!diag.consent().granted);
        assert!(!diag.consent().should_ask);

        // Late finish_batch with Retry must do nothing.
        diag.finish_batch(&in_flight.id, SendResult::Retry, now + Duration::seconds(5));

        // Re-enable later: old data & crash marker must be completely gone.
        diag.set_consent(true);
        assert!(diag.take_due_batch(now + Duration::hours(25)).is_none());

        let reopened = Diagnostics::open(tmp.path().to_path_buf(), sample_env());
        assert!(reopened.take_due_batch(now + Duration::hours(25)).is_none());
    }

    #[test]
    fn crash_marker_normalizes_paths_makes_batch_due_and_clears_on_open() {
        let tmp = TempDir::new().unwrap();
        let diag = Diagnostics::open(tmp.path().to_path_buf(), sample_env());
        diag.set_consent(true);

        // Record a recent attempt so normal 20h window has not elapsed.
        let now = Utc::now();
        diag.record(Event::Start { ms: 100 }, now);
        let b = diag.take_due_batch(now).unwrap();
        diag.finish_batch(&b.id, SendResult::Accepted, now);

        // Windows-style workspace path should normalize backslashes to slashes.
        diag.record_panic("crates\\greenc-app\\src\\commands\\cleanup.rs", 123);

        let reopened = Diagnostics::open(tmp.path().to_path_buf(), sample_env());
        let crash_batch = reopened
            .take_due_batch(now + Duration::minutes(1))
            .expect("unretried crash should make batch due immediately");
        let v: Value = serde_json::from_str(&crash_batch.body).unwrap();
        let rows = v["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["event"], "crash");
        assert_eq!(rows[0]["name"], "crates/greenc-app/src/commands/cleanup.rs:123");
        reopened.finish_batch(&crash_batch.id, SendResult::Accepted, now + Duration::minutes(1));

        // Non-matching path must become "dependency".
        reopened.record_panic("C:\\Users\\Alice\\.cargo\\registry\\src\\foo.rs", 99);
        let reopened2 = Diagnostics::open(tmp.path().to_path_buf(), sample_env());
        let dep_batch = reopened2
            .take_due_batch(now + Duration::minutes(2))
            .expect("dependency crash should be due");
        let v2: Value = serde_json::from_str(&dep_batch.body).unwrap();
        assert_eq!(v2["rows"][0]["event"], "crash");
        assert_eq!(v2["rows"][0]["name"], "dependency");

        // When NOT granted, crash marker is deleted on open without counting.
        reopened2.set_consent(false);
        reopened2.record_panic("crates/greenc-core/src/lib.rs", 10);
        let reopened3 = Diagnostics::open(tmp.path().to_path_buf(), sample_env());
        reopened3.set_consent(true);
        assert!(reopened3.take_due_batch(now + Duration::hours(25)).is_none());
    }

    #[test]
    fn invalid_name_or_code_is_logged_as_unknown_and_not_counted() {
        let tmp = TempDir::new().unwrap();
        let diag = Diagnostics::open(tmp.path().to_path_buf(), sample_env());
        diag.set_consent(true);
        let now = ts(2026, 11, 26, 8, 0, 0);

        diag.record(
            Event::Scan {
                name: "secret_user_folder",
                outcome: Outcome::Ok,
                code: "",
                ms: 100,
            },
            now,
        );
        diag.record(
            Event::Action {
                name: "cleanup",
                outcome: Outcome::Failed,
                code: "C:\\Users\\Alice\\secret.txt",
                ms: 100,
            },
            now,
        );
        assert!(diag.take_due_batch(now).is_none());

        let exported = tmp.path().join("log.txt");
        diag.export_log(&exported).unwrap();
        let text = fs::read_to_string(&exported).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "2026-11-26T08:00:00Z scan unknown ok - lt500ms");
        assert_eq!(lines[1], "2026-11-26T08:00:00Z action cleanup failed unknown lt500ms");
    }

    #[test]
    fn preview_shows_example_when_empty_and_real_batch_when_queued() {
        let tmp = TempDir::new().unwrap();
        let diag = Diagnostics::open(tmp.path().to_path_buf(), sample_env());
        let now = ts(2026, 11, 26, 8, 0, 0);

        let ex_str = diag.preview(now);
        let ex: Value = serde_json::from_str(&ex_str).unwrap();
        assert_eq!(ex["example"], true);
        assert!(!ex["rows"].as_array().unwrap().is_empty());

        diag.set_consent(true);
        diag.record(
            Event::Screen {
                name: "overview",
                cached: true,
                ms: 120,
            },
            now,
        );
        let real_str = diag.preview(now);
        let real: Value = serde_json::from_str(&real_str).unwrap();
        assert!(real.get("example").is_none());
        assert_eq!(real["rows"][0]["event"], "screen");
        assert_eq!(real["rows"][0]["name"], "overview");
        assert_eq!(real["rows"][0]["outcome"], "cached");
        // preview must not consume the queue.
        assert!(diag.take_due_batch(now).is_some());
    }

    #[test]
    fn log_rotates_at_1mib_keeps_max_5_files_prunes_old_and_clears() {
        let tmp = TempDir::new().unwrap();
        let diag = Diagnostics::open(tmp.path().to_path_buf(), sample_env());
        let now = ts(2026, 11, 26, 8, 0, 0);

        // ~60 bytes per line -> 20_000 lines ~ 1.2 MiB -> triggers rotation.
        for _ in 0..20_000 {
            diag.record(
                Event::Action {
                    name: "restore_selective",
                    outcome: Outcome::Partial,
                    code: "InsufficientVaultSpace",
                    ms: 10_000,
                },
                now,
            );
        }

        let log_dir = tmp.path().join("log");
        let files: Vec<_> = fs::read_dir(&log_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert!(files.len() >= 2 && files.len() <= 5);
        for f in &files {
            assert!(f.metadata().unwrap().len() <= 1_048_576);
        }

        // Write 8 days later: older log files are pruned.
        let later = now + Duration::days(8);
        diag.record(Event::Start { ms: 200 }, later);
        let exported = tmp.path().join("after_prune.txt");
        diag.export_log(&exported).unwrap();
        let content = fs::read_to_string(&exported).unwrap();
        assert_eq!(content.trim(), "2026-12-04T08:00:00Z start - - - lt500ms");

        diag.clear_log();
        let remaining: Vec<_> = fs::read_dir(&log_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert!(remaining.is_empty());
    }

    #[test]
    fn unwritable_directory_never_panics() {
        let tmp = TempDir::new().unwrap();
        let blocker = tmp.path().join("not_a_dir");
        fs::write(&blocker, "file").unwrap();
        let bad_dir = blocker.join("diagnostics");

        let diag = Diagnostics::open(bad_dir, sample_env());
        let _ = diag.consent();
        diag.prompt_dismissed();
        diag.set_consent(true);
        let now = ts(2026, 11, 26, 8, 0, 0);
        diag.record(Event::Active, now);
        let _ = diag.preview(now);
        let b = diag.take_due_batch(now);
        if let Some(batch) = b {
            diag.finish_batch(&batch.id, SendResult::Accepted, now);
        }
        diag.record_panic("crates/greenc-core/src/lib.rs", 1);
        diag.clear_log();
    }
}

#[cfg(feature = "store")]
mod store_mode {
    use super::*;

    #[test]
    fn store_feature_disables_everything_and_creates_no_files() {
        let tmp = TempDir::new().unwrap();
        let diag_dir = tmp.path().join("diagnostics");
        let diag = Diagnostics::open(diag_dir.clone(), sample_env());
        assert_eq!(
            diag.consent(),
            ConsentState {
                available: false,
                granted: false,
                should_ask: false,
            }
        );

        let now = ts(2026, 11, 26, 8, 0, 0);
        diag.set_consent(true);
        diag.prompt_dismissed();
        diag.record(Event::Active, now);
        diag.record(Event::Start { ms: 100 }, now);
        diag.record_panic("crates/greenc-app/src/lib.rs", 10);
        assert!(diag.take_due_batch(now).is_none());
        let export_dest = tmp.path().join("export.txt");
        diag.export_log(&export_dest).unwrap();
        diag.clear_log();

        assert!(!diag_dir.exists());
        assert!(!export_dest.exists());
    }
}
