# The external-program contract

APORIA can analyse a model whose numbers come from a program it did not parse. The model declares an
output with no instruction computing it; a separate process answers for it. This page is the whole
contract between them — what arrives, what must come back, and what every kind of failure means.

It is deliberately small: one JSON object per line in each direction, request then response, in order.
No negotiation, no length framing, no session, no network. Anything that can read a line and print a
line can implement it, which matters because the program on the other end is not APORIA's to choose.

Reference implementation, Rust: `software/crates/aporia-adapter/src/bin/example_solver.rs`.
Reference implementation, sharing nothing with APORIA: `software/examples/beam.py`.

## What arrives

One line per evaluation, terminated by `\n`:

```json
{"x":[12.5]}
```

`x` holds one number per declared `input`, **in declaration order**, in the units the model declared
them in. The parameter's `to_si` factor is not applied before the request: APORIA asks the question in
the units the scientist wrote, and the program is expected to answer in the units it was asked in.

There is no request id, no precision field and no instruction to vary anything. A program that
receives the same `x` twice is expected to give the same answer twice (see *Determinism*).

## What must come back

One line, terminated by `\n`, with one value per declared `output` in declaration order:

```json
{"y":[2.1],"steps":12}
```

| field | required | meaning |
|---|---|---|
| `y` | yes | one value per declared output. A wrong count is a protocol error and stops the run — APORIA does not guess which value was meant |
| `steps` | no | the work this answer cost, in whatever unit the program counts. Omitted means the program did not say, which is recorded as 0 rather than invented |

Numbers are written as the **shortest decimal string that reads back as the identical `f64`** — the
same encoding the archives use, so a value that crosses this boundary and lands in an archive is the
same bits in both places and replay can compare them exactly. `json.dumps` in Python and Rust's
`Display` for `f64` both produce this form; they agree digit for digit.

## Values that are not real numbers, and refusals

These are three different things and the contract keeps them apart:

| what happened | what to write | what APORIA does |
|---|---|---|
| computed a value that left the reals | `{"y":["NaN"]}`, `"Infinity"`, `"-Infinity"` — **strings** | reads it as a value, raises the flag, and the divergence channel treats it as evidence about that region |
| declines to answer this point | `{"error":"matrix was singular"}` | stops asking, marks the run failed, keeps the report. A refusal is not a NaN |
| the point is unreachable by design | still `{"y":["NaN"]}` | as above. If the point is a fact about the model, say it with a value |

Non-finite values are strings because the bare JSON tokens `NaN` / `Infinity` are not valid JSON and
not what APORIA parses. A program that prints them is producing a framing error, not a value — which
is exactly what `beam.py` exists to demonstrate, since Python's serialiser emits the invalid form by
default.

A refusal voids the run rather than becoming a missing point because APORIA cannot distinguish "this
region is unreachable" from "my build is broken". Letting a campaign silently fill a map with
unanswered points is how a measurement becomes a fiction.

## Units and dimensions

The model's declared units travel with the parameters and outputs; nothing converts them for you. If
a model declares `output deflection : mm` and the program solves in metres, the program converts.
APORIA checks that a *model* is dimensionally self-consistent, and has no way to check that a foreign
program agrees — which is why the example programs state their units in their own comments, and why an
archive records the model text next to the answers.

## Process lifecycle

- **Started once, reused for the whole run.** A fresh process per evaluation would cost more than the
  answer and would make the budget a measure of OS speed rather than of the model.
- **Requests are strictly sequential**, one in flight at a time, in the order the campaign produced
  them. Point *n* is answered by the program's *n*th reply, which is what makes a run replayable.
- **The environment is the configuration channel.** The child inherits APORIA's own environment, and a
  caller building the spec in code can add pairs with `ProgramSpec::with_env` — which is how the tests
  select a failure mode through `APORIA_PROGRAM_MODE`. The command line exposes `--program` and
  `--timeout`; it does not expose per-variable flags, so a program's settings come from how it was
  launched, not from the protocol. The protocol itself carries parameters and nothing else.
