# rail-craft performance implementation log

4 November 2026. Version 0.3.0, based on the 0.2.0 delivery imported at `ca6f4aa`. The
ZIP contains the updated source, runnable examples, the ordered patch series
(`patches/0.3.0/`; the 0.2.0 series moved to `patches/0.2.0/`), the new
[user manual](docs/user-manual.md), the updated [architecture guide](docs/architecture.md)
and [tutorial](docs/tutorial.md), application and optimization examples in
[`examples/python/`](examples/python/), raw evidence and build artifacts. The
[0.2.0 log](rail-craft-performance-log-0.2.0.md) is kept unchanged.

All measurements in this log come from one host: 2 vCPUs, Intel(R) Xeon(R) Processor @
2.10GHz (KVM guest), Rust 1.97.0, CPython 3.12.3. The 0.2.0 log's numbers came from a
different host (AMD EPYC 9V74, 4 CPUs, Rust 1.88); no ratio below mixes the two hosts.
Five projects shared the host, so every build, test and timing ran under one shared CPU
lock and no timing overlapped another job.

## Ten incremental changes

Each row compares a round with the previous kept round, using
`validation/performance_pair.py` with 11 alternating samples per side on the workload
declared in `evidence/performance-0.3.0/plan.md` before timing. Times are median
milliseconds per prepared call. The decision rule, fixed in advance, kept a round when
its median speedup was at least 1.03x and the paired output check passed.

| Round | Commit | Change | Case | Before ms | After ms | Before / after |
| ---: | --- | --- | --- | ---: | ---: | ---: |
| 1 | `a966920` | Add the typed score-matrix API: `Prepared::score_matrix` and `MatrixScore` in Rust, a GIL-releasing Python `score_matrix`, shared budget limits, deadline and dispatch. This row compares lean `scores_json` with `score_matrix`. | `serial`, lean before, matrix after | 0.114 | 0.069 | 1.65x |
| 2 | `8207392` | Keep one 128-bit ready mask per resource of the trains' next legs and a mask of resources with waiting trains; check each resource's availability once and rank only trains waiting for available resources. | `domain` | 4.823 | 2.637 | 1.83x |
| 3 | `8640b7a` | Rank contenders with one `u128` key: the total-order image of the dispatch score and a tie-break tail precomputed when a train becomes ready. | `domain`, matrix | 1.958 | 1.434 | 1.37x |
| 4 | `19d06db` | Sort releases and closure starts and ends once at preparation as packed `u64` keys, walk them with a cursor and merge them with a small heap of completions and clearance wakes. | `domain2` | 2.191 | 1.320 | 1.66x |
| 5 | `81b6a33` | Verify intervals with prepared per-train leg slots and a stable counting pass by resource, keeping the verifier's phases, checks and messages. | `domain`, lean | 1.286 | 1.072 | 1.20x |
| 6 | `8a12680` | Move every per-candidate kernel and verifier buffer into a per-worker scratch that is reset per candidate; build the delay vector only for metrics. | `serial`, matrix | 0.101 | 0.060 | 1.67x |
| 7 | `a68b375` | Encode typed metrics once as raw JSON, with fields in the sorted key order of the former `Value` maps, instead of building a `Value` tree. | `serial` | 0.117 | 0.093 | 1.26x |
| 8 | `eb30c8d` | Keep a pool of at most 64 scratch sets in `Prepared` so consecutive calls reuse warm buffers. | `one`, matrix | 0.00073 | 0.00050 | 1.47x |
| 9 | `238957b` | Rank from one interleaved 128-entry contender table indexed by `train & 127`, with waiting starts stored as exact `f64` integers. | `domain`, lean | 1.029 | 0.902 | 1.14x |
| 10 | `2e65680` | Let the calling thread run the claim loop in multi-worker batches and spawn one thread fewer. | `parallel` | 0.215 | 0.183 | 1.18x |

Raw samples are in `evidence/performance-0.3.0/roundNN-*.json`. Every round passed
`cargo fmt --check`, Clippy with the `python` feature and warnings denied, and the locked
release tests, all inside the lock. Beyond the paired check of the declared workload, each
round's probe output was compared with its predecessor on all seven workloads in full,
lean and matrix modes (19 comparisons): full and lean JSON were byte-identical, and matrix
rows matched lean records with metrics and detail normalized to null. The logical step
count never changed. Rounds 5 and 9 changed code that the CLI verifier or the score path
uses, so the independent domain validation (`validation/validate.py`) also ran against
those rounds' release CLIs and passed.

