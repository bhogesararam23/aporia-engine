# agent-demo — a small project for APORIA to investigate

A compact numerical project with a real defect, weak tests, an executable experiment and a
recorded baseline. It exists so the agent's workflow can be demonstrated end to end on something
that is actually wrong.

## What is here

| file | what it is |
|---|---|
| `rule.py` | the trapezoidal integration rule, and the helpers that sample it |
| `params.json` | the parameters the experiment uses |
| `experiment.py` | the convergence sweep: one JSON document of estimates, errors and observed order |
| `tests/test_rule.py` | the tests the project has — they check that the rule runs, not that it is right |
| `baseline.json` | what this project produces today |

## The defect

`integrate` walks the samples and, on the last step, continues from the last sample back to the
first. On a grid over `[0, 1]` that adds a closing segment of `-0.5`, so the estimate converges to
`-1/6` instead of `1/3`, and the error approaches `-0.5` instead of shrinking.

The error is measurable: run the experiment and read `error` and `observed_order`. A correct
trapezoid rule has an error that falls like the square of the step size — about `0.0083` at
`n = 9` and `0.0005` at `n = 33`.

Nothing in `tests/` notices: the tests check that the function returns a number and that the
estimate changes with the grid. No test compares the estimate with the exact integral, and no
test checks the convergence order. That is the gap an investigation should find.

## Running it by hand

```sh
python experiment.py            # the sweep, as JSON
python tests/test_rule.py      # the tests, as they are
```

## What a good investigation looks like here

1. inspect the project and notice that the tests never compare against the exact integral;
2. run the experiment and read the error: it is not shrinking;
3. propose a convergence experiment as an experiment with a hypothesis, a baseline and a
   success criterion — not as a sentence in a report;
4. change `integrate` so it stops closing the loop, add a test that compares against the exact
   integral, run the tests and the sweep again;
5. compare the before and after numbers, and record what is still unmeasured — the rule's
   behaviour on non-uniform grids is still unknown.