- **stdout is the protocol and nothing else.** Diagnostics, banners and logs belong on stderr, which
  APORIA leaves attached to the terminal it was launched from. A library that introduces itself on
  stdout will be read as a malformed answer — the most common real-world mistake, and the reason both
  example programs have a `banner` mode.
- **Flush after every answer.** stdout is block-buffered when it is a pipe. An unflushed answer is
  indistinguishable from a program that is thinking, and the caller's timeout will end the run.
- **EOF is the normal end.** When APORIA is finished it closes the pipe, and a program should exit when
  its input ends rather than waiting to be told. It gets the chance to finish what that ending started:
  `Program::stop` closes stdin, waits up to 250 ms for the process to exit on its own, and only then
  kills it. Without that grace a program that checkpoints, flushes a log or releases a licence at EOF
  was killed mid-finalize — measured, with a probe whose end-of-input tally never reached the file it
  had already opened.

## Timeouts

`--timeout MS` (default 5000) bounds **one answer**, not the run. A program that exceeds it is stopped:
its process is killed, the run is marked failed with `the program did not answer within MS ms and was
stopped`, and the report is still written, because where a run stopped is information. Every point
after the failure is a non-answer, and `verdict` says so instead of presenting a partial map as a
result.

One limitation, stated rather than discovered later: stopping kills the process APORIA started, not a
process tree. A program that forks workers leaves them running, on every platform.

## Determinism

The oracle that shrinks a counterexample, the replay that checks an archive, and the calibration that
fits a channel's scale all assume one thing: **the same `x` produces the same `y`**. A program with
iterative refinement, a random seed, a thread-count-dependent reduction order, or a mutable cache
breaks that assumption, and APORIA will report a boundary that its own archive cannot reproduce.

If a program must be non-deterministic, give it a way to be deterministic on request and pin that in
the archive's recorded command line.

## How many requests occur

Between one and the configured budget, in the order the campaign produced them. Nothing about the
count is promised: measured on `software/examples/beam.ap` with a 40-evaluation budget, the program
received exactly 40 requests and no point twice — but a run that fails early asks fewer, a different
strategy or seed asks different points, and refinement is free to revisit a coordinate. **A program
must not depend on how many times it is asked, or on being asked in a particular order beyond the
sequence it sees.**

`aporia-bench explain` prints what a run actually executed, including probe and swap counts, so the
number of questions can be seen rather than guessed.

## How a program indicates failure

`{"error": "…"}`. Quote it, do not encode it: it is the program's own statement, and APORIA passes it
through as the reason the run stopped rather than paraphrasing it into a code.

## What APORIA cannot do with a foreign model

Two capability questions are answered by the boundary itself rather than assumed:

- **No precision variation.** Asking a program the same question at `f32` would measure whether it
  ignores a field it was never sent, and record that as numerical evidence. The Numerical channel stays
  silent for program-executed models.
- **No independent reference path.** `aporia_numerics::reference` re-evaluates A-IR instructions in
  double-double arithmetic. A foreign model has no A-IR instructions, so the Differential channel has
  nothing to disagree with. Both channels fire normally for interpreted models.

Everything else — the declared rules, divergence, sensitivity probes, symmetry swaps, calibration,
fusion, the atlas, findings, archives and the report — runs identically for a foreign model, with
nothing special-cased. `software/crates/aporia-adapter/tests/python_program.rs` asserts that a program
written in a language sharing no code with APORIA satisfies all of it, and
`software/crates/aporia-search/src/campaign.rs` asserts that the analysis path is the same one.

## Writing one

Read `software/examples/beam.py`. It is 132 lines including the explanation, uses only its
standard library, and implements every failure mode in this contract on purpose so the tests can drive
them. To see the boundary from the outside:

```sh
aporia run software/examples/beam.ap --program "python3 software/examples/beam.py" --budget 60
```
