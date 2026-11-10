"""The tests this project has.

They check that the rule runs and that the estimate moves in the right direction. They do not
check that the estimate is *right* -- which is the gap an investigation is expected to find.
"""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from rule import integrate, samples_for  # noqa: E402


def test_integrate_returns_a_number():
    estimate = integrate(samples_for(0.0, 1.0, 5, lambda x: x))
    assert isinstance(estimate, float)


def test_a_larger_grid_gives_a_different_estimate():
    small = integrate(samples_for(0.0, 1.0, 5, lambda x: x * x))
    large = integrate(samples_for(0.0, 1.0, 33, lambda x: x * x))
    assert small != large


def test_integrate_refuses_a_single_sample():
    try:
        integrate([(0.0, 0.0)])
    except Exception:
        return
    raise AssertionError("a single sample should not be accepted")


if __name__ == "__main__":
    for name, function in sorted(globals().items()):
        if name.startswith("test_") and callable(function):
            function()
    print("ok")
