"use strict";
// Compare the pipeline's output with the exact values for the committed data.
// The mean of 2,4,4,4,5,5,7,9 is 5; the sample variance is 32/7.

const path = require("path");
const { summarize, readMeasurements } = require("./pipeline");

const EXACT_MEAN = 5;
const EXACT_SAMPLE_VARIANCE = 32 / 7;

const values = readMeasurements(path.join(__dirname, "measurements.csv"));
const s = summarize(values);
const out = {
  n: s.n,
  mean: s.mean,
  variance: s.variance,
  exact_mean: EXACT_MEAN,
  exact_sample_variance: EXACT_SAMPLE_VARIANCE,
  mean_error: s.mean - EXACT_MEAN,
  variance_error: s.variance - EXACT_SAMPLE_VARIANCE,
};
console.log(JSON.stringify(out, null, 2));