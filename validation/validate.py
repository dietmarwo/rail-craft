import itertools
import json
import random
import subprocess
import sys
from pathlib import Path
from tempfile import TemporaryDirectory

from tools import good, run, save

binary = sys.argv[1]
single = {"resources": [{"name": "block", "clearance": 1}], "horizon": 40,
          "trains": [{"name": f"T{i}", "release": 0, "planned": 3, "priority": p,
                      "route": [{"resource": 0, "duration": 3}]} for i,p in enumerate([0, 1, 3])]}
rows = run(binary, single, [[0,0],[1,0]], trace=True)
assert [d["entry"] for d in good(rows[0])["intervals"]] == [0,4,8]
assert [d["train"] for d in good(rows[0])["intervals"]] == [0,1,2]
assert [d["train"] for d in good(rows[1])["intervals"]] == [2,1,0]
# Complete enumeration for simultaneous release and one non-idling resource.
costs = [sum((1 + single["trains"][i]["priority"]) * 4*j for j,i in enumerate(order))
         for order in itertools.permutations(range(3))]
assert rows[1]["score"] == min(costs)
tiny_refs = []
for n in range(3,7):
    model = single | {"trains":[single["trains"][0] | {"name":str(i),"priority":i} for i in range(n)]}
    score = run(binary, model, [[1,0]])[0]["score"]
    exact = min(sum((1+i)*4*j for j,i in enumerate(o)) for o in itertools.permutations(range(n)))
    assert score == exact
    tiny_refs.append({"trains":n,"orderings":len(list(itertools.permutations(range(n)))),"optimum":exact})
# Independent tick simulation for the compatible single-block model.
finish = []
slot = 0
for t in range(single["horizon"]):
    if slot < len(single["trains"]) and (not finish or finish[-1] + 1 == t):
        finish.append(t + 3)
        slot += 1
assert finish == [i["exit"] for i in good(rows[0])["intervals"]]

def verify(model, trace):
    with TemporaryDirectory() as temp:
        m, t = Path(temp)/"model.json", Path(temp)/"trace.json"
        m.write_text(json.dumps(model))
        t.write_text(json.dumps(trace))
        return subprocess.run([binary,"verify",m,t],capture_output=True,check=False).returncode

trace = good(rows[0])["intervals"]
assert verify(single, trace) == 0
assert verify(single, trace[:-1]) != 0
assert verify(single, [trace[0], trace[1] | {"entry": 3, "service_end": 6,"exit":6}, trace[2]]) != 0
assert verify(single, [trace[0] | {"exit": 2}, *trace[1:]]) != 0
assert verify(single, [trace[0] | {"service_end": 1}, *trace[1:]]) != 0
delayed = single | {"trains": [single["trains"][0] | {"release":1}, *single["trains"][1:]]}
assert verify(delayed, trace) != 0
corridor = {"horizon":100,"resources":[{"name":name,"clearance":1} for name in ["west siding","east siding","central block","east platform","west platform"]],
            "trains":[{"name":name,"release":0,"planned":10,"priority":p,
                       "route":[{"resource":r,"duration":d} for r,d in zip(route,[1,4,3],strict=True)]}
                      for name,p,route in [("eastbound",0,[0,2,3]),("westbound",4,[1,2,4])]],
            "closures":[{"resource":2,"start":1,"end":5}]}
nominal = run(binary,corridor,[[1,0]],trace=True)[0]
moves = good(nominal)["intervals"]
assert min(r["entry"] for r in moves if r["resource"]==2)==5
assert verify(corridor,moves)==0
bad = corridor | {"trains":[t | {"route":t["route"][:-1]+[{"resource":1-i,"duration":3}]}
                             for i,t in enumerate(corridor["trains"])]}
assert run(binary,bad,[[1,0]])[0]["detail"] == "deadlock"
def scenarios(seed):
    rng=random.Random(seed)
    return [corridor | {"closures":[{"resource":2,"start":1,"end":rng.randrange(3,9)}]}
            for _ in range(6)]
policies=[[0,0],[0,1],[1,0],[1,1],[2,1],[5,2]]
def study(seed):
    return list(zip(*(run(binary,m,policies) for m in scenarios(seed)),strict=True))
training=study(17)
winner=min(range(len(policies)),key=lambda i:sum(r["score"] for r in training[i]))
hold=study(29)
assert sum(r["score"] for r in hold[winner]) < sum(r["score"] for r in hold[0])
save({"model":corridor,"parameters":[1,0]}, {"baseline_delay":rows[0]["score"],"exact_delay":min(costs),
     "tiny_references":tiny_refs,"policy_candidates":len(policies),"winner":policies[winner],
     "holdout_baseline":[r["score"] for r in hold[0]],"holdout_optimized":[r["score"] for r in hold[winner]],
     "holdout_max_delays":[r["metrics"]["max_delay"] for r in hold[winner]],"holdout_failed":0,
     "trace":moves,"deadlock_detected":True,"corruption_checks":5,"tick_reference_matches":True})
