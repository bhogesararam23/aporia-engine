#!/usr/bin/env bash
# e13b-run.sh — measure every frozen arm of E1.3b, in the arm order the protocol lists.
#
# The command is the protocol's, not a command line's: `--plan` with `--arm`, no overrides, so the
# budgets, seeds, rates, entry list, masks and trust rules in the results documents are the ones that
# were frozen on 2026-10-09 and promoted with the corpus. `scripts/e1x-run.sh` ran E1.x the same way.
#
# Results go to the committed namespace, because a reported number has to be openable from there.
# Archives go under `benchmarks/archives/e1-3b`, and that sub-namespace is not a stylistic choice:
# `archive_name` names the entry, strategy, seed, mask and rule but not the experiment, so E1.3b's
# `full-current-trust` arm asks for the same three directories E1.x's committed `full` arm already
# fills, at a different largest budget. The runner refuses that (`cli::archive_namespace_guard`) rather
# than replacing evidence a published number is opened from, and the separate namespace is what the
# refusal tells the operator to do. Archives stay out of git, as every run's do: they are the bulk of a
# run and reproducible from the corpus plus the configuration each manifest records.
#
# Each arm prints its own exit state. An arm that fails stops the loop: a partial experiment is a
# different experiment, and the file that froze it says eight arms.
set -u
cd "$(dirname "$0")/.." || exit 1
bin=.scratch/e1x-bin/aporia-bench.exe
plan=benchmarks/protocols/e1-3b-trust-resolution.json
archives=benchmarks/archives/e1-3b
log=.scratch/e13b-run.log
: >"$log"

if [ ! -x "$bin" ]; then
  echo "no binary at $bin; run scripts/r.sh first" >&2
  exit 1
fi
mkdir -p "$archives" || exit 1
sha256sum "$bin" | cut -c1-64 | {
  read -r sum
  echo "binary sha256 $sum" | tee -a "$log"
}
echo "protocol $(sha256sum "$plan" | cut -c1-16)" | tee -a "$log"

for arm in full-current-trust full-min-channels-2 full-nonfinite-blocks-trust full-both-trust-rules \
           no-physical-current-trust no-physical-both-trust-rules \
           only-physical-current-trust only-physical-both-trust-rules; do
  echo "== $arm ==" | tee -a "$log"
  if "$bin" run --plan "$plan" --arm "$arm" --archive "$archives" >>"$log" 2>&1; then
    echo "$arm: ran" | tee -a "$log"
  else
    echo "$arm: FAILED (exit $?), stopping" | tee -a "$log"
    exit 1
  fi
done
echo "all eight arms ran" | tee -a "$log"
