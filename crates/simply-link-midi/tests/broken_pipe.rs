//! `simply-link listen | head`: a reader that hangs up early ends the program
//! quietly, not with a "failed printing to stdout" panic.

use std::process::{Command, Stdio};

#[test]
fn a_closed_stdout_exits_quietly() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_simply-link"))
        .args(["listen", "--seconds", "0.5"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Hang up before it has printed anything.
    drop(child.stdout.take());
    let output = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "status {:?}, stderr: {stderr}", output.status);
    assert!(!stderr.contains("panicked"), "stderr: {stderr}");
}
