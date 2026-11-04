# Rail Craft tutorial

Evaluate dispatch weights for fixed routes through capacity-one resources with entry closures and clearance. This guide uses the examples shipped with version 0.2.0.

## 1. Install and run the example

Open the package root after extracting the ZIP. The bundled wheel was
built for Linux x86-64 with a CPython 3.10+ stable ABI and tested with
CPython 3.12. Create a virtual environment with a compatible Python.

```sh
python3 -m venv .venv
. .venv/bin/activate
python -m pip install --no-deps packages/*.whl
```

If the wheel tag is incompatible, build the supplied Rust/Python source
on the target system with Rust 1.88+ and Maturin 1.9.6. Cargo needs the
locked dependencies available from its registry or cache.

```sh
python -m pip install 'maturin==1.9.6'
maturin build --release --locked --out dist
python -m pip install --no-deps dist/*.whl
```

```python
from rail_craft import Prepared, load_example

case = load_example()
prepared = Prepared(case["model"])
row = prepared.evaluate(case["parameters"], trace=True)
assert row["status"] == "complete", row
print(row["id"], row["steps"], row["score"])
print(list(row["metrics"]))
```

The model is validated when you create `Prepared`. A malformed model
raises `ValueError`. A well-formed batch can still contain an invalid,
infeasible, or interrupted candidate. Its row carries a status and
detail. The optimizer convention is to minimize `score`.

## 2. Run the domain workflow

The following examples continue in the same Python session.

### Read an occupancy schedule

The example includes opposing routes, sidings and platforms. The
candidate is `[priority_weight, waiting_age_weight]`. Both weights
are nonnegative. The objective is priority-weighted lateness, while
`max_delay` and `delays` expose unweighted outcomes.

```python
from pathlib import Path
import json

row = prepared.evaluate(case["parameters"], trace=True)
assert row["status"] == "complete", row
metrics = row["metrics"]
print(row["score"], metrics["delays"], metrics["max_delay"])
Path("rail-model.json").write_text(json.dumps(case["model"]))
Path("rail-trace.json").write_text(json.dumps(metrics["intervals"]))
```

```sh
cargo run --release --locked -- verify rail-model.json rail-trace.json
```

The verifier accepts the interval array and the model as separate
JSON files. Change an interval's `service_end` to be less than
`entry` and verify again to exercise rejection. A train can finish
service and still hold its current block until it can enter its next
block. Inspect the difference between `service_end` and `exit`.
A closure is `[start, end)` and forbids entry, including at `start`.
Resource clearance applies after `exit`.

### Compare policies under a new scenario

```python
model = dict(case["model"])
model["closures"] = [*model.get("closures", []),
                     {"resource": 2, "start": 8, "end": 15}]
scenario = Prepared(model)
for weights in [[0, 0], [1, 0], [1, 1]]:
    row = scenario.evaluate(weights, trace=True)
    print(weights, row["status"], row["score"], row["detail"])
```

An incomplete schedule can be a horizon or deadlock failure.
Use trace diagnostics to inspect active and closed intervals, and
retain failures during policy comparisons. The supplied one-block
enumeration checks the optimum on a bounded case. The multi-block
dispatch heuristic uses current availability and waiting age.

## 3. Batch candidates and replay the winner

```python
candidates = [
    {"id": "first", "parameters": case["parameters"]},
    {"id": "second", "parameters": case["parameters"]},
]
scores = prepared.scores(candidates, workers=2)["records"]
assert [row["id"] for row in scores] == ["first", "second"]
winner = min((r for r in scores if r["status"] == "complete"),
             key=lambda r: r["score"])
candidate = next(c for c in candidates if c["id"] == winner["id"])
replay = prepared.evaluate(candidate["parameters"], trace=True)
assert replay["status"] == "complete", replay
print(replay["score"], replay["metrics"])
```

Replace the duplicate vectors with distinct choices for a real search.
Use `batch` if you need the full metrics for every row. `scores` returns
ordered records with score, status, steps and detail, and `metrics: null`.
It rejects any candidate with `trace: true`. A one-worker batch or a
singleton runs inline. Larger batches can try two or four workers.
Measure before setting a worker count for a real workload.

The optional differential-evolution driver times preparation, seeded
search and traced replay as one run. Install SciPy for this example.

```sh
python -m pip install scipy
python validation/optimize_performance.py rail_craft --scores
python validation/optimize_performance.py rail_craft
```

The driver uses small local bounds and a short fixed search budget.
For a scientific result, use problem-specific bounds, an explicit
feasibility penalty or constraint policy, and a held-out evaluation.

## 4. Run and resume a local campaign

```sh
python validation/run_campaign.py rail_craft models/campaign.json campaign.sqlite > first.json
python validation/run_campaign.py rail_craft models/campaign.json campaign.sqlite > replay.json
cmp first.json replay.json
```

The manifest holds scenario models and candidate vectors. Keep IDs
unique inside each list. Reuse the database to resume identical work.
The native build, model, scenario, candidate and work settings are
part of cache identity. A new wheel re-evaluates cached work under
its own build identity. A cancel file passed as `cancel_file` or a
`wall_ms` budget can stop new evaluations cooperatively. Inspect
statuses in the attempt journal before rerunning.

## 5. Use the Rust CLI and benchmark

```sh
cargo build --release --locked --bins --examples
target/release/rail-craft describe
target/release/rail-craft run models/request.json
target/release/examples/sweep models/example.json
target/release/examples/performance models/example.json serial
CRAFT_SCORES=1 target/release/examples/performance models/example.json serial
```

The CLI reads a request file or standard input with `run -`. Exit code
0 indicates complete rows, 1 indicates a rejected request and 2
indicates one or more unsuccessful rows in a well-formed request.
The benchmark prints elapsed seconds per prepared batch after warm-up.
Use `parallel` for four workers, `trace` for traced evaluations,
or `domain` and `domain2` for the package's supplemental cases.
With a previous benchmark executable built from the baseline and this
one, `validation/performance_pair.py BEFORE AFTER models/example.json serial`
checks output parity and times both. See the included performance log
and raw evidence for the exact workloads and comparison boundaries.

## 6. Verify results

```sh
cargo test --release --locked
cargo clippy --locked --all-targets --features python -- -D warnings
python validation/validate.py target/release/rail-craft
```

Consult the [model card](model.md) for supported inputs, the
[architecture guide](architecture.md) for data flow and numerical
assumptions, and the [source notes](sources.md) for references.
