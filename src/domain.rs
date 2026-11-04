use crate::{Candidate, Evaluation, Work, infeasible, invalid};
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
    waiting_since: u64,
    held: Option<Interval>,
    arrival: Option<u64>,
}
#[derive(Default)]
struct ResourceState {
    owner: Option<usize>,
    free_after: u64,
    closures: u32,
}

pub fn evaluate(model: &Model, candidate: &Candidate, work: &mut Work<'_>) -> Evaluation {
    let &[priority, age] = candidate.parameters.as_slice() else {
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
    let mut events = model.initial_events.clone();
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
        while events
            .peek()
            .is_some_and(|Reverse((event_time, _, _))| *event_time == at)
        {
            if let Err(e) = work.step() {
                return Err(e.with_metrics(
                    json!({"closed_intervals": candidate.trace.then_some(&intervals), "completed":completed, "time":time}),
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
                if resource.owner.is_some() || resource.free_after > time || resource.closures != 0
                {
                    continue;
                }
                let score = weighted_priority[i] + age * (time - state.waiting_since) as f64;
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
                    json!({"closed_intervals":candidate.trace.then_some(&intervals),"completed":completed,"time":time}),
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
            json!({"completed":completed,"time":time,"closed_intervals":candidate.trace.then_some(&intervals),"active":candidate.trace.then_some(active)}),
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
        "intervals":candidate.trace.then_some(intervals)})
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
