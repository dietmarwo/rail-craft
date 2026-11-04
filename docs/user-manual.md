# rail-craft user manual

Version 0.3.0. This manual is for researchers and engineers who use rail-craft to evaluate
dispatch policies on synthetic fixed-route rail corridors. It covers installation, the
model and its JSON format, every API (Python, Rust and command line), statuses, budgets,
reproducibility, performance and troubleshooting, with runnable examples throughout.

The [model card](model.md) states the equations and research scope, the
[tutorial](tutorial.md) is a short guided tour, the [architecture guide](architecture.md)
describes internals and measured performance, and [`examples/python/`](../examples/python/)
holds longer application and optimization studies.

All Python blocks in this manual form one session: run them in order in one interpreter.
`validation/release_checks.py` executes every one of them against the installed wheel.
Shell blocks show output copied from actual runs on the release build.

## Contents

1. [What rail-craft models](#1-what-rail-craft-models)
2. [Installation](#2-installation)
3. [Concepts](#3-concepts)
4. [Model JSON reference](#4-model-json-reference)
5. [Candidates, requests and results](#5-candidates-requests-and-results)
6. [Statuses and error messages](#6-statuses-and-error-messages)
7. [Budgets](#7-budgets)
8. [Python API](#8-python-api)
9. [Rust API](#9-rust-api)
10. [Command line](#10-command-line)
11. [Worked examples](#11-worked-examples)
12. [Reproducibility and provenance](#12-reproducibility-and-provenance)
13. [Performance tips](#13-performance-tips)
14. [Troubleshooting](#14-troubleshooting)
15. [Limits](#15-limits)

## 1. What rail-craft models

rail-craft simulates trains that follow fixed routes through capacity-one resources
(blocks, sidings, platforms) on an integer tick clock. A dispatcher decides which waiting
train enters a free resource next. It ranks trains by a weighted sum of the train's
declared priority and the time it has waited. The two weights are the candidate
parameters that a study or an optimizer varies. The simulator returns the
priority-weighted arrival delay as a score to minimize, and optionally a full movement
trace that an independent verifier checks.

Supported behavior: release times, per-leg run durations, blocking (a train keeps its
current resource until it can enter the next one), clearance time after a resource is
vacated, entry closures (maintenance windows) and detection of deadlock and horizon
overrun. Out of scope: signalling, train dynamics, timetable synthesis, rerouting and
operational dispatch authority. All bundled cases are synthetic.

## 2. Installation

### 2.1 Python wheel

The release ships a wheel for Linux x86-64 with the CPython 3.10+ stable ABI, tested with
CPython 3.12.

```sh
python3 -m venv .venv
. .venv/bin/activate
python -m pip install --no-deps packages/rail_craft-0.3.0-cp310-abi3-manylinux_2_34_x86_64.whl
python -c "import rail_craft; print(rail_craft.Prepared)"
```

The package has no runtime dependencies. NumPy is optional: `Prepared.score_matrix`
accepts NumPy arrays. The examples also use SciPy and fcmaes.

### 2.2 From source

On other platforms, build the wheel from the source distribution with Rust 1.88 or newer
and maturin 1.9.6. Cargo needs the locked dependencies from its registry or cache.

```sh
python -m pip install 'maturin==1.9.6'
tar xzf packages/rail_craft-0.3.0.tar.gz
cd rail_craft-0.3.0
maturin build --release --locked --out dist
python -m pip install --no-deps dist/*.whl
```

### 2.3 Command line and Rust crate

```sh
cargo build --release --locked
target/release/rail-craft describe
```

The crate is also available as `packages/rail-craft-0.3.0.crate`, a gzipped tar archive.
Extract it and add the directory as a path dependency named `rail-craft`; import it as
`rail_craft`. The `python` cargo
feature builds the PyO3 extension and is off by default.

## 3. Concepts

**Ticks.** Time is a nonnegative integer. Fixtures use abstract ticks; choose a tick
length (for example 10 seconds) before converting real data.

**Resources.** Every resource has capacity one: at most one train occupies it. After a
train leaves at tick `exit`, the resource stays unavailable until `exit + clearance`.

**Trains and routes.** A train is released at `release` and must traverse its route, a
list of legs. Each leg names a resource and a run duration. Consecutive legs use
different resources. A train arrives when the service of its last leg ends, and its delay
is `max(0, arrival - planned)`.

**Blocking.** A train that finishes service on a leg keeps holding that resource until it
enters the next one. A movement record therefore has an `entry`, a `service_end`
(`entry + duration`) and an `exit` (when the train actually left), with
`service_end <= exit`. For the last leg, `exit == service_end`.

**Closures.** A closure `[start, end)` forbids entry to a resource at every tick
`start <= t < end`. A train that is already inside continues its movement. Overlapping or
adjacent closures act as one longer closure.

**Events and dispatch.** At each tick, the simulator first processes all events of that
tick in this order: service completions, closure ends, closure starts, releases and
clearance wakes. It then dispatches repeatedly. Among the waiting trains whose next
resource is free (no occupant, clearance elapsed, no active closure), it picks the train
with the highest

```text
dispatch score = priority_weight * priority + waiting_age_weight * (t - waiting_since)
```

where `waiting_since` is the tick at which the train became ready for its next leg.
Ties go to the earlier `waiting_since`, then to the lower train index. Entering a resource
releases the previous one at the same tick. The dispatcher has no look-ahead, so opposing
trains can block each other permanently: that outcome is classified as a deadlock.

**Objective.** A complete schedule scores

```text
score = sum over trains of max(0, arrival - planned) * (1 + priority)
```

Lower is better. The optimizer convention is to minimize `score`.

**Steps.** Every processed event and every dispatch is one logical step. Steps measure
work, drive `max_steps` limits and are part of every record. The bundled example takes
21 steps.

**Verification.** After a complete simulation, an independent verifier reconstructs each
train's and each resource's movement sequence and checks route continuity, release, run
durations, horizon, closures, capacity and clearance. Even lean and matrix evaluations
run it before reporting `complete`.

**Prepared evaluator and candidates.** `Prepared` validates a model once and evaluates
many candidates against it. A candidate is a parameter vector
`[priority_weight, waiting_age_weight]` with an ID and an optional trace flag. A batch is
a list of candidates evaluated in isolation and returned in input order.

## 4. Model JSON reference

A model is a JSON object. Unknown fields are rejected at every level. All tick fields are
JSON integers (a value such as `3.0` is rejected); `priority` is any JSON number.

### 4.1 Top level

| Field | Type | Units | Bounds | Default | Meaning |
| --- | --- | --- | --- | --- | --- |
| `resources` | array of resource objects | | 1 to 128 entries | required | Capacity-one resources, referenced by index from 0 |
| `trains` | array of train objects | | 1 to 128 entries | required | Trains, referenced by index from 0 |
| `horizon` | integer | ticks | 1 to 1,000,000,000 | required | Last tick at which events are processed |
| `closures` | array of closure objects | | 0 to 512 entries | `[]` | Entry closures |

### 4.2 Resource

| Field | Type | Units | Bounds | Default | Meaning |
| --- | --- | --- | --- | --- | --- |
| `name` | string | | non-empty, unique among resources | required | Label |
| `clearance` | integer | ticks | 0 to `horizon` | required | Time after a train exits before the resource can be entered again |

### 4.3 Train

| Field | Type | Units | Bounds | Default | Meaning |
| --- | --- | --- | --- | --- | --- |
| `name` | string | | non-empty, unique among trains | required | Label |
| `release` | integer | tick | 0 to `horizon` | required | Earliest entry to the first leg |
| `planned` | integer | tick | 0 to `horizon` | required | Planned arrival; delay is measured against it |
| `priority` | number | | finite, 0 to 1000 | required | Dispatch priority and objective weight `1 + priority` |
| `route` | array of leg objects | | 1 to 64 legs; consecutive legs use different resources | required | Fixed route |

### 4.4 Leg

| Field | Type | Units | Bounds | Default | Meaning |
| --- | --- | --- | --- | --- | --- |
| `resource` | integer | index | below the number of resources | required | Resource used by this leg |
| `duration` | integer | ticks | 1 to `horizon` | required | Run (service) time on the resource |

### 4.5 Closure

| Field | Type | Units | Bounds | Default | Meaning |
| --- | --- | --- | --- | --- | --- |
| `resource` | integer | index | below the number of resources | required | Closed resource |
| `start` | integer | tick | below `end` | required | First closed tick |
| `end` | integer | tick | `start + 1` to `horizon` | required | First open tick again (half-open interval) |

### 4.6 Candidate parameters

| Index | Name | Bounds | Meaning |
| --- | --- | --- | --- |
| 0 | `priority_weight` | 0 to 10 | Multiplies each train's `priority` in the dispatch score |
| 1 | `waiting_age_weight` | 0 to 10 | Multiplies the ticks a train has waited for its next leg |

Exactly two values are required. Any other length, or a value outside `[0, 10]`
(including NaN in a matrix row), makes the candidate `invalid` without failing the batch.

## 5. Candidates, requests and results

### 5.1 Candidate

```json
{"id": "policy-7", "parameters": [4.0, 0.25], "trace": false}
```

| Field | Type | Bounds | Default |
| --- | --- | --- | --- |
| `id` | string | 1 to 128 bytes, unique within the batch | required |
| `parameters` | array of numbers | at most 64 values | required |
| `trace` | boolean | at most 16 traced candidates per batch | `false` |

### 5.2 Command-line request

```json
{
  "schema": "rail-craft.request/0.1",
  "model": {"...": "model object"},
  "candidates": [{"id": "example", "parameters": [1, 0], "trace": true}],
  "budget": {"workers": 1}
}
```

`schema` must be exactly `rail-craft.request/0.1`; `budget` is optional. See
`models/request.json` for a complete file.

### 5.3 Result envelope

```json
{"model_sha256": "898de434...", "records": ["..."],
 "schema": "rail-craft.result/0.1", "version": "0.3.0"}
```

`model_sha256` is the SHA-256 of the exact model bytes the evaluator received, `schema` is
the result schema tag (unchanged since 0.1) and `version` is the package version.

### 5.4 Record

| Field | Type | Meaning |
| --- | --- | --- |
| `id` | string | Candidate ID |
| `status` | string | One of the statuses in section 6.1 |
| `steps` | integer | Logical steps used |
| `score` | number or null | Weighted delay, present only for `complete` |
| `metrics` | object or null | Full records only; null in lean records and for `invalid` |
| `detail` | string or null | Reason for every status other than `complete` |

### 5.5 Metrics

| Status | Metrics fields |
| --- | --- |
| `complete` | `completed` (trains arrived), `delays` (per train, ticks), `intervals` (movements if traced, else null), `max_delay` |
| `work_limit`, `wall_limit`, `cancelled` | `closed_intervals` (finished movements if traced, else null), `completed`, `time` (tick of the stop) |
| `infeasible` | `active` (movements still holding a resource if traced, else null), `closed_intervals`, `completed`, `time` |
| `invalid` | null |

A movement (interval) is
`{"entry", "exit", "leg", "resource", "service_end", "train"}` with integer values; `train`,
`leg` and `resource` are indexes. Closed movements are listed in the order they were
vacated.

## 6. Statuses and error messages

### 6.1 Record statuses

A well-formed batch always returns one record per candidate. Failures of single candidates
are classified in their records.

| Status | `detail` | Meaning |
| --- | --- | --- |
| `complete` | null | All trains arrived and the trace verified; `score` is set |
| `invalid` | `expected priority and waiting-time weights` | Parameter vector does not have exactly two values |
| `invalid` | `weights must be in [0,10]` | A weight is outside `[0, 10]` or NaN |
| `invalid` | a verifier message | The independent verifier rejected the simulated trace (not observed; it would indicate a simulator defect) |
| `infeasible` | `deadlock` | No event is pending and some train can never move |
| `infeasible` | `horizon` | Events remain after `horizon` |
| `work_limit` | `candidate step budget reached` | The candidate needed more than `max_steps` steps |
| `wall_limit` | `batch deadline reached` | The batch's `wall_ms` deadline passed |
| `cancelled` | `cancel file exists` | The `cancel_file` path existed at a step |

### 6.2 Batch and model errors

These reject the whole call: Python raises `ValueError`, the CLI exits with status 1 and
prints `{"detail": ..., "status": "rejected"}` to standard error.

| Message | Cause |
| --- | --- |
| `invalid corridor dimensions or horizon` | Resource, train or closure counts or `horizon` out of bounds |
| `invalid resource name or clearance` | Empty or duplicate resource name, or `clearance > horizon` |
| `invalid train, timing, or route` | Empty or duplicate train name, `release` or `planned` after `horizon`, bad priority, route length, leg duration or resource index, or two consecutive legs on one resource |
| `invalid closure interval` | Unknown resource, `start >= end` or `end > horizon` |
| `invalid budget or batch size` | Budget value out of bounds, or not 1 to 4,096 candidates |
| `invalid or duplicate candidate id, or parameter vector exceeds limit` | Empty, too long or repeated ID, or more than 64 parameters |
| `at most 16 traced candidates per batch` | Too many traced candidates |
| `scores mode accepts untraced candidates only` | A traced candidate passed to `scores` |
| `matrix rows must be between 1 and 4096` | `score_matrix` row count |
| `parameter matrix length is not a multiple of rows` | Rust `score_matrix` buffer shape |
| `parameter vector exceeds limit` | Matrix rows with more than 64 values |
| `ragged parameter rows` | Python `score_matrix` rows of different lengths |
| `score_matrix expects a 2-D array of parameter rows` | NumPy input that is not 2-D |
| `unsupported request schema` | CLI request `schema` tag |
| `request exceeds 16 MiB` | CLI request size |
| serde messages, for example ``unknown field `speed` `` | Malformed JSON, wrong types, missing or unknown fields |

## 7. Budgets

Every call takes the same budget. In Python, pass the fields as keyword arguments; in Rust
and the CLI, pass a JSON object. Unknown fields are rejected.

| Field | Type | Bounds | Default | Effect |
| --- | --- | --- | --- | --- |
| `workers` | integer | 1 to 64 | 1 | Concurrent evaluators for the batch |
| `max_steps` | integer | 1 to 100,000,000 | 1,000,000 | Step limit per candidate; reaching it gives `work_limit` |
| `wall_ms` | integer or null | 1 to 86,400,000 | null | Deadline for the whole batch, measured from the call; checked at every step |
| `cancel_file` | path or null | | null | If this file exists at a step, the candidate is `cancelled` |

With one worker, or a single candidate, evaluation runs on the calling thread. With more
workers, the calling thread and `min(workers, candidates) - 1` scoped threads claim
candidates from a shared counter; records are always returned in input order. Results with
fixed-work budgets (`max_steps` only) are identical for every worker count. `wall_ms` and
`cancel_file` are cooperative and depend on the clock or the file system, so which
candidates they stop is not reproducible. Monitored steps cost more; see section 13.

## 8. Python API

```text
rail_craft.Prepared(model)
    .evaluate(parameters, *, trace=False, **budget) -> record dict
    .batch(candidates, **budget)                    -> result envelope dict
    .scores(candidates, **budget)                   -> result envelope dict, metrics null
    .score_matrix(rows, **budget)                   -> (scores, statuses, steps)
rail_craft.load_example()                           -> {"model": ..., "parameters": ...}
rail_craft.campaign.run(database, scenarios, candidates, *, workers=1, chunk_size=16, **budget)
```

- `Prepared(model)` encodes the model dict as compact JSON with sorted keys, validates it
  and keeps it ready. Invalid models raise `ValueError`.
- `evaluate` evaluates one candidate with the ID `"one"` and returns its record.
- `batch` returns full records with metrics for every candidate.
- `scores` returns the same envelope with `metrics: null` and rejects traced candidates.
  It still runs the verifier, applies limits and classifies failures.
- `score_matrix` takes a list of equal-length lists or a 2-D NumPy array, one row per
  candidate, and returns three lists in row order: scores (NaN for every row that is not
  `complete`), statuses and steps. It builds no JSON records in either direction and
  releases the GIL during evaluation. For each row, status, steps and score are exactly
  what `scores` reports for the same parameters.
- `load_example` returns the packaged two-train corridor and a replayable parameter vector.
- `campaign.run` evaluates every candidate in every scenario, stores results in SQLite and
  reuses them on the next run; see section 11.13.

Every method crosses into native code once per call. Native evaluation releases the GIL,
so several Python threads can share one `Prepared`.

## 9. Rust API

```rust
use rail_craft::{MatrixScore, Prepared};

let prepared = Prepared::new(model_json)?;            // Result<Prepared, String>
let full: String = prepared.evaluate_json(candidates_json, r#"{"workers":2}"#)?;
let lean: String = prepared.scores_json(candidates_json, "{}")?;
let rows: Vec<MatrixScore> = prepared.score_matrix(2, &[1.0, 0.0, 4.0, 0.25], "{}")?;
let hash: &str = prepared.model_sha256();
let result: serde_json::Value = rail_craft::run(request_json)?;   // the CLI request path
let verdict = rail_craft::verify_json(model_json, trace_json)?;   // {"movements", "valid"}
let capabilities = rail_craft::capabilities();
```

`MatrixScore` has public fields `status: &'static str` (the record status strings),
`steps: u64` and `score: Option<f64>` (`Some` only for `complete`). `score_matrix` reads
`rows` candidates stored row-major in `parameters`; each row has
`parameters.len() / rows` values, and dimension 0 is allowed. All methods take `&self` and
can be shared across threads. A `Prepared` keeps up to 64 sets of reusable evaluation
buffers between calls. `examples/matrix_search.rs` is a complete program:

```sh
cargo run --release --locked --example matrix_search -- models/example.json
```

```text
441 of 441 policies complete; best weighted delay 17 at [0.5, 0] (21 steps); worst 37
```

## 10. Command line

```text
rail-craft describe
rail-craft run FILE        (use - to read standard input; at most 16 MiB)
rail-craft verify MODEL TRACE
```

Exit status 0 means every record is `complete`, 1 means the request was rejected and 2
means a well-formed request has at least one record that is not `complete`.

```sh
target/release/rail-craft describe
```

```text
{"batch":true,"models":["integer_tick_fixed_routes","blocking_capacity_one","entry_closures","trace_validation"],"schema":"rail-craft.capabilities/0.1","version":"0.3.0","worker_limit":64}
```

```sh
target/release/rail-craft run models/request.json
```

```text
{"model_sha256":"898de4347d03524411fdf46ef144917c44608bb10516a65535301fc79dc53bc5","records":[{"detail":null,"id":"example","metrics":{"completed":2,"delays":[7,2],"intervals":[{"entry":0,"exit":5,"leg":0,"resource":1,"service_end":1,"train":1},{"entry":5,"exit":9,"leg":1,"resource":2,"service_end":9,"train":1},{"entry":0,"exit":10,"leg":0,"resource":0,"service_end":1,"train":0},{"entry":9,"exit":12,"leg":2,"resource":4,"service_end":12,"train":1},{"entry":10,"exit":14,"leg":1,"resource":2,"service_end":14,"train":0},{"entry":14,"exit":17,"leg":2,"resource":3,"service_end":17,"train":0}],"max_delay":7},"score":17.0,"status":"complete","steps":21}],"schema":"rail-craft.result/0.1","version":"0.3.0"}
```

A request with one valid and one invalid candidate exits with status 2 and keeps both
records:

```sh
cat > mixed.json <<'EOF'
{"schema": "rail-craft.request/0.1",
 "model": {"horizon": 40, "resources": [{"name": "block", "clearance": 1}],
           "trains": [{"name": "T0", "release": 0, "planned": 3, "priority": 0,
                       "route": [{"resource": 0, "duration": 3}]},
                      {"name": "T1", "release": 0, "planned": 3, "priority": 3,
                       "route": [{"resource": 0, "duration": 3}]}]},
 "candidates": [{"id": "priority", "parameters": [1, 0], "trace": true},
                {"id": "too-heavy", "parameters": [12, 0]}],
 "budget": {"workers": 2}}
EOF
rail-craft run mixed.json; echo "exit $?"
```

```text
{"model_sha256":"281b92a039b58a8a275fbc0e3a9ebba5469ffc9236506abb3fe0ea9dc51fc8cf","records":[{"detail":null,"id":"priority","metrics":{"completed":2,"delays":[4,0],"intervals":[{"entry":0,"exit":3,"leg":0,"resource":0,"service_end":3,"train":1},{"entry":4,"exit":7,"leg":0,"resource":0,"service_end":7,"train":0}],"max_delay":4},"score":4.0,"status":"complete","steps":7},{"detail":"weights must be in [0,10]","id":"too-heavy","metrics":null,"score":null,"status":"invalid","steps":0}],"schema":"rail-craft.result/0.1","version":"0.3.0"}
exit 2
```

A rejected request prints to standard error and exits with status 1:

```sh
echo '{"schema": "rail-craft.request/0.1", "model": {"horizon": 10, "resources": [], "trains": []}, "candidates": []}' | rail-craft run -; echo "exit $?"
```

```text
{"detail":"invalid corridor dimensions or horizon","status":"rejected"}
exit 1
```

`verify` checks a movement array against a model, independently of the simulator.
Section 11.12 writes these two files from Python; this shell version does the same:

```sh
python - <<'EOF'
import json
from rail_craft import Prepared, load_example
case = load_example()
row = Prepared(case["model"]).evaluate(case["parameters"], trace=True)
json.dump(case["model"], open("rail-model.json", "w"))
json.dump(row["metrics"]["intervals"], open("rail-trace.json", "w"))
broken = row["metrics"]["intervals"]
broken[0]["service_end"] = 0
json.dump(broken, open("broken-trace.json", "w"))
EOF
rail-craft verify rail-model.json rail-trace.json; echo "exit $?"
rail-craft verify rail-model.json broken-trace.json; echo "exit $?"
```

```text
{"movements":6,"valid":true}
exit 0
{"detail":"resource, release, service, horizon, or closure violation","status":"rejected"}
exit 1
```

The crate also has two example programs. `sweep` evaluates five scaled copies of the
example parameters with two workers and prints the best record; `performance` is the
benchmark used for the performance log (section 13):

```sh
cargo run --release --locked --example sweep -- models/example.json
target/release/examples/performance models/example.json serial
CRAFT_SCORES=1 target/release/examples/performance models/example.json serial
CRAFT_MATRIX=1 target/release/examples/performance models/example.json serial
CRAFT_MATRIX=1 CRAFT_WORKERS=2 target/release/examples/performance models/example.json domain
target/release/examples/performance models/example.json domain breakdown
```

```text
{"detail":null,"id":"0","metrics":{"completed":2,"delays":[7,2],"intervals":null,"max_delay":7},"score":17.0,"status":"complete","steps":21}
0.000091086
0.000070686
0.00003612875
0.0007176392
{"budget":"{\"workers\":1}","candidates":48,"per_call_seconds":{"decode":6.95386e-6,"full":0.0010206059,"lean":0.0009435173400000001,"matrix":0.0008583055},"workload":"domain"}
```

The benchmark prints mean seconds per call after a warm-up; the values vary from run to
run. Workloads are `one`, `serial`, `parallel`, `trace`, `invalid`, `domain` and `domain2`;
`CRAFT_SCORES=1` selects lean records, `CRAFT_MATRIX=1` the matrix API and `CRAFT_WORKERS`
overrides the worker count. A third argument `probe` prints the result instead of timing
it, and `breakdown` times decoding, matrix, lean and full calls separately.

## 11. Worked examples

### 11.1 First evaluation

The packaged example has an eastbound and a westbound train that share a central block,
which is closed during `[1, 5)`.

```python
from rail_craft import Prepared, load_example

case = load_example()
prepared = Prepared(case["model"])
row = prepared.evaluate(case["parameters"], trace=True)
print(row["status"], row["steps"], row["score"])
print(row["metrics"]["delays"], row["metrics"]["max_delay"])
print(row["metrics"]["intervals"][0])
```

```text
complete 21 17.0
[7, 2] 7
{'entry': 0, 'exit': 5, 'leg': 0, 'resource': 1, 'service_end': 1, 'train': 1}
```

The westbound train (index 1, priority 4) finishes its siding leg at tick 1 but holds the
siding until the closure ends at tick 5.

### 11.2 Priority against waiting age on one block

Three trains with priorities 0, 1 and 3 are released together onto one block with run
time 3 and clearance 1, so entries are 4 ticks apart.

```python
def single_block(priorities, *, clearance=1, duration=3, horizon=40):
    return {
        "horizon": horizon,
        "resources": [{"name": "block", "clearance": clearance}],
        "trains": [{"name": f"T{i}", "release": 0, "planned": duration, "priority": p,
                    "route": [{"resource": 0, "duration": duration}]}
                   for i, p in enumerate(priorities)],
    }

block = Prepared(single_block([0, 1, 3]))
for weights in ([0, 0], [0, 1], [1, 0]):
    row = block.evaluate(weights, trace=True)
    order = [m["train"] for m in row["metrics"]["intervals"]]
    print(weights, "order", order, "entries", [m["entry"] for m in row["metrics"]["intervals"]],
          "score", row["score"])
```

```text
[0, 0] order [0, 1, 2] entries [0, 4, 8] score 40.0
[0, 1] order [0, 1, 2] entries [0, 4, 8] score 40.0
[1, 0] order [2, 1, 0] entries [0, 4, 8] score 16.0
```

With equal waiting starts, waiting age cannot separate the trains and the tie goes to
the lowest index. A positive priority weight serves the heaviest train first. On one block
with simultaneous releases, that order is optimal, which complete enumeration confirms:

```python
from itertools import permutations

def weighted_delay(order, priorities, cycle=4, duration=3):
    # Train k in the order enters at k * cycle and arrives `duration` later.
    return sum((1 + priorities[i]) * (k * cycle) for k, i in enumerate(order))

priorities = [0, 1, 3]
optimum = min(weighted_delay(o, priorities) for o in permutations(range(3)))
assert block.evaluate([1, 0])["score"] == optimum
print("enumerated optimum", optimum)
```

```text
enumerated optimum 16
```

### 11.3 Blocking and clearance

Two trains run through a siding (clearance 0) into a block (clearance 2).

```python
blocking = Prepared({
    "horizon": 30,
    "resources": [{"name": "siding", "clearance": 0}, {"name": "block", "clearance": 2}],
    "trains": [
        {"name": "first", "release": 0, "planned": 5, "priority": 0,
         "route": [{"resource": 0, "duration": 1}, {"resource": 1, "duration": 4}]},
        {"name": "second", "release": 0, "planned": 5, "priority": 0,
         "route": [{"resource": 0, "duration": 1}, {"resource": 1, "duration": 4}]},
    ],
})
row = blocking.evaluate([0, 1], trace=True)
for m in row["metrics"]["intervals"]:
    print(m)
print("delays", row["metrics"]["delays"], "score", row["score"])
```

```text
{'entry': 0, 'exit': 1, 'leg': 0, 'resource': 0, 'service_end': 1, 'train': 0}
{'entry': 1, 'exit': 5, 'leg': 1, 'resource': 1, 'service_end': 5, 'train': 0}
{'entry': 1, 'exit': 7, 'leg': 0, 'resource': 0, 'service_end': 2, 'train': 1}
{'entry': 7, 'exit': 11, 'leg': 1, 'resource': 1, 'service_end': 11, 'train': 1}
delays [0, 6] score 6.0
```

The second train enters the siding at tick 1, when the first moves on (clearance 0). It
finishes service at tick 2 but blocks the siding until tick 7: the block was vacated at
tick 5 and needs 2 ticks of clearance.

### 11.4 Closures

A one-tick movement is released at tick 0 under different closures:

```python
def closed_block(closures):
    return {
        "horizon": 20,
        "resources": [{"name": "block", "clearance": 0}],
        "trains": [{"name": "train", "release": 0, "planned": 0, "priority": 0,
                    "route": [{"resource": 0, "duration": 1}]}],
        "closures": [{"resource": 0, "start": s, "end": e} for s, e in closures],
    }

for closures in ([(0, 4)], [(0, 4), (2, 7)], [(0, 4), (4, 6)], [(0, 4), (5, 6)]):
    row = Prepared(closed_block(closures)).evaluate([0, 0], trace=True)
    print(closures, "entry", row["metrics"]["intervals"][0]["entry"])
```

```text
[(0, 4)] entry 4
[(0, 4), (2, 7)] entry 7
[(0, 4), (4, 6)] entry 6
[(0, 4), (5, 6)] entry 4
```

Overlapping and adjacent closures merge; a gap of one tick is enough to enter. Closures
only forbid entry. A train already inside keeps moving:

```python
through = Prepared({
    "horizon": 20,
    "resources": [{"name": "block", "clearance": 0}],
    "trains": [{"name": "early", "release": 0, "planned": 5, "priority": 0,
                "route": [{"resource": 0, "duration": 5}]},
               {"name": "late", "release": 1, "planned": 5, "priority": 0,
                "route": [{"resource": 0, "duration": 1}]}],
    "closures": [{"resource": 0, "start": 2, "end": 8}],
})
row = through.evaluate([1, 0], trace=True)
print([(m["train"], m["entry"], m["exit"]) for m in row["metrics"]["intervals"]])
```

```text
[(0, 0, 5), (1, 8, 9)]
```

### 11.5 Horizon failure

If events remain after the horizon, the schedule is `infeasible` with detail `horizon`.
Traced records show what had finished and what was still occupied:

```python
short = Prepared(single_block([0, 1, 3], horizon=6))
row = short.evaluate([1, 0], trace=True)
print(row["status"], row["detail"], row["steps"])
print(row["metrics"])
```

```text
infeasible horizon 7
{'active': [{'entry': 4, 'exit': 7, 'leg': 0, 'resource': 0, 'service_end': 7, 'train': 1}], 'closed_intervals': [{'entry': 0, 'exit': 3, 'leg': 0, 'resource': 0, 'service_end': 3, 'train': 2}], 'completed': 1, 'time': 4}
```

An active movement reports its planned `service_end` as `exit`, because it has not left.

### 11.6 Deadlock

Two trains swap ends of a two-block line. Each enters its first block and then waits for
the block the other one holds:

```python
swap = Prepared({
    "horizon": 50,
    "resources": [{"name": "west", "clearance": 0}, {"name": "east", "clearance": 0}],
    "trains": [
        {"name": "eastbound", "release": 0, "planned": 4, "priority": 0,
         "route": [{"resource": 0, "duration": 2}, {"resource": 1, "duration": 2}]},
        {"name": "westbound", "release": 0, "planned": 4, "priority": 0,
         "route": [{"resource": 1, "duration": 2}, {"resource": 0, "duration": 2}]},
    ],
})
row = swap.evaluate([1, 0], trace=True)
print(row["status"], row["detail"], "at tick", row["metrics"]["time"])
print("held:", [(m["train"], m["resource"]) for m in row["metrics"]["active"]])
```

```text
infeasible deadlock at tick 2
held: [(0, 0), (1, 1)]
```

Whether a layout deadlocks can depend on the weights: `examples/python/optimize_fcmaes_bite.py`
optimizes a policy for a passing-loop line on which some weights deadlock.

### 11.7 Invalid candidates

Invalid parameters are classified per record and cost no steps:

```python
rows = prepared.batch([
    {"id": "ok", "parameters": [1, 0]},
    {"id": "short", "parameters": [1]},
    {"id": "range", "parameters": [11, 0]},
])["records"]
for r in rows:
    print(r["id"], r["status"], r["steps"], r["detail"])
scores, statuses, steps = prepared.score_matrix([[1, 0], [float("nan"), 0], [-1, 2]])
print(scores, statuses, steps)
```

```text
ok complete 21 None
short invalid 0 expected priority and waiting-time weights
range invalid 0 weights must be in [0,10]
[17.0, nan, nan] ['complete', 'invalid', 'invalid'] [21, 0, 0]
```

Malformed batches are rejected as a whole:

```python
for bad in ([{"id": "a", "parameters": [1, 0]}, {"id": "a", "parameters": [0, 1]}],
            [{"id": "x", "parameters": [1, 0], "color": "red"}]):
    try:
        prepared.batch(bad)
    except ValueError as error:
        print("ValueError:", error)
try:
    prepared.batch([{"id": "a", "parameters": [1, 0]}], workers=0)
except ValueError as error:
    print("ValueError:", error)
try:
    prepared.score_matrix([[1, 0], [1]])
except ValueError as error:
    print("ValueError:", error)
```

```text
ValueError: invalid or duplicate candidate id, or parameter vector exceeds limit
ValueError: unknown field `color`, expected one of `id`, `parameters`, `trace` at line 1 column 9
ValueError: invalid budget or batch size
ValueError: ragged parameter rows
```

### 11.8 Work limits, deadlines and cancellation

```python
import tempfile
from pathlib import Path

full = prepared.evaluate([1, 0])
print("complete run takes", full["steps"], "steps")
limited = prepared.evaluate([1, 0], max_steps=10, trace=True)
print(limited["status"], limited["steps"], limited["detail"], limited["metrics"]["time"],
      len(limited["metrics"]["closed_intervals"]))
with tempfile.TemporaryDirectory() as folder:
    flag = Path(folder) / "stop"
    print(prepared.evaluate([1, 0], cancel_file=str(flag))["status"])
    flag.touch()
    row = prepared.evaluate([1, 0], cancel_file=str(flag))
    print(row["status"], row["steps"], row["detail"])
print(prepared.evaluate([1, 0], wall_ms=60_000)["status"])
```

```text
complete run takes 21 steps
work_limit 10 candidate step budget reached 9 1
complete
cancelled 0 cancel file exists
complete
```

A work-limited candidate reports exactly `max_steps` steps, the tick it reached and, if
traced, the movements it had finished. A cancel file that already exists stops the
candidate before its first step. Delete a stale cancel file before rerunning.

### 11.9 Workers, lean records and the matrix API agree

```python
candidates = [{"id": f"c{i}", "parameters": [i % 4, (i % 3) * 0.5]} for i in range(32)]
serial = prepared.batch(candidates, workers=1)
parallel = prepared.batch(candidates, workers=4)
assert serial == parallel
lean = prepared.scores(candidates, workers=2)
assert lean["records"] == [r | {"metrics": None} for r in serial["records"]]
scores, statuses, steps = prepared.score_matrix([c["parameters"] for c in candidates], workers=2)
assert scores == [r["score"] for r in serial["records"]]
assert steps == [r["steps"] for r in serial["records"]]
print(serial["model_sha256"][:16], serial["schema"], serial["version"], len(serial["records"]))
```

```text
d39e3ad773e28ab9 rail-craft.result/0.1 0.3.0 32
```

### 11.10 Scoring a NumPy grid

```python
import numpy as np

grid = np.array([[p, a] for p in np.linspace(0, 10, 11) for a in np.linspace(0, 10, 11)])
scores, statuses, steps = prepared.score_matrix(grid)
scores = np.asarray(scores)
best = int(np.nanargmin(scores))
print(len(scores), "rows,", statuses.count("complete"), "complete; best", grid[best], scores[best])
```

```text
121 rows, 121 complete; best [1. 0.] 17.0
```

`examples/python/policy_grid.py` runs a 1,681-policy study of a 16-train corridor this way.

### 11.11 Optimizing with SciPy and fcmaes

SciPy's vectorized differential evolution passes a `(dimension, population)` array, so one
`score_matrix` call scores a generation. Incomplete rows receive a penalty:

```python
from scipy.optimize import differential_evolution

def objective(population):  # shape (2, S) with vectorized=True
    scores, statuses, _ = prepared.score_matrix(population.T)
    return np.where([s == "complete" for s in statuses], scores, 1e9)

result = differential_evolution(objective, [(0, 10), (0, 10)], vectorized=True,
                                updating="deferred", seed=1, maxiter=20, popsize=8, polish=False)
replay = prepared.evaluate(result.x.tolist(), trace=True)
assert replay["score"] == result.fun
print(result.fun, replay["metrics"]["delays"])
```

```text
17.0 [7, 2]
```

fcmaes minimizers call a scalar objective; a one-row matrix keeps that call cheap:

```python
from loguru import logger
from scipy.optimize import Bounds

from fcmaes import bitecpp

logger.remove()

def scalar(x):
    scores, statuses, _ = prepared.score_matrix([x])
    return scores[0] if statuses[0] == "complete" else 1e9

result = bitecpp.minimize(scalar, bounds=Bounds([0, 0], [10, 10]), x0=[5, 5],
                          max_evaluations=300, rg=np.random.Generator(np.random.PCG64DXSM(2)))
print(result.fun, result.nfev)
```

```text
17.0 300
```

The two-train example has a flat landscape. `examples/python/` contains realistic
studies: vectorized DE under a closure (`optimize_scipy.py`), CMA-ES ask/tell over three
scenarios with whole populations per call (`optimize_fcmaes_cmaes.py`), and BiteOpt with
deadlock penalties (`optimize_fcmaes_bite.py`). Always replay the winner with
`trace=True` and inspect it, and check it on held-out scenarios.

### 11.12 Traces and independent verification

Write the model and a trace for `rail-craft verify` (section 10):

```python
import json

folder = Path(tempfile.mkdtemp())
row = prepared.evaluate(case["parameters"], trace=True)
(folder / "rail-model.json").write_text(json.dumps(case["model"]))
(folder / "rail-trace.json").write_text(json.dumps(row["metrics"]["intervals"]))
print(sorted(p.name for p in folder.iterdir()))
```

```text
['rail-model.json', 'rail-trace.json']
```

The verifier accepts movements in any order. It rejects unknown trains or legs, wrong
resources, entries before release or into a closure, wrong run durations, exits before
service ends or after the horizon, missing or repeated legs, gaps between consecutive
legs, and capacity or clearance conflicts.

### 11.13 Resumable campaigns

`campaign.run` evaluates a fixed scenario-by-candidate manifest, commits each chunk to
SQLite and reuses stored results on later runs:

```python
from rail_craft import campaign

with tempfile.TemporaryDirectory() as folder:
    database = Path(folder) / "study.sqlite"
    scenarios = [{"id": "nominal", "model": case["model"]},
                 {"id": "long closure", "model": case["model"] | {
                     "closures": [{"resource": 2, "start": 1, "end": 12}]}}]
    policies = [{"id": "priority", "parameters": [1, 0]},
                {"id": "fifo", "parameters": [0, 1]}]
    first = campaign.run(database, scenarios, policies, workers=2)
    again = campaign.run(database, scenarios, policies, workers=1)
    assert first == again
    for row in first:
        print(row["scenario"], row["record"]["id"], row["record"]["status"], row["record"]["score"])
```

```text
nominal priority complete 17.0
nominal fifo complete 37.0
long closure priority complete 59.0
long closure fifo complete 79.0
```

Each returned row has `scenario`, `key`, `build_sha256`, `model_sha256` and the full
`record`. The cache key hashes the native extension file, the scenario ID, the model, the
candidate and the budget without `cancel_file`. Complete, invalid and infeasible results
are cached; records with any other status (`work_limit`, `wall_limit` or `cancelled`)
stay only in the `attempts` journal and are evaluated again by the next run. A different wheel build uses a new cache namespace. The command-line driver
`validation/run_campaign.py MODULE MANIFEST DATABASE` runs a manifest such as
`models/campaign.json` with four workers.

### 11.14 Model validation errors

```python
for broken in (case["model"] | {"horizon": 0},
               case["model"] | {"closures": [{"resource": 2, "start": 5, "end": 5}]},
               case["model"] | {"trains": case["model"]["trains"] * 2},
               case["model"] | {"speed": 3}):
    try:
        Prepared(broken)
    except ValueError as error:
        print("ValueError:", error)
```

```text
ValueError: invalid corridor dimensions or horizon
ValueError: invalid closure interval
ValueError: invalid train, timing, or route
ValueError: unknown field `speed`, expected one of `resources`, `trains`, `horizon`, `closures` at line 1 column 273
```

Duplicated trains fail because train names must be unique.

## 12. Reproducibility and provenance

- Every result carries `model_sha256`, the SHA-256 of the exact model bytes. The Python
  wrapper always encodes models compactly with sorted keys, so equal dicts give equal
  hashes; the CLI hashes the model text as written in the request.
- `schema` tags (`rail-craft.request/0.1`, `rail-craft.result/0.1`) change only when a
  shape changes; `version` is the package version.
- With `max_steps` as the only limit, scores, statuses, steps, metrics and record order
  are identical for any worker count and between `batch`, `scores` and `score_matrix`.
  Repeated runs of the same build give identical bytes.
- `wall_ms` and `cancel_file` stop work cooperatively at the next step; which candidates
  finish depends on timing.
- Campaign caches are keyed by the native extension's SHA-256, so results from one build
  are never reused by another.
- Keep the model JSON, the parameter vectors, the budget and the package version with
  every published number, and replay winners with `trace=True`.

## 13. Performance tips

Measured on this release's host: 2 vCPUs, Intel Xeon Processor @ 2.10GHz (KVM), Rust
1.97, CPython 3.12. The [architecture guide](architecture.md#performance) has the full
tables and the evidence files.

- **Use `score_matrix` inside optimizers.** For 96 example candidates, one call costs
  about 37 us with `score_matrix`, 75 us with `scores` and 95 us with `batch`; a single
  candidate costs about 0.5, 1.2 and 1.4 us
  (`evidence/performance-0.3.0/breakdown.json`). For the 110-train `domain` and the
  300-closure `domain2` cases the simulation itself dominates and the three APIs are
  within about 12%.
- **Score whole populations per call.** Ask/tell optimizers and vectorized SciPy DE can
  pass a full generation; the per-call overhead is then paid once per generation.
- **Choose workers by work per batch, not by habit.** Thread start-up costs tens of
  microseconds. On this 2-vCPU host, 96 small candidates take 37 us with one worker and
  85 us with two; 48 candidates of the 110-train case take 0.87 ms with one worker and
  0.56 ms with two. More workers than CPUs only add overhead
  (`evidence/performance-0.3.0/worker-scaling.json`). As a rule, use one worker for
  batches under about 0.2 ms of work, and at most one worker per CPU otherwise.
- **Avoid monitored budgets in tight loops.** `wall_ms` reads the clock and `cancel_file`
  checks the file system at every step. With an absent cancel file, 96 rows of the
  example took 766 us instead of 54 us; with `wall_ms` they took 118 us
  (`evidence/performance-0.3.0/budget-monitoring.json`). Use them for long campaigns, not
  inside an optimizer's objective.
- **Reuse one `Prepared` per model.** Preparation validates and hashes the model and
  sorts its static events; evaluation buffers are pooled per `Prepared` and reused by
  later calls.
- **Trace only for replay.** Traces copy every movement into the result; at most 16
  candidates per batch can be traced.
- **Keep matrices within 4,096 rows.** Split larger grids into several calls.

## 14. Troubleshooting

| Symptom | Likely cause and fix |
| --- | --- |
| `ValueError: invalid type: floating point ...` when preparing a model | A tick field (`release`, `duration`, ...) is a float. Convert to `int` before passing; NumPy integers should be converted too. |
| `ValueError: invalid train, timing, or route` | Check names are unique, `release` and `planned` are at most `horizon`, `priority` is in `[0, 1000]`, every route has 1 to 64 legs with valid resource indexes, durations are at least 1, and no two consecutive legs use the same resource. |
| `ValueError: invalid corridor dimensions or horizon` | More than 128 trains or resources, more than 512 closures, or `horizon` outside 1 to 10^9. Split very large studies into sub-corridors. |
| Every record is `cancelled` with 0 steps | The cancel file still exists from an earlier stop. Delete it. |
| Records are `wall_limit` | The batch deadline covers the whole batch, not each candidate; raise `wall_ms` or split the batch. |
| `work_limit` on a large model | Raise `max_steps` (default 1,000,000; each event and dispatch is one step). |
| `infeasible` with `horizon` | Raise `horizon`, or treat it as a failed schedule; trace a replay to see `active` movements. |
| `infeasible` with `deadlock` | Opposing trains hold each other's next resources. Add passing capacity, change routes, or penalize the policy. |
| `ValueError: ragged parameter rows` | All `score_matrix` rows must have the same length. |
| `ValueError: matrix rows must be between 1 and 4096` | Empty input or too many rows; split large grids. |
| `ValueError: scores mode accepts untraced candidates only` | Use `batch` for traced candidates. |
| Scores are NaN | `score_matrix` uses NaN for every row that is not `complete`; check `statuses`. |
| A parallel batch is slower than a serial one | Batches with little work pay thread start-up; use `workers=1` (section 13). |
| The wheel does not install | The platform or Python is not Linux x86-64 CPython 3.10+; build from the source distribution (section 2.2). |

## 15. Limits

| Limit | Value | Reason |
| --- | --- | --- |
| Trains per model | 128 | Model validation; the dispatcher keeps ready trains in 128-bit masks and a 128-entry contender table |
| Resources per model | 128 | Model validation; resources with waiting trains are tracked in a 128-bit mask |
| Legs per route | 64 | Model validation |
| Closures per model | 512 | Model validation; also bounds the packed event keys |
| Horizon | 10^9 ticks | Model validation; all ticks stay exact in `f64` and fit the packed event keys |
| Candidates per batch, matrix rows | 4,096 | Batch validation |
| Parameters per candidate | 64 (2 are used) | Batch validation |
| Traced candidates per batch | 16 | Batch validation |
| Workers | 64 | Budget validation |
| Steps per candidate | 100,000,000 | Budget validation |
| CLI request size | 16 MiB | CLI |

The 128-train and 128-resource limits predate 0.3.0; the bitmask dispatcher relies on
them, so raising them would require wider masks.
