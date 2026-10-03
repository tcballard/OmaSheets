//! Integration-only grid stand-in for service lease lifecycle tests.
fn main() {
    let path = std::path::PathBuf::from(
        std::env::var_os("OMASHEETS_TEST_MARKER").expect("acceptance marker"),
    );
    std::fs::write(&path, b"rust-grid-fixture").unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !path.with_extension("close").exists() {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}
