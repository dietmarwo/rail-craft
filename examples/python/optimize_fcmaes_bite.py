"""Optimization with fcmaes: BiteOpt with failure penalties for deadlocking policies.

A dispatch policy must serve two lines: the 16-train corridor, and a single-track line with
one passing loop where the greedy dispatcher deadlocks if it lets opposing trains fill both
blocks and both loop tracks. `fcmaes.bitecpp.minimize` calls a scalar objective; each call
scores one row on both lines with `score_matrix`. A deadlock (status `infeasible`) gets a
penalty, so the search must trade corridor lateness for a policy that never deadlocks.
"""

import numpy as np
from loguru import logger
from scipy.optimize import Bounds

from fcmaes import bitecpp
from corridor import corridor, passing_loop
from rail_craft import Prepared

logger.remove()
PENALTY = 1e5
line = Prepared(corridor(trains=16))
loop = Prepared(passing_loop())
calls = deadlocks = 0


def objective(x):
    global calls, deadlocks
    calls += 1
    total = 0.0
    for prepared in (line, loop):
        scores, statuses, _ = prepared.score_matrix([x])
        if statuses[0] != "complete":
            deadlocks += 1
            return PENALTY + total
        total += scores[0]
    return total


result = bitecpp.minimize(objective, bounds=Bounds([0.0, 0.0], [10.0, 10.0]), x0=[5.0, 5.0],
                          max_evaluations=1500, rg=np.random.Generator(np.random.PCG64DXSM(3)))
x = result.x
print(f"BiteOpt: combined weighted delay {result.fun:.0f} at [{x[0]:.3f}, {x[1]:.3f}] "
      f"({calls} calls, {deadlocks} penalized deadlocks)")
for label, prepared in (("corridor", line), ("passing loop", loop)):
    row = prepared.evaluate(list(x))
    print(f"  {label:12s} status {row['status']}, score {row['score']:.0f}")

corridor_best = [4.0, 0.25]
row = loop.evaluate(corridor_best, trace=True)
print(f"\ncorridor-only optimum {corridor_best}: corridor score "
      f"{line.evaluate(corridor_best)['score']:.0f}, passing loop {row['status']} ({row['detail']})")
names = [t["name"] for t in passing_loop()["trains"]]
print(f"  deadlock at tick {row['metrics']['time']} after {row['metrics']['completed']} arrivals; held:")
for held in row["metrics"]["active"]:
    print(f"    {names[held['train']]} holds resource {held['resource']} (leg {held['leg']})")
