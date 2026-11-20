use greenc_core::error::GreenCError;
use greenc_platform::mutex::GreenCAppMutex;

#[test]
fn test_single_instance_mutex() {
    let test_mutex_name = format!("GreenCTestMutex_Win32_{}", std::process::id());

    let mutex_result = GreenCAppMutex::acquire_named(&test_mutex_name);
    assert!(mutex_result.is_ok(), "First acquire should succeed");

    let first = mutex_result.unwrap();

    // Second acquire in the same process must fail because mutex is already held
    let second_res = GreenCAppMutex::acquire_named(&test_mutex_name);
    assert!(
        matches!(second_res, Err(GreenCError::PreconditionBlocked { .. })),
        "Second acquire must fail with PreconditionBlocked"
    );

    drop(first);

    // After dropping first, acquire should succeed again
    let third_res = GreenCAppMutex::acquire_named(&test_mutex_name);
    assert!(third_res.is_ok(), "Acquire after drop should succeed");
}

#[test]
fn test_single_instance_mutex_named_isolation() {
    let mutex_name_a = format!("GreenCTestMutex_A_{}", std::process::id());
    let mutex_name_b = format!("GreenCTestMutex_B_{}", std::process::id());

    let guard_a = GreenCAppMutex::acquire_named(&mutex_name_a).expect("Acquire A should succeed");
    let guard_b = GreenCAppMutex::acquire_named(&mutex_name_b)
        .expect("Acquire B should succeed concurrently with A");

    drop(guard_a);
    drop(guard_b);
}
