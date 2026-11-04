# rail-craft implementation log

3 November 2026. Version 0.1.0. This delivery uses the bounded research
scope recorded below. The independent references and optional comparisons
are identified separately for each increment.

## Increment results

| Increment | Status | Implementation and evidence |
| --- | --- | --- |
| 0 | Implemented on synthetic inputs | Specified the fixed blocking corridor and the one-block case. The model card separates tick-based dispatch from a grid adapter and detailed railway dynamics. |
| 1 | Implemented | Capacity-one event queue, stable same-tick ordering, versioned native CLI and installed Python evaluator. |
| 2 | Implemented | Independent trace replay checks resource, closure, clearance, service duration and continuity. Five corruptions are rejected. |
| 3 | Implemented | Blocking transfers, opposing routes, dedicated platforms, release and horizon accounting. The blocking-cycle fixture returns deadlock. |
| 4 | Implemented | Seeded entry-closure scenarios. The dispatcher sees current availability and waiting ages. A closure beginning exactly at service completion prevents a new entry. |
| 5 | Implemented on synthetic inputs | Six policy vectors tested. Priority-weighted delay, maximum delay and held-out completion are recorded. The native sweep and the optional in-process SciPy search run. |
| 6 | Implemented on a bounded subset | Complete non-idling permutation enumeration for three through six trains with simultaneous release on one block. The trace validator checked the executable policy and the analytical reference applies to the one-block subproblem. |
| 7 | Implemented on a bounded subset | Independent tick reference matched the one-block abstraction. Prepared serial and parallel timings were measured. No paired Flatland or OSRD execution was available. |
| 8 | Local fallback | Seeded training and holdout scenarios, occupancy diagram, local SQLite campaign and cancellation/resume proof. The external workers and collaborator case remain open. |
| 9 | Implemented for tested platform | Verified source, Rust crate, sdist and wheel. The documented release is a synthetic fixed-route dispatch research package. |

## Numerical results

The three-train optimum was **16** weighted ticks versus 40 for the baseline. Across six separate corridor holdout scenarios, aggregate weighted delay was **126** versus 246. Every selected holdout schedule completed.

The optional in-process SciPy differential-evolution demonstration made
16 evaluations and replayed its selected candidate.
It used the packaged example and its own local bounds. The domain-specific
grid and holdout evidence are in `evidence/validation.json`.

## Installed performance and replay

The following are medians of three 96-candidate batch observations,
measured in a clean CPython 3.12 environment with the installed wheel.
The scenarios and candidate inputs are identical within a row pair.
Times include native crossing and compact JSON result conversion.

| Boundary | Time |
| --- | ---: |
| Cold preparation and batch, one worker | 0.597 ms |
| Prepared batch, one worker | 0.492 ms |
| Prepared batch, four workers | 0.638 ms |

The replay test reversed candidate order and changed worker count. It
recovered 12 completed SQLite records across cancellation and retained
21 attempts. This campaign uses a local database and package-build
identity. Cross-machine determinism and distributed worker behavior
were outside the test scope.

## Release evidence

- `cargo fmt`, Clippy with warnings denied and locked Rust release tests passed.
- The native sweep and numerical validation ran against a release build.
- A wheel built from the extracted sdist and installed offline into a fresh
  environment. The wheel target is Linux x86-64, CPython 3.10+ ABI. Actual
  verification used CPython 3.12 on x86_64.
- The standalone Rust crate passed Cargo package verification.
- Model and source manifests are in `docs/`. Raw timing, numerical and
  clean-install results are in `evidence/`.

The source was staged in successive local commits. Each increment above
records its own outcome, while related implementation changes share
commits.

- `b8ffe32` Define versioned evaluation and packaging boundaries
- `7b4116e` Implement bounded domain evaluator and resumable local execution
- `751b10e` Validate model laws and add bounded search and installation examples
- `3b76146` Prepare source distributions and document validated research scope
- `5014c0d` Canonicalize campaign replay output
