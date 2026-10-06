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

use aporia_store::{Json, Loaded};

use crate::run::Exit;

/// Open both archives and check both, in one pass, before anything is compared.
///
/// The third value is the integrity problem list, empty when both archives match their own manifests.
/// Reporting a difference between a trustworthy archive and an edited one would present the edit as a
/// result, so both renderers stop here — each in its own format, because a caller reading
/// `"verdict": "IDENTICAL"` out of a half-written document would be worse off than one that got an
/// integrity failure.
fn read_both(a: &Path, b: &Path) -> Result<(Loaded, Loaded, Vec<String>), String> {
    let (left, right) = match (Loaded::open(a), Loaded::open(b)) {
        (Ok(left), Ok(right)) => (left, right),
        (Err(e), _) => {
            return Err(format!("cannot read the archive at `{}`: {e}", a.display()));
        }
        (_, Err(e)) => {
            return Err(format!("cannot read the archive at `{}`: {e}", b.display()));
        }
    };
    let mut broken = Vec::new();
    for (side, loaded) in [(a, &left), (b, &right)] {
        for problem in loaded.integrity_problems() {
            broken.push(format!("{}: {problem}", side.display()));
        }
    }
    Ok((left, right, broken))
}

/// The status a comparison means, decided once so the two renderings cannot disagree about the same
/// `Comparison`.
fn exit_for(comparison: &aporia_store::Comparison) -> Exit {
    match comparison.verdict() {
        aporia_store::Verdict::Identical => Exit::Clean,
        aporia_store::Verdict::Differing => Exit::Differing,
        aporia_store::Verdict::NotFullyComparable => Exit::Incomplete,
    }
}

/// `aporia compare <archive-a> <archive-b>`.
pub fn command(a: &Path, b: &Path, out: &mut dyn Write) -> Result<Exit, String> {
    let (left, right, broken) = read_both(a, b)?;
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
    let comparison = aporia_store::Comparison::new(&left, &right);
    let exit = exit_for(&comparison);
    let _ = write!(out, "{}", comparison.describe(a, b));
    Ok(exit)
}

/// The same two archives and the same one comparison, as a JSON document.
///
/// The human rendering above prints *only the fields there is something to say about*, which is
/// exactly what a caller parsing it would mis-count as "everything agreed". This carries every
/// section, whether it could be compared at all, and the counts — one `Comparison` rendered two ways,
/// not two comparisons that might disagree.
pub fn command_json(a: &Path, b: &Path, out: &mut dyn Write) -> Result<Exit, String> {
    let (left, right, broken) = read_both(a, b)?;
    if !broken.is_empty() {
        let document = Json::object(vec![
            ("schema", Json::text("aporia.compare/1")),
            ("a", Json::text(a.display().to_string())),
            ("b", Json::text(b.display().to_string())),
            // Not a `Verdict`, because no comparison took place. The token is the same one the text
            // prints, so "these two were not diffed" has one name in both renderings.
            ("verdict", Json::text("NOT COMPARED")),
            (
                "integrity",
                Json::Arr(broken.iter().map(|p| Json::text(p.clone())).collect()),
            ),
        ]);
        let _ = writeln!(out, "{}", document.to_pretty());
        return Ok(Exit::Integrity);
    }
    let comparison = aporia_store::Comparison::new(&left, &right);
    let exit = exit_for(&comparison);
    let _ = writeln!(out, "{}", comparison.to_json(a, b).to_pretty());
    Ok(exit)
}