New tests guard the structural changes: a randomized comparison of complete traced
output, steps and work-limit points with a frozen copy of the 0.2.0 dispatcher (400 random
corridor seeds, seven weight pairs, two step budgets, all outcome classes reached); a
randomized comparison of verifier verdicts and messages with the 0.2.0 verifier on
corrupted traces (all six verdict classes reached); order checks for the packed event keys
and the ranking keys; a byte check that every metrics shape matches the sorted `Value`
serialization; matrix-against-lean equivalence including invalid and infeasible rows,
`max_steps` limits, a cancel file and several worker counts; pooled-scratch equivalence
with fresh evaluators; and the dispatch bound on concurrent evaluators. Mutation checks
confirmed that the two randomized tests fail when the cross-resource tie-break or the
repeated-slot check is removed.

The 128-bit masks do not add a limit: model validation has limited models to 128 trains
and 128 resources since 0.1.0, and the masks and the 128-entry contender table rely on
that limit. The user manual lists it.

## Rejected attempts

None. Each of the ten planned rounds met the 1.03x rule at its first timing, so no
attempt was reverted and `evidence/performance-0.3.0/rejected/` is empty. The discarded
0.2.0 iterator-pipeline attempt remains documented in
`evidence/performance-discarded-rail-round9.json`.

## Cumulative native results

The 0.2.0 benchmark binary, built from the import commit, against the round-10 binary,
with 11 alternating samples per side (`evidence/performance-0.3.0/cumulative/`). Matrix
rows use 0.2.0 lean JSON scores as their baseline because 0.2.0 has no matrix API. The
`parallel` row keeps its four workers on this 2-vCPU host.

| Workload | 0.2.0 ms | 0.3.0 ms | Before / after |
| --- | ---: | ---: | ---: |
| Single prepared evaluation (`one`) | 0.0019 | 0.0014 | 1.30x |
| 96 full records, one worker (`serial`) | 0.149 | 0.084 | 1.76x |
| 96 full records, four workers (`parallel`) | 0.365 | 0.200 | 1.82x |
| 12 traced records (`trace`) | 0.102 | 0.030 | 3.40x |
| 1,024 invalid candidates (`invalid`) | 1.504 | 1.468 | 1.02x |
| 48 domain cases (`domain`) | 3.742 | 1.011 | 3.70x |
| 48 secondary domain cases (`domain2`) | 4.398 | 0.954 | 4.61x |
| 96 lean scores | 0.113 | 0.071 | 1.59x |
| 48 lean domain scores | 3.388 | 0.839 | 4.04x |
| 48 lean secondary scores | 4.339 | 0.901 | 4.82x |
| 96 matrix rows (baseline 0.2.0 lean) | 0.104 | 0.034 | 3.06x |
| 48 matrix domain rows (baseline 0.2.0 lean) | 3.458 | 0.899 | 3.84x |

The comparisons exclude process start-up and model preparation, warm up each prepared
evaluator, and measure JSON decoding (except for the matrix API), evaluation, checks and
encoding. Ratios of individual rounds do not multiply into the cumulative ratio, because
the timings were rerun and host speed varied between sessions. The `invalid` workload is
dominated by decoding 64,512 JSON numbers, which no round targeted; the matrix API avoids
it (18.7 us instead of 1.33 ms per call, `breakdown.json`). A first complete run of the
table is kept in `cumulative/run1/`: a host slowdown hit both sides during samples 1 to 8
of its matrix-domain row, so the table was repeated once and the repeat is reported.
The largest run-to-run differences were matrix `parallel` (2.13x and 1.63x) and lean
`domain` (3.58x and 4.04x); every other row differed by at most 0.21.

Supplementary rows from the same run: one lean score 1.32x, one matrix row against one
0.2.0 lean score 3.01x, 48 matrix secondary rows 5.10x and 96 matrix rows with four
workers 1.63x. The architecture guide adds the time breakdown, per-call costs, worker
scaling and monitored-budget costs.

## Installed Python optimization

Seven runs per configuration of `validation/optimize_performance.py`, with the same seed,
bounds, starting point, optimizer budget and final traced replay, pinned with
`taskset -c 0`. The configurations ran interleaved in rotating order. The 0.3.0 wheel was
built from the extracted source distribution; each wheel was installed into its own fresh
CPython 3.12.3 environment with NumPy 2.3.5 and SciPy 1.17.0. Times include preparation,
objective evaluation and traced winner replay, and exclude Python imports and wheel
compilation. The total median is calculated per run, separately from the column medians.

