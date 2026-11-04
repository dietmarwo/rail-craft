"""Optimization with SciPy: vectorized differential evolution through `score_matrix`.

Tune the two dispatch weights for a 16-train corridor with a 15-tick maintenance closure.
`differential_evolution(vectorized=True)` hands the objective a whole population as a
(2, S) array; one `score_matrix` call scores it on two native workers without JSON. Rows
that are not complete would receive a large penalty. The result is compared with a dense
grid search and checked on a held-out closure scenario.
"""

import numpy as np
from scipy.optimize import differential_evolution

from corridor import corridor
from rail_craft import Prepared

PENALTY = 1e6
training = Prepared(corridor(trains=16, closures=[(20, 35)]))
held_out = Prepared(corridor(trains=16, closures=[(40, 52)]))
calls = rows = failures = 0


def objective(population):
    """Score a (2, S) population; incomplete rows get PENALTY."""
    global calls, rows, failures
    scores, statuses, _ = training.score_matrix(population.T, workers=2)
    calls += 1
    rows += len(statuses)
    scores = np.array(scores)
    failed = np.array([s != "complete" for s in statuses])
    failures += int(failed.sum())
    return np.where(failed, PENALTY, scores)


result = differential_evolution(objective, bounds=[(0, 10), (0, 10)], vectorized=True,
                                updating="deferred", popsize=12, maxiter=40, tol=0, seed=11,
                                polish=False)
print(f"differential evolution: weighted delay {result.fun:.0f} at "
      f"[{result.x[0]:.3f}, {result.x[1]:.3f}]")
print(f"  {result.nit} generations, {calls} native calls scored {rows} rows, {failures} penalized; "
      f"{result.message}")

# 61 x 61 = 3,721 rows fit in one call (the limit is 4,096 rows).
grid = np.array([[p, a] for p in np.linspace(0, 10, 61) for a in np.linspace(0, 10, 61)])
scores, _, _ = training.score_matrix(grid, workers=2)
best = int(np.argmin(scores))
print(f"61 x 61 grid search:    weighted delay {scores[best]:.0f} at {grid[best].tolist()}")

checks = [result.x.tolist(), grid[best].tolist(), [0, 1], [1, 0]]
held, _, _ = held_out.score_matrix(checks)
print("\nheld-out closure [40, 52)")
for label, weights, score in zip(("DE result", "grid best", "FIFO", "priority only"),
                                 checks, held, strict=True):
    print(f"  {label:13s} [{weights[0]:6.3f}, {weights[1]:6.3f}] -> {score:.0f}")

replay = training.evaluate(result.x.tolist(), trace=True)
assert replay["status"] == "complete" and replay["score"] == result.fun, replay
print(f"\ntraced replay agrees: score {replay['score']:.0f}, steps {replay['steps']}, "
      f"max delay {replay['metrics']['max_delay']}")
