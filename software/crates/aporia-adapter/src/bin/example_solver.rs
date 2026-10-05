//! A worked example of an external program, and the one the adapter tests drive.
//!
//! It is a real process, launched over a pipe, speaking the protocol with the same `aporia-adapter`
//! functions APORIA uses — so a test that passes against it has exercised the boundary rather than a
//! mock of it. Anyone writing a solver adapter can copy this file's shape: read a line, decode the
//! request, answer one value per declared output, exit when the pipe closes.
//!
//! The physics is a cantilever beam under a tip load that is deliberately trivial — deflection is a
//! positive number up to 60 N and negative beyond it, so a model declaring `require deflection >= 0`
//! has a region for APORIA to find. The point is not the beam. The point is that one side of the pipe
//! knows nothing about the analysis and the other knows nothing about the arithmetic.
//!
//! ## The misbehaviour switches
//!
//! `APORIA_PROGRAM_MODE` makes the failure paths testable instead of simulated. Each mode is a thing
//! a real program does: a solver that exits when its matrix is singular, a build that prints a banner
//! before the answers, a script that waits on input it never gets, a language that writes `nan` where
//! the protocol wants a number.
//!
//! - `answer` (default) — the beam, plus a work count.
//! - `exit` — answers once, then exits nonzero, as if the second load had broken the solver.
//! - `garbage` — answers with text that is not the protocol.
//! - `banner` — prints a non-protocol line first, which is the most likely real-world mistake.
//! - `arity` — answers with two values for a one-output model.
//! - `refuse` — answers `{"error": ...}`, a refusal rather than a value.
//! - `nan` — answers with the value NaN, which the protocol *does* carry, as a string.
//! - `hang` — stops answering, so the caller's timeout is what ends the run.

use aporia_adapter::{Answer, protocol};
use std::io::{BufRead, BufReader, Write};

fn deflect(load: f64) -> f64 {
    if load > 60.0 { -1.4 } else { 2.1 }
}

fn main() {
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
