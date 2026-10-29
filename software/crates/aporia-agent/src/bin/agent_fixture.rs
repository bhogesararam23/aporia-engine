//! A child that makes `execute` observable without invoking the real instrument.
//!
//! `tests/execute.rs` needs a process that writes an archive, exits nonzero, hangs, or does none of
//! those. It is a fixture, not a capability: the end-to-end tests use the real `aporia` binary.

use std::io::Write;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("instrument" | "run") => {
            let archive = args
                .iter()
                .position(|arg| arg == "--archive")
                .and_then(|index| args.get(index + 1));
            let Some(archive) = archive else {
                eprintln!("agent-fixture: instrument needs --archive");
                std::process::exit(2);
            };
            std::fs::create_dir_all(archive).expect("archive dir");
            std::fs::write(
                std::path::Path::new(archive).join("manifest.json"),
                b"{\"fixture\":true}\n",
            )
            .expect("manifest");
            std::fs::write(
                std::path::Path::new(archive).join("model.ap"),
                b"model x { }\n",
            )
            .expect("model");
            println!("the fixture wrote an archive");
        }
        Some("echo") => {
            let mut stdout = std::io::stdout().lock();
            let _ = writeln!(stdout, "{}", args[1..].join(" "));
        }
        Some("fail") => {
            let code = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(1);
            std::process::exit(code);
        }
        Some("sleep") => {
            let millis = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(1000);
            std::thread::sleep(std::time::Duration::from_millis(millis));
            println!("slept");
        }
        _ => {
            eprintln!("agent-fixture: choose instrument, echo, fail or sleep");
            std::process::exit(2);
        }
    }
}
