# rail-craft performance implementation log

3 November 2026. Version 0.2.0, based on the prior 0.1.0 delivery
at `5014c0d833b3`. The ZIP contains the updated source, runnable
examples, an ordered patch series, [tutorial](docs/tutorial.md),
[architecture](docs/architecture.md), raw evidence and build artifacts.
The [earlier implementation record](docs/implementation-0.1.0.md)
documents the original research scope and remaining external work.

## Ten incremental changes

Each row compares the indicated stage with its immediate predecessor.
Times are median milliseconds per prepared call. A ratio below 1.00×
means the new median was slower for that measurement. Small differences
vary with thread scheduling, code layout and host noise.

| Round | Commit | Change | Case | Before ms | After ms | Before / after |
| ---: | --- | --- | --- | ---: | ---: | ---: |
| 1 | `dfc65e1` | Evaluate singleton and one-worker batches on the calling thread, removing worker startup and join. | `one` | 0.043 | 0.003 | 12.85× |
| 2 | `bea06db` | Replace worker-local row collections and final sorting with one result slot per candidate index. Keep dynamic work assignment. | `parallel` | 0.386 | 0.378 | 1.02× |
| 3 | `dcd736e` | Validate unique candidate IDs through a pre-sized hash set. The large invalid batch isolates ID checks and response overhead from most domain computation. | `invalid` | 2.454 | 2.241 | 1.10× |
| 4 | `c27f595` | Serialize a typed prepared batch directly and keep the public CLI result adapter. | `trace` | 0.138 | 0.076 | 1.81× |
| 5 | `1886f2d` | Borrow candidate IDs until the batch is serialized, eliminating output-side string clones. | `invalid` | 1.480 | 1.494 | 0.99× |
| 6 | `66fb163` | Inline the unmonitored work counter. Keep cancellation, deadline and exhausted-budget checks on the guarded path. | `domain` | 3.442 | 3.809 | 0.90× |
| 7 | `170c329` | Add Rust and Python score-only entry points, preserving status, step counts, score and record order while skipping diagnostic construction. This row compares the earlier full path with the new lean path. | `lean-serial` | 0.155 | 0.098 | 1.58× |
| 8 | `3d8a4cf` | Build the initial event heap once, then clone it for each simulation. | `domain2` | 5.561 | 5.204 | 1.07× |
| 9 | `68d36f9` | Track ready trains in a 128-bit mask, bind weighted priority per train and rank each eligible contender once. | `domain` | 3.240 | 3.259 | 0.99× |
| 10 | `de84445` | Merge and index half-open closures per resource for the interval verifier. Keep event-level closure counting. | `domain2` | 5.844 | 4.870 | 1.20× |

Each stage passed locked Rust release tests and paired output checks
for the supplied workloads. The result comparisons
ignore the package-version string and require matching record order,
status, detail, steps, provenance and numeric values within tolerance.
Lean comparisons also normalize metrics to null. The benchmark commit
precedes the ten rounds. Documentation and version changes follow them.

An initial round 9 used an iterator pipeline for dispatch ranking. It ran at 0.50× on the 110-train case, so it was replaced with the ready-train bitset and stable scan. The discarded samples are in `evidence/performance-discarded-rail-round9.json`. Rounds 9 and 10 were then rebuilt, replayed and timed again.

## Cumulative native results

| Workload | 0.1.0 ms | Final ms | Before / after |
| --- | ---: | ---: | ---: |
| Single prepared evaluation | 0.055 | 0.002 | 28.66× |
| 96 full records, one worker | 0.475 | 0.164 | 2.90× |
| 96 full records, four workers | 0.477 | 0.314 | 1.52× |
| 12 traced records | 0.240 | 0.112 | 2.15× |
| 1,024 invalid candidates | 3.030 | 1.807 | 1.68× |
| 48 domain cases | 5.253 | 4.776 | 1.10× |
| 48 secondary domain cases | 6.921 | 5.253 | 1.32× |
| 96 lean scores | 0.559 | 0.106 | 5.29× |
| 48 lean domain scores | 4.246 | 3.232 | 1.31× |
| 48 lean secondary scores | 6.485 | 4.183 | 1.55× |

The lean API was introduced in 0.2.0. The baseline uses full
records for the three lean rows. The paired comparisons alternate process order,
exclude startup and model preparation, warm up each prepared evaluator,
and measure JSON decode, evaluation, checks and JSON encoding. They
use five observations per side on an AMD EPYC 9V74 with Rust 1.88.0,
using logical CPUs 0–3. These are fixed test workloads, with four
workers only in the parallel row. Ratios from individual stages do
not multiply into the cumulative ratio because the timings were rerun.

The supplemental workloads stress a 110-train single resource and a 100-train resource with 300 closures. The verifier is still part of successful lean evaluations. Overlapping and adjacent closures were checked at their final end tick. The model remains a synthetic fixed-route blocking corridor.

## Installed Python optimization

Seven runs per configuration used the same seed, bounds, starting point,
optimizer budget and final traced replay. CPython 3.12 and SciPy 1.17.0
ran on one logical CPU. Times include preparation, objective evaluation
and traced winner replay, excluding Python imports and wheel compilation.
The total median is calculated per run, separately from the column medians.

| Configuration | Prepare ms | Search ms | Replay ms | Total ms | Baseline / total |
| --- | ---: | ---: | ---: | ---: | ---: |
| v0.1.0-full | 0.098 | 8.442 | 0.084 | 8.611 | 1.00× |
| v0.2.0-full | 0.077 | 7.495 | 0.039 | 7.607 | 1.13× |
| v0.2.0-scores | 0.077 | 7.940 | 0.042 | 8.060 | 1.07× |

The run made **16** objective calls and classified
**0** failures. The selected vector was
`[0.9178665376954516, 0.004332725510795259]` with replay score **17**.
All configurations agreed on evaluation and failure counts, final
parameters and full replay score within the recorded tolerance.

## Release checks

- Rust formatting, Clippy with Python enabled and warnings denied,
  release tests, CLI request and native sweep passed.
- The independent domain validation passed against the release CLI.
- The wheel built from the extracted source distribution and installed
  into a fresh CPython 3.12 environment. Its ABI is CPython 3.10+ and
  its bundled target is Linux x86-64.
- The installed lean and full paths agreed on results, including mixed
  valid/invalid candidates, worker count, fixed work limits and
  cancellation. The tutorial's 4 Python blocks ran.
- SQLite cancellation/resume retained 21
  attempts and recovered 12 completed rows.
  Two full manifest runs against the same database produced identical output.
- The Rust crate archive passed Cargo package verification.

Run the packaged `validation/performance_pair.py` with saved native
benchmark binaries to reproduce comparisons. To reconstruct the ten
rounds in Git, apply `patches/*.patch` in filename order to the source
from the 0.1.0 ZIP. A baseline may first be committed locally. The patch
series includes the benchmark and release documentation commits around
the ten implementation commits.
