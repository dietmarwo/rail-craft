"""Application: which corridor parameters drive lateness?

One-at-a-time sensitivity study around a 16-train base case. For every variant the best
of 121 dispatch policies (an 11 x 11 weight grid) is found with one `score_matrix` call,
so each row compares the best achievable weighted delay, not one fixed policy. Demand
(number of trains), headway between train pairs, central-block run time and clearance are
varied in turn, and the study reports the change against the base case.
"""

import numpy as np

from corridor import corridor
from rail_craft import Prepared

grid = np.array([[p, a] for p in np.linspace(0, 10, 11) for a in np.linspace(0, 10, 11)])
base = {"trains": 16, "headway": 3, "block_run": 4, "clearance": 1}


def best_policy(**changes):
    prepared = Prepared(corridor(**(base | changes)))
    scores, statuses, _ = prepared.score_matrix(grid)
    scores = np.array(scores)
    failed = sum(s != "complete" for s in statuses)
    i = int(np.nanargmin(scores))
    return scores[i], grid[i], failed


reference, weights, _ = best_policy()
print(f"base case {base}: best weighted delay {reference:.0f} at weights {weights.tolist()}")
print(f"\n{'parameter':10s} {'value':>5s} {'best':>7s} {'change':>8s} {'weights':>14s} {'failed':>6s}")
for parameter, values in {"trains": (8, 12, 16, 20, 24), "headway": (2, 3, 4, 5, 6),
                          "block_run": (3, 4, 5, 6), "clearance": (0, 1, 2)}.items():
    for value in values:
        score, weights, failed = best_policy(**{parameter: value})
        change = (score - reference) / reference * 100
        print(f"{parameter:10s} {value:5d} {score:7.0f} {change:7.1f}% "
              f"[{weights[0]:4.1f}, {weights[1]:4.1f}] {failed:6d}")
