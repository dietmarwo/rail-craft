use crate::{Evaluation, Work, infeasible, invalid};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    cmp::Reverse,
    collections::{BTreeSet, BinaryHeap},
};

pub const CAPABILITIES: &[&str] = &[
    "integer_tick_fixed_routes",
    "blocking_capacity_one",
    "entry_closures",
    "trace_validation",
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Model {
    resources: Vec<Resource>,
    trains: Vec<Train>,
    horizon: u64,
    #[serde(default)]
    closures: Vec<Closure>,
    #[serde(skip)]
    initial_events: BinaryHeap<Event>,
    #[serde(skip)]
    closed_by_resource: Vec<Vec<(u64, u64)>>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Resource {
    name: String,
    clearance: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Train {
    name: String,
    release: u64,
    planned: u64,
    priority: f64,
    route: Vec<Leg>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Leg {
    resource: usize,
    duration: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Closure {
    resource: usize,
    start: u64,
    end: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Interval {
    train: usize,
    leg: usize,
    resource: usize,
    entry: u64,
    service_end: u64,
    exit: u64,
}

type Event = Reverse<(u64, u8, usize)>;

pub fn prepare(mut model: Model) -> Result<Model, String> {
    if !(1..=128).contains(&model.resources.len())
        || !(1..=128).contains(&model.trains.len())
        || !(1..=1_000_000_000).contains(&model.horizon)
        || model.closures.len() > 512
    {
        return Err("invalid corridor dimensions or horizon".into());
    }
    let mut names = BTreeSet::new();
    for r in &model.resources {
        if r.name.is_empty() || !names.insert(&r.name) || r.clearance > model.horizon {
            return Err("invalid resource name or clearance".into());
        }
    }
    names.clear();
    for t in &model.trains {
        if t.name.is_empty()
            || !names.insert(&t.name)
            || t.release > model.horizon
            || t.planned > model.horizon
            || !t.priority.is_finite()
            || !(0.0..=1000.0).contains(&t.priority)
            || !(1..=64).contains(&t.route.len())
            || t.route.iter().any(|l| {
                l.duration == 0 || l.duration > model.horizon || l.resource >= model.resources.len()
            })
            || t.route.windows(2).any(|w| w[0].resource == w[1].resource)
        {
            return Err("invalid train, timing, or route".into());
        }
    }
    if model
        .closures
        .iter()
        .any(|c| c.resource >= model.resources.len() || c.start >= c.end || c.end > model.horizon)
    {
        return Err("invalid closure interval".into());
    }
    // Order at a tick: completions, closure ends, closure starts, releases, wakes.
    let mut events = Vec::with_capacity(model.trains.len() + 2 * model.closures.len());
    for (i, train) in model.trains.iter().enumerate() {
        events.push(Reverse((train.release, 3, i)));
    }
    for (i, c) in model.closures.iter().enumerate() {
        events.push(Reverse((c.start, 2, i)));
        events.push(Reverse((c.end, 1, i)));
    }
    model.initial_events = BinaryHeap::from(events);
    let mut closures = vec![Vec::new(); model.resources.len()];
    for c in &model.closures {
        closures[c.resource].push((c.start, c.end));
    }
    model.closed_by_resource = closures
        .into_iter()
        .map(|mut rows| {
            rows.sort_unstable();
            let mut merged: Vec<(u64, u64)> = Vec::new();
            for (start, end) in rows {
                if let Some(last) = merged.last_mut()
                    && start <= last.1
                {
                    last.1 = last.1.max(end);
                } else {
                    merged.push((start, end));
                }
            }
            merged
        })
        .collect();
    Ok(model)
}

#[derive(Default)]
struct TrainState {
    next: usize,
    held: Option<Interval>,
    arrival: Option<u64>,
}

/// Map a finite score to an unsigned integer with the same order as `f64::total_cmp`.
#[inline]
fn order_key(score: f64) -> u64 {
    let bits = score.to_bits();
    bits ^ ((((bits as i64) >> 63) as u64) | (1 << 63))
}

/// Tie-break tail of a ready train: an earlier waiting start ranks higher, then a lower
/// train index. Waiting starts never exceed the horizon (at most 10^9 < 2^57) and train
/// indexes are below 128, so the tail fits in 64 bits.
#[inline]
fn tie_tail(waiting_since: u64, train: usize) -> u64 {
    (((1_u64 << 57) - 1 - waiting_since) << 7) | (127 - train as u64)
}
#[derive(Default)]
struct ResourceState {
    owner: Option<usize>,
    free_after: u64,
    closures: u32,
}

pub fn evaluate(model: &Model, parameters: &[f64], trace: bool, work: &mut Work<'_>) -> Evaluation {
    let &[priority, age] = parameters else {
        return Err(invalid("expected priority and waiting-time weights"));
    };
    if ![priority, age].iter().all(|v| (0.0..=10.0).contains(v)) {
        return Err(invalid("weights must be in [0,10]"));
    }
    let mut trains: Vec<_> = model.trains.iter().map(|_| TrainState::default()).collect();
    let mut resources: Vec<_> = model
        .resources
        .iter()
        .map(|_| ResourceState::default())
        .collect();
    let weighted_priority: Vec<_> = model.trains.iter().map(|t| priority * t.priority).collect();
    // Dense dispatch state: waiting start and tie-break tail per ready train.
    let mut since_of = vec![0_u64; model.trains.len()];
    let mut tail_of = vec![0_u64; model.trains.len()];
    let mut events = model.initial_events.clone();
    let mut intervals = Vec::new();
    let mut completed = 0;
    // Ready trains grouped by the resource of their next leg, plus the resources
    // that have at least one ready train. Both models are limited to 128 entries.
    let mut ready_on = vec![0_u128; model.resources.len()];
    let mut waiting_resources = 0_u128;
    let mut time = 0;
    let release = |mut interval: Interval,
                   time,
                   resources: &mut [ResourceState],
                   intervals: &mut Vec<Interval>,
                   events: &mut BinaryHeap<_>| {
        interval.exit = time;
        let r = interval.resource;
        resources[r].owner = None;
        resources[r].free_after = time + model.resources[r].clearance;
        if resources[r].free_after > time {
            events.push(Reverse((resources[r].free_after, 4, r)));
        }
        intervals.push(interval);
    };
    while let Some(&Reverse((at, _, _))) = events.peek() {
        if at > model.horizon {
            break;
        }
        time = at;
        while events
            .peek()
            .is_some_and(|Reverse((event_time, _, _))| *event_time == at)
        {
            if let Err(e) = work.step() {
                return Err(e.with_metrics(
                    json!({"closed_intervals": trace.then_some(&intervals), "completed":completed, "time":time}),
                ));
            }
            let Reverse((_, kind, i)) = events.pop().expect("peeked event");
            match kind {
                0 => {
                    let state = &mut trains[i];
                    if state.next == model.trains[i].route.len() {
                        release(
                            state.held.take().expect("active train"),
                            time,
                            &mut resources,
                            &mut intervals,
                            &mut events,
                        );
                        state.arrival = Some(time);
                        completed += 1;
                    } else {
                        let r = model.trains[i].route[state.next].resource;
                        ready_on[r] |= 1_u128 << i;
                        waiting_resources |= 1_u128 << r;
                        since_of[i] = time;
                        tail_of[i] = tie_tail(time, i);
                    }
                }
                1 => resources[model.closures[i].resource].closures -= 1,
                2 => resources[model.closures[i].resource].closures += 1,
                3 => {
                    let r = model.trains[i].route[0].resource;
                    ready_on[r] |= 1_u128 << i;
                    waiting_resources |= 1_u128 << r;
                    since_of[i] = time;
                    tail_of[i] = tie_tail(time, i);
                }
                _ => {}
            }
        }
        loop {
            // Rank (score, earlier waiting start, lower train index) over trains whose
            // next resource is available. Blocked resources are skipped as a group.
            // `order_key` is zero only for one NaN pattern and scores are finite, so
            // a zero maximum means that no train is eligible.
            let mut best = 0_u128;
            let mut open = waiting_resources;
            while open != 0 {
                let r = open.trailing_zeros() as usize;
                open &= open - 1;
                let resource = &resources[r];
                if resource.owner.is_some() || resource.free_after > time || resource.closures != 0
                {
                    continue;
                }
                let mut contenders = ready_on[r];
                while contenders != 0 {
                    let i = contenders.trailing_zeros() as usize;
                    contenders &= contenders - 1;
                    // The wait is below 2^63, so the i64 conversion equals the u64 one.
                    let wait = (time - since_of[i]) as i64 as f64;
                    let score = weighted_priority[i] + age * wait;
                    let key = (u128::from(order_key(score)) << 64) | u128::from(tail_of[i]);
                    best = best.max(key);
                }
            }
            if best == 0 {
                break;
            }
            let i = 127 - (best as usize & 127);
            if let Err(e) = work.step() {
                return Err(e.with_metrics(
                    json!({"closed_intervals":trace.then_some(&intervals),"completed":completed,"time":time}),
                ));
            }
            let state = &mut trains[i];
            if let Some(previous) = state.held.take() {
                release(previous, time, &mut resources, &mut intervals, &mut events);
            }
            let leg = &model.trains[i].route[state.next];
            let end = time + leg.duration;
            resources[leg.resource].owner = Some(i);
            state.held = Some(Interval {
                train: i,
                leg: state.next,
                resource: leg.resource,
                entry: time,
                service_end: end,
                exit: end,
            });
            state.next += 1;
            ready_on[leg.resource] &= !(1_u128 << i);
            if ready_on[leg.resource] == 0 {
                waiting_resources &= !(1_u128 << leg.resource);
            }
            events.push(Reverse((end, 0, i)));
        }
        if completed == trains.len() {
            break;
        }
    }
    if completed != trains.len() {
        let active: Vec<_> = trains.iter().filter_map(|t| t.held.as_ref()).collect();
        let reason = if events.is_empty() {
            "deadlock"
        } else {
            "horizon"
        };
        return Err(infeasible(reason).with_metrics(
            json!({"completed":completed,"time":time,"closed_intervals":trace.then_some(&intervals),"active":trace.then_some(active)}),
        ));
    }
    validate_trace(model, &intervals).map_err(invalid)?;
    let delay: Vec<_> = trains
        .iter()
        .zip(&model.trains)
        .map(|(s, t)| s.arrival.expect("completed").saturating_sub(t.planned))
        .collect();
    let score: f64 = delay
        .iter()
        .zip(&model.trains)
        .map(|(&d, t)| d as f64 * (1.0 + t.priority))
        .sum();
    Ok((
        score,
        work.metrics(|| {
            json!({"completed":completed,"max_delay":delay.iter().max(),"delays":delay,
        "intervals":trace.then_some(intervals)})
        }),
    ))
}

pub fn validate_trace(model: &Model, intervals: &[Interval]) -> Result<(), String> {
    let mut by_train = vec![Vec::new(); model.trains.len()];
    let mut by_resource = vec![Vec::new(); model.resources.len()];
    for row in intervals {
        let Some(train) = model.trains.get(row.train) else {
            return Err("unknown train".into());
        };
        let Some(leg) = train.route.get(row.leg) else {
            return Err("unknown movement".into());
        };
        let closed = &model.closed_by_resource[leg.resource];
        let next = closed.partition_point(|&(start, _)| start <= row.entry);
        if row.resource != leg.resource
            || row.entry < train.release
            || row.exit > model.horizon
            || row.entry.checked_add(leg.duration) != Some(row.service_end)
            || row.service_end > row.exit
            || (next > 0 && row.entry < closed[next - 1].1)
        {
            return Err("resource, release, service, horizon, or closure violation".into());
        }
        by_train[row.train].push(row);
        by_resource[row.resource].push(row);
    }
    for (rows, train) in by_train.iter_mut().zip(&model.trains) {
        rows.sort_by_key(|row| row.leg);
        if rows.len() != train.route.len()
            || rows.iter().enumerate().any(|(i, r)| i != r.leg)
            || rows.windows(2).any(|w| w[0].exit != w[1].entry)
            || rows.last().is_none_or(|r| r.exit != r.service_end)
        {
            return Err("missing, repeated, or disconnected movement".into());
        }
    }
    for (rows, r) in by_resource.iter_mut().zip(&model.resources) {
        rows.sort_by_key(|row| row.entry);
        if rows
            .windows(2)
            .any(|w| w[0].exit + r.clearance > w[1].entry)
        {
            return Err("resource capacity or clearance violation".into());
        }
    }
    Ok(())
}

pub fn verify_json(model: &str, trace: &str) -> Result<serde_json::Value, String> {
    let model = prepare(serde_json::from_str(model).map_err(|e| e.to_string())?)?;
    let rows: Vec<Interval> = serde_json::from_str(trace).map_err(|e| e.to_string())?;
    validate_trace(&model, &rows)?;
    Ok(json!({"valid":true,"movements":rows.len()}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Budget;
    use serde_json::Value;

    #[derive(Default)]
    struct TrainState {
        next: usize,
        waiting_since: u64,
        held: Option<Interval>,
        arrival: Option<u64>,
    }

    /// The 0.2.0 dispatcher and verifier, kept verbatim apart from building its own
    /// event heap, as an oracle for structural changes to `evaluate`.
    fn reference(
        model: &Model,
        parameters: &[f64],
        trace: bool,
        work: &mut Work<'_>,
    ) -> Evaluation {
        let &[priority, age] = parameters else {
            return Err(invalid("expected priority and waiting-time weights"));
        };
        if ![priority, age].iter().all(|v| (0.0..=10.0).contains(v)) {
            return Err(invalid("weights must be in [0,10]"));
        }
        let mut trains: Vec<_> = model.trains.iter().map(|_| TrainState::default()).collect();
        let mut resources: Vec<_> = model
            .resources
            .iter()
            .map(|_| ResourceState::default())
            .collect();
        let mut events = BinaryHeap::new();
        for (i, train) in model.trains.iter().enumerate() {
            events.push(Reverse((train.release, 3_u8, i)));
        }
        for (i, c) in model.closures.iter().enumerate() {
            events.push(Reverse((c.start, 2, i)));
            events.push(Reverse((c.end, 1, i)));
        }
        let mut intervals = Vec::new();
        let mut completed = 0;
        let mut ready = 0_u128;
        let mut time = 0;
        let release = |mut interval: Interval,
                       time,
                       resources: &mut [ResourceState],
                       intervals: &mut Vec<Interval>,
                       events: &mut BinaryHeap<_>| {
            interval.exit = time;
            let r = interval.resource;
            resources[r].owner = None;
            resources[r].free_after = time + model.resources[r].clearance;
            if resources[r].free_after > time {
                events.push(Reverse((resources[r].free_after, 4, r)));
            }
            intervals.push(interval);
        };
        while let Some(&Reverse((at, _, _))) = events.peek() {
            if at > model.horizon {
                break;
            }
            time = at;
            while events.peek().is_some_and(|Reverse((t, _, _))| *t == at) {
                if let Err(e) = work.step() {
                    return Err(e.with_metrics(
                        json!({"closed_intervals": trace.then_some(&intervals), "completed":completed, "time":time}),
                    ));
                }
                let Reverse((_, kind, i)) = events.pop().expect("peeked event");
                match kind {
                    0 => {
                        let state = &mut trains[i];
                        if state.next == model.trains[i].route.len() {
                            release(
                                state.held.take().expect("active train"),
                                time,
                                &mut resources,
                                &mut intervals,
                                &mut events,
                            );
                            state.arrival = Some(time);
                            completed += 1;
                        } else {
                            ready |= 1_u128 << i;
                            state.waiting_since = time;
                        }
                    }
                    1 => resources[model.closures[i].resource].closures -= 1,
                    2 => resources[model.closures[i].resource].closures += 1,
                    3 => {
                        ready |= 1_u128 << i;
                        trains[i].waiting_since = time;
                    }
                    _ => {}
                }
            }
            loop {
                let mut contenders = ready;
                let mut best = None::<(usize, u64, f64)>;
                while contenders != 0 {
                    let i = contenders.trailing_zeros() as usize;
                    contenders &= contenders - 1;
                    let state = &trains[i];
                    let resource = &resources[model.trains[i].route[state.next].resource];
                    if resource.owner.is_some()
                        || resource.free_after > time
                        || resource.closures != 0
                    {
                        continue;
                    }
                    let score = priority * model.trains[i].priority
                        + age * (time - state.waiting_since) as f64;
                    if best.is_none_or(|(_, since, previous)| {
                        score
                            .total_cmp(&previous)
                            .then_with(|| since.cmp(&state.waiting_since))
                            .is_gt()
                    }) {
                        best = Some((i, state.waiting_since, score));
                    }
                }
                let Some((i, _, _)) = best else { break };
                if let Err(e) = work.step() {
                    return Err(e.with_metrics(
                        json!({"closed_intervals":trace.then_some(&intervals),"completed":completed,"time":time}),
                    ));
                }
                let state = &mut trains[i];
                if let Some(previous) = state.held.take() {
                    release(previous, time, &mut resources, &mut intervals, &mut events);
                }
                let leg = &model.trains[i].route[state.next];
                let end = time + leg.duration;
                resources[leg.resource].owner = Some(i);
                state.held = Some(Interval {
                    train: i,
                    leg: state.next,
                    resource: leg.resource,
                    entry: time,
                    service_end: end,
                    exit: end,
                });
                state.next += 1;
                ready &= !(1_u128 << i);
                events.push(Reverse((end, 0, i)));
            }
            if completed == trains.len() {
                break;
            }
        }
        if completed != trains.len() {
            let active: Vec<_> = trains.iter().filter_map(|t| t.held.as_ref()).collect();
            let reason = if events.is_empty() {
                "deadlock"
            } else {
                "horizon"
            };
            return Err(infeasible(reason).with_metrics(
                json!({"completed":completed,"time":time,"closed_intervals":trace.then_some(&intervals),"active":trace.then_some(active)}),
            ));
        }
        validate_trace(model, &intervals).map_err(invalid)?;
        let delay: Vec<_> = trains
            .iter()
            .zip(&model.trains)
            .map(|(s, t)| s.arrival.expect("completed").saturating_sub(t.planned))
            .collect();
        let score: f64 = delay
            .iter()
            .zip(&model.trains)
            .map(|(&d, t)| d as f64 * (1.0 + t.priority))
            .sum();
        Ok((
            score,
            work.metrics(|| {
                json!({"completed":completed,"max_delay":delay.iter().max(),"delays":delay,
            "intervals":trace.then_some(intervals)})
            }),
        ))
    }

    /// Small deterministic generator for reproducible random corridors.
    struct Lcg(u64);
    impl Lcg {
        fn below(&mut self, n: u64) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 33) % n
        }
    }

    fn random_model(seed: u64) -> Value {
        let mut rng = Lcg(seed);
        let resources = 1 + rng.below(6) as usize;
        let trains = if seed.is_multiple_of(17) {
            100 + rng.below(29) as usize
        } else {
            1 + rng.below(14) as usize
        };
        let horizon = 30 + rng.below(400);
        let priorities = [0.0, 1.0, 2.5, 3.0, 0.125, 1000.0];
        let train_rows: Vec<_> = (0..trains)
            .map(|i| {
                let legs = 1 + rng.below(4.min(resources as u64 + 1));
                let mut route = Vec::new();
                let mut last = usize::MAX;
                for _ in 0..legs {
                    let mut r = rng.below(resources as u64) as usize;
                    if r == last {
                        r = (r + 1) % resources;
                    }
                    if r == last {
                        break;
                    }
                    last = r;
                    route.push(json!({"resource": r, "duration": 1 + rng.below(5)}));
                }
                json!({"name": format!("T{i}"), "release": rng.below(12), "planned": rng.below(25),
                       "priority": priorities[rng.below(6) as usize], "route": route})
            })
            .collect();
        let closures: Vec<_> = (0..rng.below(8))
            .map(|_| {
                let start = rng.below(40);
                json!({"resource": rng.below(resources as u64), "start": start,
                       "end": start + 1 + rng.below(6)})
            })
            .collect();
        let resource_rows: Vec<_> = (0..resources)
            .map(|r| json!({"name": format!("R{r}"), "clearance": rng.below(3)}))
            .collect();
        json!({"resources": resource_rows, "trains": train_rows, "horizon": horizon,
               "closures": closures})
    }

    fn run(
        kernel: fn(&Model, &[f64], bool, &mut Work<'_>) -> Evaluation,
        model: &Model,
        parameters: &[f64],
        trace: bool,
        budget: &Budget,
    ) -> Value {
        let mut work = Work {
            budget,
            deadline: None,
            used: 0,
            monitored: false,
            metrics_enabled: true,
        };
        let result = kernel(model, parameters, trace, &mut work);
        match result {
            Ok((score, metrics)) => json!({"status": "complete", "score": score,
                                          "metrics": metrics, "steps": work.used}),
            Err(e) => json!({"status": e.status, "detail": e.detail, "metrics": e.metrics,
                             "steps": work.used}),
        }
    }

    #[test]
    fn ranking_keys_follow_total_order_and_tie_breaks() {
        let values = [
            -1e300,
            -2.5,
            -f64::MIN_POSITIVE,
            -0.0,
            0.0,
            f64::from_bits(1),
            f64::MIN_POSITIVE,
            0.125,
            1.0,
            1.0 + f64::EPSILON,
            9_999.0,
            1e300,
        ];
        for a in values {
            assert_ne!(order_key(a), 0);
            for b in values {
                assert_eq!(order_key(a).cmp(&order_key(b)), a.total_cmp(&b), "{a} {b}");
            }
        }
        assert!(tie_tail(3, 7) > tie_tail(4, 0));
        assert!(tie_tail(3, 7) > tie_tail(3, 8));
        assert!(tie_tail(1_000_000_000, 127) > 0);
        assert_eq!(127 - (tie_tail(1_000_000_000, 127) & 127), 127);
        assert_eq!(127 - (tie_tail(0, 0) & 127), 0);
    }

    #[test]
    fn evaluate_matches_the_reference_dispatcher() {
        let weights = [
            [1.0, 0.0],
            [0.0, 0.0],
            [0.0, 1.0],
            [2.5, 0.75],
            [10.0, 10.0],
            [0.1, 0.3],
            [-0.0, -0.0],
        ];
        let mut outcomes = std::collections::BTreeMap::new();
        for seed in 0..400 {
            let text = random_model(seed).to_string();
            let Ok(model) = prepare(serde_json::from_str(&text).unwrap()) else {
                continue;
            };
            for (k, parameters) in weights.iter().enumerate() {
                let trace = (seed + k as u64).is_multiple_of(2);
                for max_steps in [1_000_000, 1 + seed % 40] {
                    let budget = Budget {
                        max_steps,
                        ..Budget::default()
                    };
                    let expected = run(reference, &model, parameters, trace, &budget);
                    let actual = run(evaluate, &model, parameters, trace, &budget);
                    assert_eq!(actual, expected, "seed {seed} weights {parameters:?}");
                    *outcomes
                        .entry(format!("{}:{}", expected["status"], expected["detail"]))
                        .or_insert(0) += 1;
                }
            }
        }
        // The generator must reach every outcome class the kernel can produce.
        for class in [
            "\"complete\":null",
            "\"work_limit\":\"candidate step budget reached\"",
            "\"infeasible\":\"horizon\"",
            "\"infeasible\":\"deadlock\"",
        ] {
            assert!(
                outcomes.contains_key(class),
                "{class} missing from {outcomes:?}"
            );
        }
    }
}
