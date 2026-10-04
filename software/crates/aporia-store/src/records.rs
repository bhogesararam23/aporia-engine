//! The observation file: every execution, kept exactly.
//!
//! Observations are the bulk of an experiment and the thing a replay has to reproduce, so they get a
//! fixed-record binary form rather than text. The reasons are the ones that matter for numbers:
//!
//! - an f64 is stored as its 64-bit pattern, so what comes back is bit-identical to what went in.
//!   Decimal text would round-trip through `Display` but would make a *file* depend on formatting,
//!   and a digest of a file is only meaningful if the file is byte-stable.
//! - record order is evaluation order, which is the order the search produced them, so an atlas can
//!   be rebuilt from the file alone.
//! - it is compact enough that a ten-thousand-evaluation experiment is a few megabytes rather than a
//!   text file nobody will open.
//!
//! The layout is documented in [`MAGIC`]'s terms below and is versioned by its own first six bytes,
//! so a reader can say *which* format it is holding instead of guessing from field order.

use aporia_runtime::{Observation, Records, value::Flags};
use std::io::{self, Read, Write};

/// `APOR` plus the record-format version. A file that does not start with this is not ours.
pub const MAGIC: [u8; 6] = *b"APOR1\0";

/// One header, then `count` records back to back.
///
/// ```text
/// header: magic[6] arity u16 outputs u16 traces u16 count u64 total_steps u64
/// record: id u64 steps u64 flags u8 pad[7] x[arity] y[outputs]
///         then, per trace, len u32 followed by len f64
/// ```
///
/// Every integer is little-endian and every float is a bit pattern, so the file is the same on every
/// machine this project runs on.
#[derive(Clone, Copy, Debug)]
pub struct Header {
    pub arity: u16,
    pub outputs: u16,
    pub traces: u16,
    pub count: u64,
    pub total_steps: u64,
}

/// Write every observation. Returns the number of bytes written.
pub fn write(w: &mut impl Write, records: &Records) -> io::Result<u64> {
    let first = records.items.first();
    let header = Header {
        arity: u16::try_from(first.map_or(0, |o| o.x.len())).unwrap_or(u16::MAX),
        outputs: u16::try_from(first.map_or(0, |o| o.y.len())).unwrap_or(u16::MAX),
        traces: u16::try_from(first.map_or(0, |o| o.traces.len())).unwrap_or(u16::MAX),
        count: records.items.len() as u64,
        // Derived in `Records`, but stored in the header so a reader can report the cost without
        // loading every record.
        total_steps: records.total_steps(),
    };
    let mut written = 0u64;
    written += put(w, &MAGIC)?;
    written += put_u16(w, header.arity)?;
    written += put_u16(w, header.outputs)?;
    written += put_u16(w, header.traces)?;
    written += put_u64(w, header.count)?;
    written += put_u64(w, header.total_steps)?;
    for o in &records.items {
        written += put_u64(w, o.id)?;
        written += put_u64(w, o.steps)?;
        written += put(w, &[o.flags.to_bits()])?;
        written += put(w, &[0; 7])?;
        for v in &o.x {
            written += put_u64(w, v.to_bits())?;
        }
        for v in &o.y {
            written += put_u64(w, v.to_bits())?;
        }
        for series in &o.traces {
            written += put_u32(w, series.len() as u32)?;
            for v in series {
                written += put_u64(w, v.to_bits())?;
            }
        }
    }
    Ok(written)
}

