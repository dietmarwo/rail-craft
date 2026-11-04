"""Application: how much does a maintenance closure cost, and which policy copes best?

Scenario comparison on a 16-train corridor. A closure of the central block starts at tick
12 and lasts 0 to 30 ticks; one further scenario slows every central-block run by one tick.
Four dispatch policies are evaluated in each scenario with full records, so the study can
report lateness, the worst delay and each policy's regret against the scenario's best. A
traced replay then shows central-block occupancy around the longest closure, and a short
horizon shows how an unfinished schedule is classified.
"""

from corridor import CENTRAL, corridor
from rail_craft import Prepared

policies = {"fifo": [0, 1], "priority": [1, 0], "balanced": [4, 1], "tuned": [4, 0.25]}
scenarios = {
    "no closure": corridor(trains=16),
    "closure 10": corridor(trains=16, closures=[(12, 22)]),
    "closure 20": corridor(trains=16, closures=[(12, 32)]),
    "closure 30": corridor(trains=16, closures=[(12, 42)]),
    "slow block": corridor(trains=16, block_run=5),
}

print(f"{'scenario':12s} {'policy':9s} {'weighted':>9s} {'max':>4s} {'mean':>6s} {'regret':>7s}")
totals = dict.fromkeys(policies, 0.0)
for label, model in scenarios.items():
    prepared = Prepared(model)
    candidates = [{"id": name, "parameters": w} for name, w in policies.items()]
    records = prepared.batch(candidates, workers=2)["records"]
    assert all(r["status"] == "complete" for r in records), records
    best = min(r["score"] for r in records)
    for r in records:
        delays = r["metrics"]["delays"]
        totals[r["id"]] += r["score"]
        print(f"{label:12s} {r['id']:9s} {r['score']:9.0f} {r['metrics']['max_delay']:4d} "
              f"{sum(delays) / len(delays):6.2f} {r['score'] - best:7.0f}")
print("\ntotal weighted delay over all scenarios")
for name, total in sorted(totals.items(), key=lambda item: item[1]):
    print(f"  {name:9s} {total:7.0f}")

# Central-block occupancy for the tuned policy around the 30-tick closure.
replay = Prepared(scenarios["closure 30"]).evaluate(policies["tuned"], trace=True)
names = [t["name"] for t in scenarios["closure 30"]["trains"]]
rows = sorted((i for i in replay["metrics"]["intervals"] if i["resource"] == CENTRAL),
              key=lambda i: i["entry"])
print("\ncentral block, closure [12, 42), tuned policy ('=' service, '-' held after service)")
for i in rows[:10]:
    bar = " " * i["entry"] + "=" * (i["service_end"] - i["entry"]) + "-" * (i["exit"] - i["service_end"])
    print(f"  {names[i['train']]} {i['entry']:3d}-{i['exit']:3d} |{bar}")

# A horizon shorter than the makespan leaves the schedule unfinished.
short = Prepared(corridor(trains=16, horizon=60))
row = short.evaluate(policies["tuned"], trace=True)
print(f"\nhorizon 60: status {row['status']}, detail {row['detail']}, "
      f"completed {row['metrics']['completed']} of 16 by tick {row['metrics']['time']}, "
      f"{len(row['metrics']['active'])} trains still holding a resource")
