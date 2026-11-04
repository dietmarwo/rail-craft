# rail-craft 0.3.0 performance plan

Host: 2 vCPUs, Intel(R) Xeon(R) Processor @ 2.10GHz (KVM), Rust 1.97.0, CPython 3.12.
Every timing runs `validation/performance_pair.py PREVIOUS NEW models/example.json WORKLOAD --samples 11`
inside `/home/claude/craft5/cpu.lock`, where PREVIOUS is the previous kept round's benchmark binary
from `/home/claude/craft5/bench/rail-craft/roundNN/performance`.

Decision rule for every round (fixed before timing): keep the round if the declared workload's
median speedup is at least 1.03x and the paired output check passes; otherwise revert it,
save its timing under `rejected/` and try something else.

## Profile of the 0.2.0 baseline (callgrind, instructions)

- `domain` (110 trains, one resource): 85% of instructions are in `domain::evaluate`'s dispatch
  scan. Each ready contender is checked for resource availability, so every tick with an event
  rescans all ~100 waiting trains, including ticks where the only resource is in clearance.
  `validate_trace` is about 6%, heap operations about 3%, metrics construction about 1%.
- `domain2` (100 trains, 300 closures): the same scan dominates; the heap holds 600 static
  closure events and costs about 9%.
- `serial` (96 example candidates): malloc/free is about 25%, JSON serialization about 7%,
  the domain kernel about 12%, candidate decoding about 5%.

## Rounds

### Round 1: typed score-matrix API

- Workload: `serial` (96 candidates, one worker). Before: round 0 with `--before-scores`
  (lean `scores_json`). After: round 1 with `--after-matrix` (`score_matrix`).
- Expected effect: removes candidate JSON decoding, ID checks and record JSON encoding;
  expect a large gain (more than 1.3x) because the example kernel is small.

### Round 2: per-resource ready masks in the dispatcher

- Workload: `domain` (48 full records, 110 trains on one resource), round 1 against round 2.
- Change: keep one 128-bit ready mask per resource (trains whose next leg uses it) and a
  128-bit mask of resources with waiting trains. A dispatch pass checks each such resource's
  availability once and ranks only the trains waiting for available resources. The ranking
  key stays (score, earlier waiting start, lower train index), now compared explicitly.
- Expected effect: most passes happen while the single block is owned or in clearance; those
  passes become O(1) instead of a scan over about 100 waiting trains. Expect more than 1.5x.

### Round 3: rank contenders with one integer key

- Workload: `domain` in matrix mode (`--before-matrix --after-matrix`, 48 candidates,
  110 trains), round 2 against round 3.
- Change: store waiting starts in a dense array and precompute a 64-bit tie-break tail
  (later waiting start loses, then higher index loses) when a train becomes ready. Each
  contender's score is mapped to a total-order integer and combined with the tail into one
  u128, so ranking is a single unsigned maximum. The age term converts the wait through
  `i64`, which yields the same `f64` for every possible wait.
- Expected effect: the per-contender comparator chain is the largest remaining cost
  (about 60% of instructions). Expect 1.2x to 1.4x.

### Round 4: static event timeline and packed dynamic heap

- Workload: `domain2` (48 full records, 100 trains, 300 closures), round 3 against round 4.
- Change: releases and closure starts and ends do not depend on the candidate. Sort them once
  at preparation into a timeline of packed `u64` keys (tick, kind, index), which preserve the
  tuple order. Each candidate walks the timeline with a cursor and merges it with a small
  binary heap that holds only service completions and clearance wakes. This removes the
  per-candidate heap clone and the logarithmic pops of 700 static events.
- Expected effect: heap work is about 19% of `domain2` instructions after round 3, plus the
  clone. Expect 1.1x to 1.25x.
