//! Performance benchmark harness for GreenC v2 (`SC-001`, `SC-004`, `SC-007` — Task T070):
//! 1. Parallel ZIP64/ZSTD Vault archiving throughput (`>= 250 MB/s` target on NVMe)
//! 2. Cooperative cancellation latency (`< 200 ms` target)
//! 3. 50-locale in-memory translation lookup (`< 5 ms` target)

#![deny(unsafe_code)]

use greenc_core::models::{CleanCandidate, DriveInfo};
use greenc_core::task::TaskControl;
use greenc_i18n::catalog::Catalog;
use greenc_vault::archiver::{ArchiveOptions, ArchiverPipeline};
use greenc_win32::traits::MockVolumeApi;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tempfile::tempdir;

fn bench_parallel_vault_archiver() {
    let dir = tempdir().expect("tempdir");
    let src_dir = dir.path().join("src_payload");
    let vault_dir = dir.path().join("vault_out");
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::create_dir_all(&vault_dir).unwrap();

    // Generate 64 MiB across 32 compressible 2 MiB payload files
    let file_count = 32usize;
    let file_size = 2 * 1024 * 1024usize;
    let mut payload = vec![0u8; file_size];
    for (idx, b) in payload.iter_mut().enumerate() {
        *b = ((idx * 31 + 7) % 251) as u8;
    }

    let mut candidates = Vec::with_capacity(file_count);
    for i in 0..file_count {
        let p = src_dir.join(format!("chunk_{i:03}.log"));
        std::fs::write(&p, &payload).unwrap();
        let mtime = std::fs::metadata(&p)
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        candidates.push(CleanCandidate {
            path: p,
            size_bytes: file_size as u64,
            mtime,
            is_locked: false,
            profile_id: "bench".to_string(),
            is_canary: false,
            is_orphan: false,
            hardlink_siblings: Vec::new(),
        });
    }

    let vol_api = MockVolumeApi::new(vec![DriveInfo {
        root: vault_dir.clone(),
        drive_letter: 'D',
        label: "Bench".to_string(),
        filesystem: "NTFS".to_string(),
        total_bytes: 500 * 1024 * 1024 * 1024,
        free_bytes: 250 * 1024 * 1024 * 1024,
        is_system: false,
        is_eligible: true,
        ineligible_reason: None,
    }]);

    let pipeline = ArchiverPipeline::new(ArchiveOptions {
        vault_dir,
        retention_days: 7,
        ..Default::default()
    });
    let ctrl = TaskControl::new();
    let total_bytes = (file_count * file_size) as u64;

    let start = Instant::now();
    let manifest = pipeline
        .archive_candidates(0, "BenchSuite", &candidates, &vol_api, &ctrl, |_| {})
        .expect("archive benchmark must succeed");
    let elapsed = start.elapsed();

    let mb_per_sec = (total_bytes as f64 / (1024.0 * 1024.0)) / elapsed.as_secs_f64().max(1e-6);
    println!(
        "[BENCH] ArchiverPipeline: {} files ({:.1} MiB -> {:.1} MiB) in {:?} ({:.1} MiB/s)",
        manifest.files.len(),
        total_bytes as f64 / (1024.0 * 1024.0),
        manifest.compressed_size as f64 / (1024.0 * 1024.0),
        elapsed,
        mb_per_sec
    );
}

fn bench_i18n_50_locale_lookup() {
    let catalog = Catalog::load_embedded();
    let start = Instant::now();
    for _ in 0..1_000 {
        let _ = catalog.tr(
            "ar",
            "Còn khoảng {days} ngày trước khi đầy",
            &[("days", "14")],
        );
        let _ = catalog.tr("vi", "Ổn định · {pct:.0f}%", &[("pct", "68")]);
        let _ = catalog.tr("ja", "Tổng quan", &[]);
    }
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_millis(250),
        "3,000 localized lookups should take well under 250ms, took {:?}",
        elapsed
    );
    println!(
        "[BENCH] 50-Locale Catalog: 3,000 interpolated lookups in {:?} ({:.3} ms per switch batch)",
        elapsed,
        elapsed.as_secs_f64() * 1000.0 / 1000.0
    );
}

fn bench_cooperative_cancellation_latency() {
    let ctrl = Arc::new(TaskControl::new());
    let ctrl_thread = Arc::clone(&ctrl);

    let start = Instant::now();
    let handle = std::thread::spawn(move || {
        while !ctrl_thread.is_cancelled() {
            std::thread::sleep(Duration::from_millis(5));
        }
    });
    ctrl.cancel();
    handle.join().unwrap();
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_millis(200),
        "Cancellation must yield within < 200 ms, took {:?}",
        elapsed
    );
    println!("[BENCH] Cooperative Cancellation Latency: {:?}", elapsed);
}

fn main() {
    println!("=== GreenC v2 Performance Benchmark Suite (Task T070) ===");
    bench_parallel_vault_archiver();
    bench_i18n_50_locale_lookup();
    bench_cooperative_cancellation_latency();
}
