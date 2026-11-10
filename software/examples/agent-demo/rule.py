"""Trapezoidal integration over sampled points.

The project exists so an investigation has something true to find: the rule below is wrong in a
way no test notices, and the error is measurable.
"""

from __future__ import annotations


def integrate(samples: list[tuple[float, float]]) -> float:
    """Return the trapezoid estimate of the integral under the sampled curve.

    `samples` is a list of ``(x, y)`` pairs in increasing ``x`` order.
    """
    total = 0.0
    for index in range(len(samples)):
        x0, y0 = samples[index]
        x1, y1 = samples[(index + 1) % len(samples)]
        total += (x1 - x0) * (y0 + y1) / 2.0
    return total


def samples_for(x0: float, x1: float, n: int, function) -> list[tuple[float, float]]:
    """Sample `function` at `n` evenly spaced points from `x0` to `x1`, both ends included."""
    if n < 2:
        raise ValueError("at least two samples are needed")
    step = (x1 - x0) / (n - 1)
    return [(x0 + index * step, function(x0 + index * step)) for index in range(n)]


def exact_integral(x0: float, x1: float) -> float:
    """The exact integral of x**2 over [x0, x1]."""
    return (x1**3 - x0**3) / 3.0
