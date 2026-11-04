# Python examples

Each script answers one question with the installed `rail_craft` package. Run them from
this directory, so `corridor.py` (the shared synthetic models) is importable:

```sh
cd examples/python
python policy_grid.py
```

The recorded output of every script is in [`outputs/`](outputs/). Times printed by the
scripts vary with the host; the scores, statuses and step counts are deterministic.
The fcmaes examples silence fcmaes logging with `from loguru import logger; logger.remove()`.
Requirements: `rail_craft` 0.3.0, NumPy and SciPy; the fcmaes examples also need fcmaes
(tested with 2.1.0).

| Script | Kind | Question | API used |
| --- | --- | --- | --- |
| [`policy_grid.py`](policy_grid.py) | Application: parameter study | Which dispatch weights minimize priority-weighted lateness on a 16-train corridor, and how much does the priority/age ratio matter? | `score_matrix` over 1,681 policies with 2 workers, traced `evaluate` replay |
| [`closure_scenarios.py`](closure_scenarios.py) | Application: scenario comparison | What does a 10 to 30 tick maintenance closure or slower block running cost, and which policy has the lowest regret? Includes block occupancy and a horizon failure. | `batch` with full metrics, traced `evaluate` |
| [`capacity_sensitivity.py`](capacity_sensitivity.py) | Application: sensitivity | How do demand, headway, block run time and clearance change the best achievable lateness? | `score_matrix` per model variant |
| [`optimize_scipy.py`](optimize_scipy.py) | Optimization: SciPy | Tune the weights under a closure with vectorized differential evolution, compare with a grid search and a held-out scenario. | `score_matrix` on whole populations |
| [`optimize_fcmaes_cmaes.py`](optimize_fcmaes_cmaes.py) | Optimization: fcmaes ask/tell | Minimize mean lateness over three closure scenarios with CMA-ES, scoring each population with one call per scenario. | `fcmaes.cmaes.Cmaes.ask/tell`, `score_matrix` with 2 workers |
| [`optimize_fcmaes_bite.py`](optimize_fcmaes_bite.py) | Optimization: fcmaes minimizer with failure penalties | Find one policy for the corridor and a passing-loop line on which some policies deadlock. | `fcmaes.bitecpp.minimize`, single-row `score_matrix`, deadlock diagnostics |

[`corridor.py`](corridor.py) builds the two synthetic layouts: `corridor()` (opposing
trains share one central block) and `passing_loop()` (a single-track line with one passing
loop, where the greedy dispatcher can deadlock).

The Rust example [`../matrix_search.rs`](../matrix_search.rs) runs a 21 x 21 grid search
with `Prepared::score_matrix`; its output is in [`../outputs/matrix_search.txt`](../outputs/matrix_search.txt):

```sh
cargo run --release --locked --example matrix_search -- models/example.json
```

Observations from the recorded runs:

- On the corridor, the best policies weight priority about 6 to 16 times more than
  waiting age; FIFO by waiting age is about 50% worse than the best policy.
- Block run time and clearance enter the block cycle as a sum: run time 3 with clearance 1
  and run time 4 with clearance 0 give the same best lateness.
- The corridor's best policy deadlocks on the passing loop. The best policy for both lines
  weights waiting age above priority and costs the corridor about 50% more lateness.
