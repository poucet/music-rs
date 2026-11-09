//! audiowatch -- watch which processes put audio on this Mac's outputs.
//!
//! See README.md for what it detects and, more importantly, what it cannot.
//!
//! The watcher is CoreAudio and libproc, so it is macOS only; the recorder
//! (`rec`, `--mcp`) is cpal and MCP, and runs wherever they do.

#[cfg(target_os = "macos")]
mod agent;
#[cfg(target_os = "macos")]
mod cli;
#[cfg(target_os = "macos")]
mod config;
#[cfg(target_os = "macos")]
mod event;
#[cfg(target_os = "macos")]
mod filter;
#[cfg(target_os = "macos")]
mod hal;
#[cfg(target_os = "macos")]
mod logfile;
#[cfg(target_os = "macos")]
mod notify;
#[cfg(target_os = "macos")]
mod proc;
#[cfg(target_os = "macos")]
mod reconcile;
#[cfg(target_os = "macos")]
mod sys;
#[cfg(target_os = "macos")]
mod timefmt;
#[cfg(target_os = "macos")]
mod watch;
#[cfg(target_os = "macos")]
mod watcher;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // The recorder has its own arguments: `rec ...` and `--mcp ...`.
    if args.first().map(String::as_str) == Some("rec") {
        return audiowatch::rec::cli::main(&args[1..]);
    }
    if args.iter().any(|a| a == "--mcp") {
        return audiowatch::rec::server::main(&args);
    }
    watch_main(&args)
}

#[cfg(target_os = "macos")]
fn watch_main(args: &[String]) -> ExitCode {
    watcher::main(args)
}

#[cfg(not(target_os = "macos"))]
fn watch_main(_args: &[String]) -> ExitCode {
    eprintln!(
        "audiowatch: watching audio output is macOS only (CoreAudio and libproc).\n\
         \x20 The recorder works here: `audiowatch rec ...` and `audiowatch --mcp`."
    );
    ExitCode::FAILURE
}
