"use strict";
// The measurement pipeline: mean and variance of a CSV column.
// The project documents the SAMPLE variance (n - 1). The divisor below is the defect.

const fs = require("fs");
const path = require("path");

function summarize(values) {
  const n = values.length;
  if (n === 0) throw new Error("no measurements");
  const mean = values.reduce((a, b) => a + b, 0) / n;
  let variance = 0;
  for (const v of values) variance += (v - mean) ** 2;
  variance /= n;
  return { n, mean, variance };
}

function readMeasurements(file) {
  const text = fs.readFileSync(file, "utf8");
  return text
    .split(/\r?\n/)
    .slice(1)
    .filter((line) => line.trim() !== "")
    .map(Number);
}

function main() {
  const values = readMeasurements(path.join(__dirname, "measurements.csv"));
  const summary = summarize(values);
  fs.writeFileSync(
    path.join(__dirname, "summary.json"),
    JSON.stringify(summary, null, 2) + "\n"
  );
  console.log(JSON.stringify(summary));
}

if (require.main === module) main();
module.exports = { summarize, readMeasurements };