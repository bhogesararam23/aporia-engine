"use strict";
// The tests the project has: they check shape and the mean, not the variance estimator.

const test = require("node:test");
const assert = require("node:assert");
const { summarize } = require("../pipeline");

test("summarize returns numbers", () => {
  const s = summarize([1, 2, 3]);
  assert.strictEqual(typeof s.mean, "number");
  assert.strictEqual(typeof s.variance, "number");
});

test("the mean of 1, 2, 3 is 2", () => {
  assert.strictEqual(summarize([1, 2, 3]).mean, 2);
});