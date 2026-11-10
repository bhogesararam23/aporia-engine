"""Measure the error of the integration rule as the grid is refined.

Prints one JSON document: the estimate, the exact value and the error for each grid size, plus
the observed convergence order between consecutive grids.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from rule import exact_integral, integrate, samples_for  # noqa: E402


def run(parameters: dict) -> dict:
    x0 = float(parameters["x0"])
    x1 = float(parameters["x1"])
    exact = exact_integral(x0, x1)
    rows = []
    previous = None
    for n in (5, 9, 17, 33):
        samples = samples_for(x0, x1, n, lambda x: x * x)
        estimate = integrate(samples)
        error = estimate - exact
        order = None
        if previous is not None and error != 0 and previous[1] != 0:
            order = abs((previous[1] / error)) if previous[1] != 0 else None
        rows.append(
            {
                "n": n,
                "estimate": estimate,
                "exact": exact,
                "error": error,
                "observed_order": order,
            }
        )
        previous = (n, error)
    return {"rows": rows, "exact": exact}


def main() -> int:
    parameters = json.loads(Path(__file__).resolve().parent.joinpath("params.json").read_text())
    print(json.dumps(run(parameters), indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
