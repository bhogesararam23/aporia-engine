#!/usr/bin/env bash
# e15-run.sh — run E1.5's four frozen arms, in the order the freeze lists them, with no overrides.
#
# The protocol file is the definition. The runner refuses every flag that shapes what is measured beside
# --plan, so this script passes --plan, --arm, --out and --archive and nothing else: the budgets, the
# seeds, the grid, the rates, the certification bars and each arm's mask, trust rule, resolution rule and
# adequacy criterion all come from the frozen bytes. The log prints the protocol digest beside the
# binary's sha256 and the commit it was built from, so a reader can tell which code measured and which
# pre-registration it followed.
#
# Archives go under benchmarks/archives/e1-5, which is gitignored: they are reproducible from the corpus
# plus the configuration each manifest records, and every one of them now carries the measurement identity
# of the run that wrote it. Results documents go to benchmarks/results, which is not ignored, because
# those are the published numbers.
#
# The binary is the one scripts/r.sh kept and the reproduction gate certified. If it is missing, or if
# anything under crates/ has moved since it was linked, stop: a stale executable is not a cheaper run, it
# is a different instrument.
set -eu
cd "$(dirname "$0")/.." || exit 1
source scripts/dev-env.sh || exit 1
BIN=".scratch/e1x-bin/aporia-bench.exe"
PLAN="benchmarks/protocols/e1-5-spatial-uncertainty-cost.json"
ARMS="A-current B-size C-density D-density-and-demand"
[ -x "$BIN" ] || { echo "no permitted binary at $BIN: run scripts/r.sh" >&2; exit 2; }

# What the binary was built from, and whether anything that could change a measurement has moved since.
BUILT_FROM="$(git log -1 --format=%h -- crates/ software/Cargo.lock software/crates 2>/dev/null || echo unknown)"
DIRTY="$(git status --porcelain -- crates/ | wc -l)"
echo "binary  sha256 $(sha256sum "$BIN" | cut -d' ' -f1)"
echo "crates  last committed at $BUILT_FROM, $DIRTY path(s) differ from HEAD"
echo "plan    $PLAN sha256 $(sha256sum "$PLAN" | cut -d' ' -f1)"
echo "commit  $(git rev-parse --short HEAD)  tree: $(git status --porcelain | wc -l) path(s) differ"
if [ "$DIRTY" != "0" ]; then
  echo "crate sources differ from HEAD: rebuild with scripts/r.sh and re-run the gate before measuring" >&2
  exit 2
fi
echo

for arm in $ARMS; do
  echo "=== $arm"
  # --archive is per arm because the arms differ in the axes their archive directories name; the namespace
  # guard inside the runner is what stops one arm's bytes landing in another's directory, and it runs
  # before a campaign starts.
  "$BIN" run --plan "$PLAN" --arm "$arm" \
      --out benchmarks/results \
      --archive benchmarks/archives/e1-5
  echo
done
