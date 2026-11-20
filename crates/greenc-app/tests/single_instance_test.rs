use greenc_app::acquire_single_instance;
use greenc_core::error::GreenCError;

#[test]
fn test_app_single_instance_enforcement() {
    let first = acquire_single_instance();
    assert!(
        first.is_ok(),
        "First single-instance acquisition must succeed"
    );

    let first_guard = first.unwrap();

    let second = acquire_single_instance();
    assert!(
        matches!(second, Err(GreenCError::PreconditionBlocked { .. })),
        "Second acquisition while first is held must fail with PreconditionBlocked"
    );

    drop(first_guard);

    let third = acquire_single_instance();
    assert!(
        third.is_ok(),
        "Acquisition after dropping guard must succeed"
    );
}
