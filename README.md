![rail-craft — dispatch policies and simulation](docs/images/banner.png)

# rail-craft 0.3.0

> **One of 18 related repositories.** The [repository overview](https://github.com/dietmarwo/fcmaes-rust/blob/main/docs/repositories.md) shows what each one does and how they connect.

A bounded research evaluator for dispatch policies on synthetic fixed-route rail
corridors: capacity-one resources, blocking, clearance and entry closures on an integer
tick clock. See the [model card](docs/model.md) for units, equations and supported
behavior, the [user manual](docs/user-manual.md) for the complete reference with runnable
examples, the [tutorial](docs/tutorial.md) for a short tour and the
[architecture guide](docs/architecture.md) for internals and measured performance.
[`examples/python/`](examples/python/) holds application and optimization studies with
SciPy and fcmaes. The [0.3.0 performance log](rail-craft-performance-log-0.3.0.md) records
the ten performance rounds of this release; the
[0.2.0 log](rail-craft-performance-log-0.2.0.md) and the
[0.1.0 implementation record](docs/implementation-0.1.0.md) preserve earlier work.

## Rust CLI and native examples

```sh
cargo run --release --locked -- describe
cargo run --release --locked -- run models/request.json
cargo run --release --locked --example sweep -- models/example.json
cargo run --release --locked --example matrix_search -- models/example.json
```

The CLI uses versioned JSON with exact model-byte SHA-256, candidate IDs and
explicit work and worker limits. `run -` accepts stdin. Exit 1 indicates a
rejected request. Exit 2 retains classified candidate records when at least
one candidate is incomplete or infeasible. Output traces are opt-in.

For rail trace replay, write the model and the result's interval array to
separate JSON files and use `rail-craft verify MODEL TRACE`.

## Python

Install the matching wheel from `packages/` in a clean environment.
The tested wheel target is Linux x86-64 with a CPython 3.10+ ABI.
Actual clean-environment verification used CPython 3.12.

```python
from rail_craft import Prepared, load_example

case = load_example()
prepared = Prepared(case["model"])
row = prepared.evaluate(case["parameters"], trace=True)
assert row["status"] == "complete"
print(row["score"])
scores, statuses, steps = prepared.score_matrix([case["parameters"], [0, 1]])
print(scores, statuses, steps)
```

`Prepared.batch` returns full records, `Prepared.scores` lean records with null metrics,
and `Prepared.score_matrix` plain score, status and step lists for untraced parameter
rows, without JSON in either direction. Each call crosses into native code once and
releases the GIL; workers evaluate candidates in isolation and results keep input order.
The local Python campaign commits results to SQLite, preserves interrupted
attempts and reuses completed records by candidate, scenario, settings and
build identity. Fixed-work replay is scoped to the tested build.
Run `validation/run_campaign.py MODULE models/campaign.json campaign.sqlite`
with the installed package to evaluate or resume the included manifest.

## Performance

Against 0.2.0 on the same 2-vCPU host, the 110-train and 300-closure benchmark cases run
3.7x and 4.6x faster with full records, small 96-candidate batches 1.8x faster, and the
matrix API scores 96 small candidates 3.1x faster than 0.2.0 lean records. Details,
the time breakdown, worker scaling and Python optimization timings are in the
[architecture guide](docs/architecture.md#performance) and the performance log.

## Verify and rebuild

```sh
cargo test --release --locked
cargo build --release --locked
python validation/validate.py ./target/release/rail-craft
python validation/release_checks.py rail_craft docs/tutorial.md docs/user-manual.md
maturin sdist --out dist
maturin build --release --locked --out dist
```

Independent numerical scripts use Python 3.12. The validation and optimization
scripts use NumPy 2.3.5 and SciPy 1.17.0; the fcmaes examples were run with fcmaes 2.1.0.
The optional `validation/optimize.py rail_craft` demonstrates an in-process SciPy DE
objective. The native sweep needs only the Rust crate. See `evidence/` for actual runs.
