# Archives, replay, comparison and measurement identity

The artifact APORIA produces is only evidence if someone else, later, on this machine or another, can
make the same numbers appear. This page is about the machinery that makes that checkable: what an
archive holds, what replay does and refuses to do, what `compare` will and will not say about two
runs, and how a measurement gets a name.

## The archive

`aporia run model.ap --archive DIR` writes a directory; `aporia-bench` writes one for every
top-of-ladder campaign of a sweep.

```
manifest.json        what ran, in what configuration, on what model, with what calibration —
                     and a SHA-256 of every other file
model.ap             the source text, as the author wrote it
model.air            the canonical A-IR, which is what replay re-executes
observations.bin     every execution, fixed-record, explicit little-endian bit patterns
atlas.csv            the finished map: cells, labels, risk, sample counts, the observations each holds
bands.csv            boundary bands: axis, interval, faces, transitions seen
decisions.jsonl      one line per search decision: what was asked, where, and what it bought
summary.json         counts, coverage, configuration, environment, and the integrity inputs
findings/NNNNNN.apx  one archive per suspicious region, self-contained: bounds, representative,
                     risk, evidence items, and the counterexample case if one was verified
```

The manifest is written **last**, so a directory containing a manifest is a complete archive; a
half-written run has none. A directory is never overwritten. `created_unix_ms` is stored as 0, which
is the second of two choices that make the same run produce the same digests twice — when a file was
written is the filesystem's answer, and the manifest's answer is what the run contained.

### What is in it and why each field earns its place

| field (manifest) | the question it answers |
|---|---|
| `model_sha256`, `air_sha256` | is this the model I think it is, byte for byte |
| `config` (the campaign configuration as JSON) | what was asked: budget, strategy, seed, every sampling rate, and the atlas policy thresholds |
| `exec` (`fp`, `max_steps`) | how it was executed. An f32 run is a different experiment, and a step guard is part of what stopped an evaluation |
| `counts` | what it cost and what it saw: evaluations, instruction steps, params, outputs, constraints, relations, cells, samples, findings |
| `calibration`, `channel_correlation`, `correlation_samples` | what "risk 0.62" means. Without the fitted scales a score is a number with no unit |
| `environment` | OS, architecture, pointer width, detected toolchain, corpus path, GPU status |
| `files` | the digest list, in write order |

`summary.json` carries the map's shape — coverage as fractions of the declared domain, including the
count of measurements the partition could not place in any leaf, plus the band and finding tallies —
because a reader asking "how much of the space did you actually resolve" should not have to re-derive
it from `atlas.csv`.

`exec.max_steps` was added after a real bug: archives recorded the interpreter's default guard
(50,000,000) for campaigns that ran at 2,000,000, so replay was permitted to finish runs the
experiment had cut off, and any disagreement would have been reported as the build's arithmetic
rather than as the archive's own bookkeeping error. Fields are included when they change how a number
should be read — not because metadata is reassuring.

## Replay

```sh
aporia replay run-0001     # 5 integrity failure, 6 reproduction failure
```

Two questions, deliberately two statuses:

- **Integrity** — does every file still match the digest in the manifest? A mismatch is status `5`:
  these are not the bytes that were written. It is not a scientific finding about the model.
- **Reproduction** — re-executing the archived A-IR at the archived points, with the archived
  execution configuration, gives the same outputs, traces, flags and instruction-step counts. Every
  digest matching and a disagreement here is status `6`: the archive is intact and the arithmetic
  moved.

Collapsing the two into "something is wrong with the archive" is the mistake this split exists to
prevent. And a run executed by an external program is checked for integrity and reported as **not
replayed**: re-running a program this command does not have is not the same experiment, and printing
"0 mismatches" over nothing would be a pass earned by not trying. Cross-platform reproduction has its
own caveat, stated in [`portability.md`](portability.md): transcendental functions are not specified
bit-for-bit, so replay's promise is bit-exactness on the platform that wrote the archive.

## Report

`aporia report DIR` prints a stored run — configuration, counts, calibration, bands, decisions,
finding blocks — reading every number from the files and recomputing none. It exists so that "what did
the experiment say" and "what does re-running it say" are different commands with different failure
modes, and so that an archive can be read on a machine that has no compiler for the model.

