//! Putting two stored runs next to each other, without executing either model.
//!
//! `aporia report` answers "what does this archive say"; this answers "what is different between
//! these two". The comparison itself is `aporia-store`'s, because that is where the archive's
//! representation lives; this module only decides the two things a command line owns: which
//! directories the caller meant, and what the process exits with.
//!
//! Integrity is checked on both sides first, and a corrupt archive stops the comparison. Two
//! directories that do not match their own manifests have no trustworthy fields to diff, and printing
//! a difference between them would attach a conclusion to bytes that may have been edited.
//!
//! The exit status distinguishes the three endings a comparison can have. `Differing` is not
//! `Suspicious`: a pair of runs that disagree says nothing about whether either found a region worth
//! trusting less. `Incomplete` is not `Clean`: fields that could not be asked about did not agree,
//! they were never reached.

use std::io::Write;
use std::path::Path;

use aporia_store::Loaded;

use crate::run::Exit;

/// `aporia compare <archive-a> <archive-b>`.
pub fn command(a: &Path, b: &Path, out: &mut dyn Write) -> Result<Exit, String> {
    let (left, right) = match (Loaded::open(a), Loaded::open(b)) {
        (Ok(left), Ok(right)) => (left, right),
        (Err(e), _) => {
            return Err(format!("cannot read the archive at `{}`: {e}", a.display()));
        }
        (_, Err(e)) => {
            return Err(format!("cannot read the archive at `{}`: {e}", b.display()));
        }
    };
    // Both sides, in one pass, before anything is compared: reporting a difference between a
    // trustworthy archive and an edited one would present the edit as a result.
    let mut broken = Vec::new();
    for (side, loaded) in [(a, &left), (b, &right)] {
        for problem in loaded.integrity_problems() {
            broken.push(format!("{}: {problem}", side.display()));
        }
    }
    if !broken.is_empty() {
        let _ = writeln!(out, "integrity  {} problem(s)", broken.len());
        for problem in &broken {
            let _ = writeln!(out, "  {problem}");
        }
        let _ = writeln!(
            out,
            "verdict  NOT COMPARED: an archive that does not match its own manifest has nothing \
             reliable to diff"
        );
        return Ok(Exit::Integrity);
    }
    let comparison = aporia_store::compare::Comparison::new(&left, &right);
    let _ = write!(out, "{}", comparison.describe(a, b));
    Ok(match comparison.verdict() {
        aporia_store::Verdict::Identical => Exit::Clean,
        aporia_store::Verdict::Differing => Exit::Differing,
        aporia_store::Verdict::NotFullyComparable => Exit::Incomplete,
    })
}
