#!/usr/bin/env bash
# e1x-run.sh — measure every frozen arm of e1-geometry, in the arm order the protocol lists.
#
# The command is the protocol's, not a command line's: `--plan` with `--arm`, no overrides, so the
# budgets, seeds, rates, entry list and masks in the results documents are the ones that were frozen
# before the corpus existed. Archives and results go to the committed namespaces, which is where a
# reported number has to be openable from later.
#
# Each arm prints its own exit state. An arm that fails stops the loop: a partial experiment is a
# different experiment, and the file that froze it says eleven arms.
set -u
cd "$(dirname "$0")/.." || exit 1
bin=.scratch/e1x-bin/aporia-bench.exe
plan=benchmarks/protocols/e1-geometry.json
log=.scratch/e1x-run.log
: >"$log"

for arm in full no-behavioral no-numerical no-differential no-sensitivity no-physical \
           only-behavioral only-physical only-numerical only-differential only-sensitivity; do
  echo "== $arm ==" | tee -a "$log"
  if "$bin" run --plan "$plan" --arm "$arm" >>"$log" 2>&1; then
    echo "$arm: ran" | tee -a "$log"
  else
    echo "$arm: FAILED (exit $?), stopping" | tee -a "$log"
    exit 1
  fi
done
echo "all eleven arms ran" | tee -a "$log"