## Compare

```sh
aporia compare run-0001 run-0002      # 0 identical, 8 differ, 9 incomparable sections
```

Nothing executes. Seven sections are compared, each reported as *identical*, *changed* (with both
values: `config.budget 40 -> 80`), *only in A* or *only in B*:

`identity` (schema, tool version, model identity) · `run` (configuration and execution mode) ·
`size` (counts) · `calibration` (scales and correlations) · `atlas` (cell labels, coverage including
unplaced measurements, band edges) · `artefacts` (per-file digests, record counts, decision lines,
finding counts) · `findings` (region by region).

Two decisions carry the weight:

- **Findings are paired by the region they describe, not by atlas cell id.** A cell id is an index
  into one run's own partition and means nothing in the other's. A region one run did not find is
  reported as missing there, not as a disagreement about a region both found.
- **Two archives whose models have different parameter counts are not paired at all**, because those
  regions are coordinates in different spaces. The command says which sections it refused rather than
  inventing an alignment.

`8` is deliberately not `1`: a difference between two runs is not a claim that either found a region
worth trusting less. A corrupt archive on either side stops the comparison at `5` rather than
producing a diff out of bytes that may have been edited.

### The machine-readable form

`aporia compare --json <a> <b>` renders the same `Comparison` object as `aporia.compare/1`: the
verdict, the field count, a tally of `changed` / `only_a` / `only_b` / `same`, one entry per section
with whether it was compared at all and how many of its fields differ, the full change list with both
values, and the skipped reasons. The exit status is mapped once from the same verdict, so the two
renderings cannot disagree about the same pair.

`compared: false` exists because the text form's silence is ambiguous to a machine: a section that
agreed and a section that was refused both print no differences, and only the counts tell them apart.
An integrity failure prints `"verdict": "NOT COMPARED"` with the problems in `integrity` — the same
stop as the text form, because a diff over edited bytes is not a comparison. Measured against the two
committed fixtures: identical → `IDENTICAL`, status 0, 65 fields, 0 differences; differing →
`DIFFERENT`, status 8, 31 changed, 3 only in A, 8 only in B.

`report` deliberately has no `--json`. The archive *is* the machine-readable artifact —
`manifest.json`, `summary.json`, `atlas.csv`, `decisions.jsonl`, the `.apx` finding blocks — and
serialising a second copy of them through a renderer would give a reader two representations of one run
to reconcile, which is the pattern behind several of this repository's corrected defects.

## Bench, and the identity of a measurement

`aporia-bench run` will not produce numbers until the corpus has been checked: every declared region
is verified against direct evaluation of the model's own rules, and one that does not hold stops the
run (status `1`) instead of scoring detections against a wrong claim.

Each run writes `software/benchmarks/results/results-<identity>.json`, where the identity is a
12-hex digest of:

```
schema=aporia.results/2
<every plan field: budgets, strategies, seeds, grid, minimise budget, and the six sampling rates>
entries=<every corpus entry and the ground truth it claims>
```

Sorted before digesting, because `--strategies adaptive,random` and `--strategies random,adaptive`
ask one question. Excluding the clock and the environment, because when a machine ran and which machine
ran are provenance, not definition — both still travel inside the document.

A second run of one identity is **refused** rather than written: the two files could only differ in
`wall_ms`, and two names for one experiment is how a published number loses its meaning. `--out DIR`
is the escape hatch when a second copy is genuinely wanted.

The schema is in the digest because a field whose *meaning* changed is a different experiment:
`aporia.results/2` split the counterexample cost into oracle calls and model executions, and a
re-measurement of the same plan over the same corpus had to arrive under its own name instead of
colliding with the run it corrected. Documents written before the identity field existed — all twelve
published ones — are still nameable, because `results_identity` recomputes it from the schema, plan
and entries inside the file.

## The four statuses a reader should learn

| status | meaning |
|---|---|
| `5` | integrity: an archive byte is not the byte that was written |
| `6` | reproduction: the archive is intact and re-execution disagrees |
| `7` | the run finished and the archive could not be written |
| `8` / `9` | the two archives differ / agree on everything that could be compared |

`0`–`4` are `aporia run`'s own: clean, suspicious, usage, unusable model, program stopped answering.
The full list is `aporia help`.
