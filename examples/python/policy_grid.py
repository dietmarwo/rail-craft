"""Application: which dispatch weights minimize priority-weighted lateness?

Parameter study over the two dispatch weights on a 16-train corridor that shares one
central block. All 1,681 grid policies are scored in one `score_matrix` call (no JSON
records) with two native workers. The study then checks how strongly the outcome depends
on the ratio of the priority weight to the waiting-age weight, and replays the best policy
with a full trace to report per-train delays.
"""

from time import perf_counter

import numpy as np

from corridor import corridor
from rail_craft import Prepared

model = corridor(trains=16)
prepared = Prepared(model)
steps = 41
weights = np.linspace(0.0, 10.0, steps)
grid = np.array([[p, a] for p in weights for a in weights])

start = perf_counter()
scores, statuses, work = prepared.score_matrix(grid, workers=2)
elapsed = perf_counter() - start
scores = np.array(scores)
assert set(statuses) == {"complete"}, set(statuses)
print(f"scored {len(grid)} policies in {elapsed * 1e3:.1f} ms "
      f"({elapsed / len(grid) * 1e6:.2f} us per policy), {sum(work)} logical steps")

best = int(np.argmin(scores))
reference = {"FIFO by waiting age [0, 1]": [0, 1], "priority only [1, 0]": [1, 0],
             "equal weights [1, 1]": [1, 1], "grid best": grid[best].tolist()}
named, _, _ = prepared.score_matrix([w for w in reference.values()])
print("\npolicy                          weights        weighted delay")
for (label, w), score in zip(reference.items(), named, strict=True):
    print(f"{label:30s}  [{w[0]:4.2f}, {w[1]:4.2f}]  {score:8.0f}")

# The dispatcher ranks priority_weight * priority + age_weight * wait, so scaling both
# weights by the same factor rarely changes a decision. Group policies by their ratio.
print("\nscores by priority/age ratio, rounded to 0.1 (age weight > 0)")
ratios = {}
for (p, a), score in zip(grid, scores, strict=True):
    if a > 0:
        key = round(p / a, 1)
        ratios.setdefault(key, []).append(score)
for key in (0.0, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0, 40.0):
    if key in ratios:
        values = ratios[key]
        print(f"  ratio {key:5.1f}: {len(values):3d} policies, min {min(values):6.0f}, "
              f"max {max(values):6.0f}")

# Coarse map: one character per 1.25 x 1.25 cell, darker means larger weighted delay.
shades = " .:-=+*#%@"
low, high = scores.min(), scores.max()
table = scores.reshape(steps, steps)
print("\nscore map (rows: priority weight 0..10, columns: age weight 0..10)")
for i in range(0, steps, 5):
    cells = "".join(shades[min(9, int((table[i, j] - low) / (high - low + 1e-9) * 10))]
                    for j in range(0, steps, 2))
    print(f"  p={weights[i]:5.2f} |{cells}|")

replay = prepared.evaluate(grid[best].tolist(), trace=True)
assert replay["status"] == "complete", replay
delays = replay["metrics"]["delays"]
late = [(t["name"], t["priority"], d) for t, d in zip(model["trains"], delays, strict=True) if d]
print(f"\nreplay of grid best: score {replay['score']:.0f}, max delay {replay['metrics']['max_delay']}, "
      f"{len(late)} of {len(delays)} trains late")
for name, priority, delay in late:
    print(f"  {name} priority {priority}: {delay} ticks late")