/// Read a whole observation file.
pub fn read(bytes: &[u8]) -> Result<Records, crate::StoreError> {
    let Some(magic) = bytes.get(0..6) else {
        return Err(crate::StoreError::Truncated { at: 0, needed: 6 });
    };
    if magic != MAGIC {
        return Err(crate::StoreError::Format(format!(
            "not an observation file: starts with {:02x?}",
            &bytes[..bytes.len().min(6)]
        )));
    }
    let mut at = 6usize;
    let arity = take_u16(bytes, &mut at)?;
    let outputs = take_u16(bytes, &mut at)?;
    let traces = take_u16(bytes, &mut at)?;
    let count = take_u64(bytes, &mut at)?;
    let total_steps = take_u64(bytes, &mut at)?;
    let mut records = Records::new();
    for _ in 0..count {
        let id = take_u64(bytes, &mut at)?;
        let steps = take_u64(bytes, &mut at)?;
        let Some(&flag_byte) = bytes.get(at) else {
            return Err(crate::StoreError::Truncated { at, needed: 1 });
        };
        at += 1;
        // The seven padding bytes are part of the record size, so they are skipped explicitly
        // rather than assumed to be whatever happens to be there.
        if bytes
            .get(at..at + 7)
            .is_none_or(|pad| pad.iter().any(|&b| b != 0))
        {
            return Err(crate::StoreError::Format(format!(
                "record {id} has non-zero padding at byte {at}, which means the file was written by \
                 a different format"
            )));
        }
        at += 7;
        let mut x = Vec::with_capacity(arity as usize);
        for _ in 0..arity {
            x.push(f64::from_bits(take_u64(bytes, &mut at)?));
        }
        let mut y = Vec::with_capacity(outputs as usize);
        for _ in 0..outputs {
            y.push(f64::from_bits(take_u64(bytes, &mut at)?));
        }
        let mut series = Vec::with_capacity(traces as usize);
        for _ in 0..traces {
            let len = take_u32(bytes, &mut at)? as usize;
            let mut one = Vec::with_capacity(len);
            for _ in 0..len {
                one.push(f64::from_bits(take_u64(bytes, &mut at)?));
            }
            series.push(one);
        }
        records.items.push(Observation {
            x,
            y,
            traces: series,
            flags: Flags::from_bits(flag_byte),
            steps,
            id,
        });
    }
    if at != bytes.len() {
        return Err(crate::StoreError::Format(format!(
            "{} trailing bytes after the last record",
            bytes.len() - at
        )));
    }
    let _ = total_steps;
    Ok(records)
}

/// The header alone, for a report that wants counts without loading every record.
pub fn header(bytes: &[u8]) -> Result<Header, crate::StoreError> {
    if bytes.get(0..6).is_some_and(|m| m == MAGIC) {
        let mut at = 6usize;
        Ok(Header {
            arity: take_u16(bytes, &mut at)?,
            outputs: take_u16(bytes, &mut at)?,
            traces: take_u16(bytes, &mut at)?,
            count: take_u64(bytes, &mut at)?,
            total_steps: take_u64(bytes, &mut at)?,
        })
    } else {
        Err(crate::StoreError::Format(
            "not an observation file".to_string(),
        ))
    }
}

fn put(w: &mut impl Write, b: &[u8]) -> io::Result<u64> {
    w.write_all(b)?;
    Ok(b.len() as u64)
}

fn put_u16(w: &mut impl Write, v: u16) -> io::Result<u64> {
    put(w, &v.to_le_bytes())
}

fn put_u32(w: &mut impl Write, v: u32) -> io::Result<u64> {
    put(w, &v.to_le_bytes())
}

fn put_u64(w: &mut impl Write, v: u64) -> io::Result<u64> {
    put(w, &v.to_le_bytes())
}

fn take_u16(bytes: &[u8], at: &mut usize) -> Result<u16, crate::StoreError> {
    let slice = take(bytes, at, 2)?;
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}

fn take_u32(bytes: &[u8], at: &mut usize) -> Result<u32, crate::StoreError> {
    let slice = take(bytes, at, 4)?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn take_u64(bytes: &[u8], at: &mut usize) -> Result<u64, crate::StoreError> {
    let slice = take(bytes, at, 8)?;
    Ok(u64::from_le_bytes(slice.try_into().unwrap_or([0; 8])))
}

fn take<'a>(bytes: &'a [u8], at: &mut usize, n: usize) -> Result<&'a [u8], crate::StoreError> {
    let end = *at + n;
    match bytes.get(*at..end) {
        Some(slice) => {
            *at = end;
            Ok(slice)
        }
        None => Err(crate::StoreError::Truncated { at: *at, needed: n }),
    }
}

