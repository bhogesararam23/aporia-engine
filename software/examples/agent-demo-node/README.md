# agent-demo-node — a small data pipeline for APORIA to investigate

A compact Node.js statistics project with a real defect, weak tests, an executable experiment
and a documented expectation. It exists so the agent's workflow can be demonstrated end to end
on a project type materially different from the Python numerical demo: a data pipeline with an
estimator bias, not an integration rule.

## What is here

| file | what it is |
|---|---|
| `pipeline.js` | the measurement pipeline: mean and variance of a CSV column |
| `measurements.csv` | the data: eight measurements with a known mean and sample variance |
| `experiment.js` | the check: compares the pipeline's output with the exact values |
| `tests/test_pipeline.js` | the tests the project has — they check shape and the mean, not the variance |
| `baseline.json` | what this project produces today |

## The defect

The project documents the **sample** variance (the `n - 1` estimator). `summarize` divides the
sum of squared deviations by `n` — the population estimator. On the committed data
(2, 4, 4, 4, 5, 5, 7, 9) the mean is exactly 5 and the sample variance is exactly 32/7
(about 4.571428571). The pipeline reports variance 4, so `variance_error` is about -0.571428571.

Nothing in `tests/` notices: the tests check that the output is numeric and that the mean of
1, 2, 3 is 2. No test compares the variance with the known sample variance. That is the gap an
investigation should find.

## Running it by hand

```sh
node experiment.js                     # the check, as JSON
node --test tests/test_pipeline.js     # the tests, as they are
```

## What a good investigation looks like here

1. inspect the pipeline and notice the divisor is `n`, while the README documents `n - 1`;
2. run the experiment and read `variance_error`: it is not zero;
3. fix the divisor in the workspace, run the experiment again, and run the project's tests;
4. compare the before and after numbers, and record what is still unmeasured — how the two
   estimators differ as the sample size grows is a genuinely open question, kept in the backlog.