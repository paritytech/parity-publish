use std::process::Command;

/// Test that the CLI accepts the new polling flags
#[test]
fn test_apply_help_shows_poll_flags() {
    let output = Command::new("cargo")
        .args(["run", "--", "apply", "--help"])
        .output()
        .expect("Failed to run command");

    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        stdout.contains("--poll-interval"),
        "Help should show --poll-interval flag"
    );
    assert!(
        stdout.contains("--poll-timeout"),
        "Help should show --poll-timeout flag"
    );
}

/// Test that poll-interval accepts valid values
#[test]
fn test_poll_interval_accepts_number() {
    // This test verifies the CLI parsing works - it will fail at runtime
    // because there's no Plan.toml, but that's expected
    let output = Command::new("cargo")
        .args(["run", "--", "apply", "--poll-interval", "10", "--help"])
        .output()
        .expect("Failed to run command");

    // --help should succeed even with other flags
    assert!(output.status.success(), "Command with --poll-interval should parse successfully");
}

/// Test that poll-timeout accepts valid values
#[test]
fn test_poll_timeout_accepts_number() {
    let output = Command::new("cargo")
        .args(["run", "--", "apply", "--poll-timeout", "120", "--help"])
        .output()
        .expect("Failed to run command");

    assert!(output.status.success(), "Command with --poll-timeout should parse successfully");
}

/// Test that poll-interval rejects invalid values
#[test]
fn test_poll_interval_rejects_negative() {
    let output = Command::new("cargo")
        .args(["run", "--", "apply", "--poll-interval", "-1"])
        .output()
        .expect("Failed to run command");

    // Should fail to parse negative number for u64
    assert!(!output.status.success(), "Command should reject negative poll-interval");
}

/// Test that poll-interval rejects non-numeric values
#[test]
fn test_poll_interval_rejects_string() {
    let output = Command::new("cargo")
        .args(["run", "--", "apply", "--poll-interval", "abc"])
        .output()
        .expect("Failed to run command");

    assert!(!output.status.success(), "Command should reject non-numeric poll-interval");
}

/// Test default values are documented in help
#[test]
fn test_default_values_in_help() {
    let output = Command::new("cargo")
        .args(["run", "--", "apply", "--help"])
        .output()
        .expect("Failed to run command");

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Check that default values are mentioned
    assert!(
        stdout.contains("default: 5") || stdout.contains("[default: 5]"),
        "Help should show default poll-interval of 5 seconds"
    );
    assert!(
        stdout.contains("default: 60") || stdout.contains("[default: 60]"),
        "Help should show default poll-timeout of 60 seconds"
    );
}