/// Read to the end of a file into memory. Kept here so `read` and callers use the same limit rules.
pub fn read_all(r: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    r.read_to_end(&mut out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aporia_dsl::lower::compile;
    use aporia_runtime::{interp, value::ExecConfig};

    fn records(n: usize) -> Records {
        let c = compile(
            "t.ap",
            "model w \"\" {\n input x in [0, 10]\n input dt : s in [0, 1]\n state e = 1.0\n loop 3 {\n advance e = e * 0.5\n watch e\n }\n let y = x * x\n let z = y / dt\n}\n",
        );
        assert!(!c.diagnostics.has_errors(), "{}", c.diagnostics);
        let m = c.model;
        let mut r = Records::new();
        for i in 0..n {
            let x = vec![i as f64, if i % 7 == 3 { 0.0 } else { 0.01 * i as f64 }];
            let outcome = interp::run(&m, &x, ExecConfig::default());
            r.push(Observation::new(i as u64, x, &outcome));
        }
        r
    }

    #[test]
    fn a_written_file_reads_back_bit_for_bit() {
        let original = records(11);
        let mut bytes = Vec::new();
        let written = write(&mut bytes, &original).expect("write");
        assert_eq!(written as usize, bytes.len());
        let back = read(&bytes).expect("read");
        assert_eq!(back.items.len(), original.items.len());
        for (a, b) in original.items.iter().zip(back.items.iter()) {
            assert_eq!(a.id, b.id);
            assert_eq!(a.steps, b.steps);
            assert_eq!(a.flags.to_bits(), b.flags.to_bits());
            for (x, y) in a.x.iter().zip(b.x.iter()) {
                assert_eq!(x.to_bits(), y.to_bits());
            }
            // Including the values that only exist because of IEEE: NaN from 1/0 and the traces.
            for (x, y) in a.y.iter().zip(b.y.iter()) {
                assert_eq!(x.to_bits(), y.to_bits());
            }
            assert_eq!(a.traces, b.traces);
        }
        assert_eq!(back.total_steps(), original.total_steps());
    }

    #[test]
    fn a_nan_survives_the_round_trip_as_the_same_bits() {
        // The point of a bit pattern rather than text: `1/0` produces NaN, and NaN does not compare
        // equal to itself, so a text format would quietly lose the finding.
        let original = records(11);
        let mut bytes = Vec::new();
        write(&mut bytes, &original).unwrap();
        let back = read(&bytes).unwrap();
        let with_nan = original
            .items
            .iter()
            .find(|o| o.y.iter().any(|v| v.is_nan()))
            .expect("the division by zero should have produced a NaN");
        let copied = &back.items[with_nan.id as usize];
        assert!(copied.y.iter().any(|v| v.is_nan()));
        assert_eq!(with_nan.flags.to_bits(), copied.flags.to_bits());
    }

    #[test]
    fn the_header_reports_counts_without_reading_records() {
        let original = records(5);
        let mut bytes = Vec::new();
        write(&mut bytes, &original).unwrap();
        let h = header(&bytes).unwrap();
        assert_eq!(h.count, 5);
        // Asserted against the observations themselves rather than numbers typed into the test,
        // because what the model produced is the thing being checked.
        let first = &original.items[0];
        assert_eq!(h.arity as usize, first.x.len());
        assert_eq!(h.outputs as usize, first.y.len());
        assert_eq!(h.traces as usize, first.traces.len());
        assert_eq!(h.total_steps, original.total_steps());
    }

    #[test]
    fn a_truncated_file_says_where_it_stopped() {
        let mut bytes = Vec::new();
        write(&mut bytes, &records(3)).unwrap();
        bytes.truncate(bytes.len() - 9);
        let e = read(&bytes).unwrap_err();
        assert!(
            matches!(e, crate::StoreError::Truncated { .. }),
            "unexpected error: {e:?}"
        );
    }

    #[test]
    fn a_foreign_file_is_refused_by_its_magic() {
        let e = read(b"not an aporia file at all").unwrap_err();
        assert!(matches!(e, crate::StoreError::Format(_)), "{e:?}");
    }

    #[test]
    fn non_zero_padding_is_treated_as_a_different_format_not_ignored() {
        let mut bytes = Vec::new();
        write(&mut bytes, &records(1)).unwrap();
        // The header is 28 bytes, then id (8) and steps (8) and the flag byte (1).
        let at = 28 + 8 + 8 + 1;
        bytes[at] = 7;
        let e = read(&bytes).unwrap_err();
        assert!(matches!(e, crate::StoreError::Format(_)), "{e:?}");
    }

    #[test]
    fn an_empty_experiment_is_a_valid_file() {
        let mut bytes = Vec::new();
        write(&mut bytes, &Records::new()).unwrap();
        let back = read(&bytes).unwrap();
        assert!(back.items.is_empty());
    }
}
