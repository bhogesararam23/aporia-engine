# Documentation

Developer-facing documentation for APORIA, in the repository, tracked, and public. Everything here
describes the code as it is now; where something is unmeasured, unfinished or refused, that is written
as such rather than smoothed over.

| page | what it answers |
|---|---|
| [architecture.md](architecture.md) | how a model becomes a Trust Atlas: A-IR, execution, the five evidence channels, calibration and fusion, the atlas and its labelling policy, the adaptive search, minimisation |
| [adapter-protocol.md](adapter-protocol.md) | the contract an external program has to satisfy — one JSON object per line each way, and what every kind of failure means |
| [reproducibility.md](reproducibility.md) | the archive as an artifact: what it holds, why each field earns its place, integrity versus reproduction, what `compare` will say, how a measurement is named |
| [portability.md](portability.md) | which platforms have been measured and which are only expected, prerequisites, and the transcendental-function caveat on cross-platform replay |
| [developer-guide.md](developer-guide.md) | how to change this software: layout, crate edges, gates, adding a DSL construct, an evidence channel, a corpus entry or an external program, recording an experiment, commit style |
| [prior-work.md](prior-work.md) | where APORIA sits in the published literature: the nearest work on evidence, search and output, which of its own claims that removed, and which citations were resolved to a primary record |
| [limitations.md](limitations.md) | what APORIA does not do, does not know, and has not measured — including the questions that come up most often about it |

Outside this directory, two documents carry the evidence:

- [`../README.md`](../README.md) — what the project is, how to build and run it, and the measured answer
  in one paragraph.
- [`../software/benchmarks/results/README.md`](../software/benchmarks/results/README.md) — the
  measurements themselves: fifteen runs, what each change did, where the method loses, and the
  places where it corrected an earlier claim of its own against the data.

## Reading order

To use it: the root README, then `adapter-protocol.md` if a program is involved.

To judge it: `prior-work.md`, then `limitations.md`, then the results README. The literature position
comes first because it decides which questions are worth asking at all; the research claim is worth
nothing except as measured, and the measurement file is where the negative and inconclusive results are
kept.

To change it: `developer-guide.md`, then `architecture.md` for the reasons, then `reproducibility.md`
before touching anything that writes a file — an archive is a byte contract, and the two most painful
defects in this project's history were a checkout rewriting digested bytes and a field whose meaning
changed while its name did not.

## A note on the other documentation directory

`docs/` at the repository root is **internal** and is never published: it holds planning material, the
working canvas, and numbered decision records that explain how each choice above got made, including
the ones that were wrong first. `.gitignore` keeps it out of the repository entirely, and the gate is
one command that must print `0`:

```sh
git ls-files | grep -c "^docs/"
```
