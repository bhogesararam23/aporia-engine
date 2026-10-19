#!/usr/bin/env bash
# e14-run.sh — run E1.4's four frozen arms, in the order the freeze lists them, with no overrides.
#
# The protocol file is the definition. Every flag that shapes what is measured is refused by the runner
# beside --plan, so this script passes --plan, --arm, --out and --archive and nothing else: the budgets,
# the seeds, the grid, the rates, the certification bar and each arm's mask, trust rule and resolution
# rule all come from the frozen bytes. The protocol digest is printed with the binary's sha256 so the
# log names both the code that measured and the pre-registration it followed.
#
# Archives go under benchmarks/archives/e1-4, which is gitignored: they are reproducible from the corpus
# plus the configuration each manifest records, and they carry the measurement identity of the run that
# wrote them. Results documents go to benchmarks/results, which is not, because those are the published
# numbers.
set -eu
cd "$(dirname "$0")/.." || exit 1
source scripts/dev-env.sh || exit 1
BIN=".scratch/e1x-bin/aporia-bench.exe"
PLAN="benchmarks/protocols/e1-4-resolution-demand.json"
ARMS="A-current B-demand C-withhold D-demand-and-withhold"
[ -x "$BIN" ] || { echo "no permitted binary at $BIN: run scripts/r.sh" >&2; exit 2; }

echo "binary  sha256 $(sha256sum "$BIN" | cut -d' ' -f1)"
echo "plan    $PLAN sha256 $(sha256sum "$PLAN" | cut -d' ' -f1)"
echo "commit  $(git rev-parse --short HEAD)  tree: $(git status --porcelain | wc -l) path(s) differ"
echo

for arm in $ARMS; do
  echo "=== $arm"
  # --archive is per arm because the arms differ in the axis their archive directories name; the
  # namespace guard inside the runner is what stops one arm's bytes landing in another's directory,
  # and it runs before a campaign starts.
  "$BIN" run --plan "$PLAN" --arm "$arm" \
      --out benchmarks/results \
      --archive benchmarks/archives/e1-4
  echo
done
