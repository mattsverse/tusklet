// Run with --no-default-features: UI dependencies add unrelated trait implementations
// to compiler diagnostics and would make these snapshots depend on desktop features.
#![cfg(not(feature = "desktop"))]

#[test]
fn persistence_rejects_invalid_field_and_filter_types() {
    let tests = trybuild::TestCases::new();
    tests.pass("tests/type_safety/valid.rs");
    tests.compile_fail("tests/type_safety/invalid_*.rs");
}
