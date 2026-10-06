#!/usr/bin/env python3
"""An APORIA external program in Python: one line in, one line out.

This file exists to prove one thing about the boundary. Everything else in the repository that
speaks the adapter protocol is Rust — `aporia-example-solver`, which shares APORIA's own number
encoder, so getting the format right costs that program nothing. A program in another language gets
nothing from APORIA at all: it must read the request, produce the response and follow the rules of
the protocol (`aporia-adapter`'s `protocol` module is the normative statement, one JSON object per
line in each direction) from its own standard library. If *this* works, the boundary is a protocol
and not a Rust API wearing a pipe.

The arithmetic is a cantilever beam's tip deflection, deliberately trivial: positive up to 60 N of
load, negative beyond it. A model declaring `require deflection >= 0` therefore has a region for
APORIA to find, and the region is this program's behaviour rather than anything the A-IR computes.

Run it through the instrument, against the model file that sits beside it:

    aporia run software/examples/beam.ap --program "python3 software/examples/beam.py"

Stdlib only — `json`, `math`, `os`, `sys`. No dependency is installed to make this work, and none
should be added: the point is a boundary that holds with nothing on the other side of it but a
language.
"""

import json
import math
import os
import sys
import time


def deflect(load: float) -> float:
    """The beam. Up to 60 N the tip goes down; beyond it the model is in a regime it does not
    describe, and the sign flips — which is the finding APORIA is looking for."""
    return -1.4 if load > 60.0 else 2.1


def encode(value: float) -> object:
    """Write one f64 the way the protocol means it.

    This is the part the Rust example does not have to think about. `json.dumps` writes the bare
    tokens `NaN`, `Infinity` and `-Infinity`, which are *not* valid JSON and are not what APORIA
    parses: the protocol carries a non-finite value as a **string**, so that a reader of the archive
    can tell a reported value from a framing accident. Python will happily emit the invalid form, so
    the conversion is written here, on purpose, in the language that needs it.
    """
    if math.isnan(value):
        return "NaN"
    if math.isinf(value):
        return "Infinity" if value > 0 else "-Infinity"
    return value


def answer(y: list, steps: int = 12) -> None:
    """Print one response and hand the line to the reader.

    The flush is not politeness. stdout is block-buffered when it is a pipe, so a program that waits
    for its buffer to fill has answered nothing as far as APORIA is concerned, and the run ends on the
    caller's timeout instead of on this line. Every response goes out flushed or the protocol does not
    work.
    """
    print(json.dumps({"steps": steps, "y": [encode(v) for v in y]}), flush=True)


def refuse(reason: str) -> None:
    """Decline to answer. Distinct from answering NaN: a refusal voids the run, a NaN is a value."""
    print(json.dumps({"error": reason}), flush=True)


def main() -> None:
    mode = os.environ.get("APORIA_PROGRAM_MODE", "answer")
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            request = json.loads(line)
            x = request["x"]
        except (ValueError, KeyError, TypeError):
            # A malformed request is the caller's mistake, and the answer is to say so and keep
            # listening — this program has no business deciding what APORIA meant to ask.
            refuse("the request is not one parameter")
            continue

        if mode == "hang":
            # Never answer. The caller's timeout is the only thing that ends this, which is what the
            # test needs to observe: a boundary that cannot report a hang cannot report a slow solver.
            while True:
                time.sleep(3600)
        if mode == "exit" and state["n"] >= 1:
            sys.exit(3)
        if mode == "banner" and state["n"] == 0:
            # The most likely real-world mistake: a library that introduces itself on stdout. One line,
            # no answer, exactly as the Rust example behaves.
            print("# beam.py ready", flush=True)
            state["n"] += 1
            continue
        if mode == "garbage":
            print("deflection = 2.1 mm", flush=True)
            state["n"] += 1
            continue
        if mode == "arity":
            answer([1.0, 2.0], steps=0)
            state["n"] += 1
            continue
        if mode == "refuse":
            refuse("matrix was singular")
            state["n"] += 1
            continue
        if mode == "nan":
            answer([math.nan])
            state["n"] += 1
            continue

        answer([deflect(float(x[0]))])
        state["n"] += 1

    # EOF: the caller closed the pipe, so the run is over. This is where a real solver writes its log,
    # checkpoints its state or releases a licence — and it is the moment APORIA used to arrive at by
    # killing the process instead of waiting for it. `finalize` mode exists so
    # `tests/python_program.rs` can tell the two apart.
    if mode == "finalize":
        mark = os.environ.get("APORIA_PROGRAM_MARK")
        if mark:
            with open(mark, "w", encoding="utf-8") as handle:
                handle.write(json.dumps({"answered": state["n"], "finalized": True}))


state = {"n": 0}

if __name__ == "__main__":
    main()
