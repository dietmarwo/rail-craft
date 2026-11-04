"""Optimization with fcmaes: CMA-ES ask/tell over whole populations.

Find dispatch weights that minimize the mean weighted delay over three closure scenarios
of a 20-train corridor. `fcmaes.cmaes.Cmaes` proposes a population with `ask()`; each
scenario scores the whole population in one `score_matrix` call on two native workers,
and `tell()` receives the mean. Incomplete rows would be penalized.
"""

import numpy as np
from loguru import logger
from scipy.optimize import Bounds

from fcmaes import cmaes
from corridor import corridor
from rail_craft import Prepared

logger.remove()
PENALTY = 1e6
scenarios = [Prepared(corridor(trains=20, closures=c)) for c in ([(10, 20)], [(30, 45)], [(55, 60)])]
bounds = Bounds([0.0, 0.0], [10.0, 10.0])
es = cmaes.Cmaes(bounds=bounds, x0=np.array([5.0, 5.0]), input_sigma=2.5, popsize=16,
                 rg=np.random.Generator(np.random.PCG64DXSM(5)))


def population_score(xs):
    total = np.zeros(len(xs))
    for prepared in scenarios:
        scores, statuses, _ = prepared.score_matrix(xs, workers=2)
        ok = np.array([s == "complete" for s in statuses])
        total += np.where(ok, scores, PENALTY)
    return total / len(scenarios)


best_y, best_x, generations = np.inf, None, 0
for generations in range(1, 31):
    xs = es.ask()
    ys = population_score(xs)
    if ys.min() < best_y:
        best_y, best_x = float(ys.min()), xs[int(ys.argmin())]
    if es.tell(ys) != 0:
        break
print(f"CMA-ES: mean weighted delay {best_y:.2f} at [{best_x[0]:.3f}, {best_x[1]:.3f}] "
      f"after {generations} generations ({generations * 16} policies x {len(scenarios)} scenarios)")

reference = population_score(np.array([[0, 1], [1, 0], [1, 1]]))
for label, value in zip(("FIFO [0, 1]", "priority only [1, 0]", "equal [1, 1]"), reference, strict=True):
    print(f"  {label:22s} mean weighted delay {value:.2f}")
per_scenario = [p.score_matrix([best_x])[0][0] for p in scenarios]
print(f"  best policy per scenario: {per_scenario}")
