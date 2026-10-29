//! A child process that makes the sandbox's promises observable.
//!
//! It is deliberately not part of APORIA's product surface: `tests/exec.rs` needs a real process
//! that can exit with a chosen status, outlive a timeout, emit more bytes than the retention cap,
//! and print what its environment and working directory actually are.

use std::io::Write;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("echo") => {
            println!("{}", args[1..].join(" "));
        }
        Some("sleep") => {
            let millis = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(1000);
            std::thread::sleep(std::time::Duration::from_millis(millis));
            println!("slept");
        }
        Some("emit") => {
            let bytes: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
            let chunk = [b'x'; 8192];
            let mut stdout = std::io::stdout().lock();
            let mut left = bytes;
            while left > 0 {
                let take = usize::try_from(left.min(chunk.len() as u64)).unwrap_or(chunk.len());
                stdout.write_all(&chunk[..take]).expect("emit");
                left -= take as u64;
            }
            stdout.flush().expect("flush");
        }
        Some("dump") => {
            let name = args.get(1).map_or("", String::as_str);
            match std::env::var(name) {
                Ok(value) => println!("{value}"),
                Err(_) => println!("ABSENT"),
            }
        }
        Some("cwd") => {
            println!("{}", std::env::current_dir().expect("cwd").display());
        }
        Some("fail") => {
            let code = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(1);
            std::process::exit(code);
        }
        _ => {
            eprintln!("sandbox-fixture: choose echo, sleep, emit, dump, cwd or fail");
            std::process::exit(2);
        }
    }
}