| Configuration | Prepare ms | Search ms | Replay ms | Total ms | Baseline / total |
| --- | ---: | ---: | ---: | ---: | ---: |
| v0.2.0-full | 0.099 | 3.428 | 0.050 | 3.572 | 1.00x |
| v0.2.0-scores | 0.098 | 3.393 | 0.052 | 3.544 | 1.01x |
| v0.3.0-full | 0.095 | 3.280 | 0.039 | 3.476 | 1.03x |
| v0.3.0-scores | 0.093 | 3.359 | 0.043 | 3.495 | 1.02x |
| v0.3.0-matrix | 0.094 | 2.999 | 0.065 | 3.164 | 1.13x |

Every run made **16** objective calls and classified **0** failures. The selected vector
was `[0.9178665376954516, 0.004332725510795259]` with replay score **17**. All five
configurations agreed on evaluation and failure counts, final parameters and replay score
(`evidence/performance-0.3.0/python-optimization.json`). SciPy's own work dominates
this 16-call search (about 0.2 ms per call against about 1.4 us of native evaluation), so
the end-to-end gain is modest; the matrix mode saves about 18 us of Python-side JSON work
per call. The wheel used for these timings was the release candidate, whose files,
including the native extension, are identical in content to the shipped wheel
(`evidence/release-0.3.0/packages.json`).

## Release checks

- Rust formatting, Clippy with the `python` feature and warnings denied, and the 12 locked
  release tests passed (`evidence/release-0.3.0/rust-checks.txt`). The CLI request, the
  capabilities query and the native sweep ran (`evidence/cli.json`,
  `evidence/capabilities.json`, `evidence/native-sweep.json`).
- The independent domain validation passed against the release CLI and reproduced
  `models/example.json` and `evidence/validation.json` byte for byte.
- The source distribution was built, extracted, and the wheel built inside it. The wheel
  installed into fresh CPython 3.12 environments; its ABI is CPython 3.10+ and its target
  is Linux x86-64.
- `validation/installed.py` passed with the installed wheel outside the source tree. The
  installed lean, full and matrix paths agreed on results, including mixed valid and
  invalid candidates, worker counts, fixed work limits and cancellation; ragged, empty,
  oversized and 1-D NumPy matrices were rejected. All 5 Python blocks of the tutorial and
  all 18 of the user manual ran (`evidence/tutorial-check.json`); a separate script
  confirmed that each manual block prints exactly the output shown in the manual.
- SQLite cancellation and resume retained 21 attempts and recovered 12 completed rows.
  Two full manifest runs against the same database produced identical output
  (`evidence/campaign-replay.json`).
- `cargo package --locked` built the crate archive and verified it by compiling the
  packaged crate.
- These checks ran on the release candidate and again on the final wheel in a new fresh
  environment (`evidence/release-0.3.0/final-*.json`); both passed.
- All six Python examples and the Rust example ran; their outputs are in
  `examples/python/outputs/` and `examples/outputs/`. Scores and statuses reproduce
  exactly between runs; printed times vary.

## Reproduction

```sh
# Baseline and release benchmark binaries
git checkout ca6f4aa && cargo build --release --locked --example performance
cp target/release/examples/performance /tmp/performance-0.2.0
git checkout -  && cargo build --release --locked --example performance
python validation/performance_pair.py /tmp/performance-0.2.0 target/release/examples/performance \
    models/example.json domain --samples 11
python validation/performance_pair.py /tmp/performance-0.2.0 target/release/examples/performance \
    models/example.json serial --samples 11 --before-scores --after-matrix
# Time breakdown and worker scaling of one binary
target/release/examples/performance models/example.json domain breakdown
CRAFT_MATRIX=1 CRAFT_WORKERS=2 target/release/examples/performance models/example.json domain
# Installed Python optimization (repeat per configuration)
taskset -c 0 python validation/optimize_performance.py rail_craft --matrix
```

To reconstruct the history in Git, commit the unmodified 0.2.0 delivery and apply
`patches/0.3.0/*.patch` in filename order with `git am`. The series contains the ten
round commits, followed by the documentation and examples commit and the release
commits. The release packages, the patch series itself and `SHA256SUMS` were added after
the series was generated.
