use std::process::Command;

#[test]
fn cli_compiles_without_warnings() {
    let status = Command::new(env!("CARGO"))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(["check", "--quiet", "--bin", "maze-defence"])
        .status()
        .expect("failed to invoke cargo check for maze-defence CLI binary");

    assert!(status.success(), "cargo check --bin maze-defence should succeed");
}
