//! The example program's loop, shared by every binary that wants a worked example.
use crate::protocol::{self, Answer};
use std::io::{BufRead, BufReader, Write};

fn deflect(load: f64) -> f64 {
    if load > 60.0 { -1.4 } else { 2.1 }
}

pub fn serve() {
    let mode = std::env::var("APORIA_PROGRAM_MODE").unwrap_or_else(|_| "answer".to_string());
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    let mut lines = BufReader::new(stdin.lock()).lines();

    // A program that cannot read its own name is a program whose answers cannot be attributed.
    let _program = std::env::args().next().unwrap_or_default();

    for (i, line) in (&mut lines).enumerate() {
        let Ok(line) = line else { break }; // the pipe closed: this is the normal exit
        let Ok(request) = protocol::decode_request(&line, 1) else {
            // A malformed request is the caller's mistake, and the answer is to say so and keep
            // listening: a protocol error here is not this program's decision to make.
            let mut err = protocol::encode_refusal("the request is not one parameter");
            err.push('\n');
            let _ = out.write_all(err.as_bytes());
            let _ = out.flush();
            continue;
        };
        match mode.as_str() {
            "exit" if i >= 1 => {
                let _ = out.flush();
                std::process::exit(3);
            }
            "garbage" => {
                let _ = writeln!(out, "deflection = 2.1 mm");
            }
            "banner" if i == 0 => {
                let _ = writeln!(out, "# aporia-example-solver ready");
            }
            "arity" => {
                let _ = writeln!(
                    out,
                    "{}",
                    protocol::encode_response(&Answer {
                        y: vec![1.0, 2.0],
                        steps: 0
                    })
                );
            }
            "refuse" => {
                let _ = writeln!(out, "{}", protocol::encode_refusal("matrix was singular"));
            }
            "nan" => {
                let _ = writeln!(
                    out,
                    "{}",
                    protocol::encode_response(&Answer {
                        y: vec![f64::NAN],
                        steps: 0
                    })
                );
            }
            "hang" => {
                // Never answer. The caller's timeout is the only thing that ends this, which is
                // exactly what the test needs to observe.
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(3600));
                }
            }
            _ => {
                let _ = writeln!(
                    out,
                    "{}",
                    protocol::encode_response(&Answer {
                        y: vec![deflect(request[0])],
                        steps: 12,
                    })
                );
            }
        }
        let _ = out.flush();
    }
}
