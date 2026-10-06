# Example programs

Files here are **not** part of APORIA. They are the other side of a boundary: programs that answer
what APORIA asks and know nothing about why it asked. Each one demonstrates a different property of
that boundary, which is the only reason to add a language to this repository.

| file | language | what it demonstrates |
|---|---|---|
| `beam.py` | Python 3, standard library only | a program that shares no code with APORIA still satisfies the protocol, including the number encoding APORIA's own Rust example gets for free |
| `beam.ap` | Aporia DSL | a model whose output has no computing instruction — the declaration the programs above answer |

The Rust example program lives with the crate that owns the boundary,
`software/crates/aporia-adapter/src/bin/example_solver.rs`, because it is also the fixture that
crate's tests drive.

## Running one

```sh
cargo build --release -p aporia-cli
./software/target/release/aporia run software/examples/beam.ap \
    --program "python3 software/examples/beam.py" --budget 60
```

On Windows, `python3` may be a Store alias that does not execute; `python` or `py -3` will. Replace
the program with the Rust one to compare the two answers:

```sh
aporia run software/examples/beam.ap \
    --program "target/release/aporia-example-solver.exe" --budget 60
```

Both produce the same Trust Atlas, because both produce the same bytes. That is the property
`software/crates/aporia-adapter/tests/python_program.rs` asserts, and the contract behind it is
[`../../documentation/adapter-protocol.md`](../../documentation/adapter-protocol.md).

## What an example program has to get right

Nothing here is APORIA-specific plumbing. It is one line in, one line out, and four rules:

1. Read a JSON object per request, answer with one per response, **flush after every answer**.
   A pipe is block-buffered, so an unflushed answer is indistinguishable from a hang.
2. One value per declared output, in declaration order. A wrong count is a protocol error, and the
   run stops rather than guessing which value was meant.
3. A value that left the reals is the **string** `"NaN"`, `"Infinity"` or `"-Infinity"`. The bare JSON
   tokens are not the protocol, and a refusal (`{"error": …}`) is not a NaN either — a refusal voids
   the run.
4. Report your own work as `steps` if you know it. Absent means 0, and 0 is not nothing: it is what
   the program said.
