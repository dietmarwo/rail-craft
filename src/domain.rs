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
    /// Candidate-independent events (releases, closure starts and ends) as sorted keys.
    #[serde(skip)]
    timeline: Vec<u64>,
    #[serde(skip)]
    closed_by_resource: Vec<Vec<(u64, u64)>>,
    /// Prefix sums of route lengths: train `t` owns verifier slots `leg_offset[t]..[t + 1]`.
    #[serde(skip)]
    leg_offset: Vec<usize>,
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

/// One movement. Fields are declared in sorted key order, so typed serialization
/// emits the same bytes as the sorted `serde_json::Value` maps used before 0.3.0.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Interval {
    entry: u64,
    exit: u64,
    leg: usize,
    resource: usize,
    service_end: u64,
    train: usize,
}

/// Metrics of a complete schedule, in sorted key order.
#[derive(Serialize)]
struct CompleteMetrics<'a> {
    completed: usize,
    delays: &'a [u64],
    intervals: Option<&'a [Interval]>,
    max_delay: Option<&'a u64>,
}

/// Metrics at a work, deadline or cancellation stop, in sorted key order.
#[derive(Serialize)]
struct StopMetrics<'a> {
    closed_intervals: Option<&'a [Interval]>,
    completed: usize,
    time: u64,
}

/// Metrics of an unfinished schedule, in sorted key order.
#[derive(Serialize)]
struct UnfinishedMetrics<'a> {
    active: Option<Vec<&'a Interval>>,
    closed_intervals: Option<&'a [Interval]>,
    completed: usize,
    time: u64,
}

/// Event kinds in processing order at one tick.
const COMPLETION: u64 = 0;
const CLOSURE_END: u64 = 1;
const CLOSURE_START: u64 = 2;
const RELEASE: u64 = 3;
const WAKE: u64 = 4;

/// Pack an event into one key whose unsigned order is the (tick, kind, index) order.
/// Ticks stay below 2^31 (horizon, durations and clearances are at most 10^9 each and an
/// event lies at most one duration or clearance past a tick within the horizon) and
/// indexes below 512 (closures), so the key never reaches `u64::MAX`.
#[inline]
fn event_key(tick: u64, kind: u64, index: usize) -> u64 {
    (tick << 12) | (kind << 9) | index as u64
}

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
    let mut timeline = Vec::with_capacity(model.trains.len() + 2 * model.closures.len());
    for (i, train) in model.trains.iter().enumerate() {
        timeline.push(event_key(train.release, RELEASE, i));
    }
    for (i, c) in model.closures.iter().enumerate() {
        timeline.push(event_key(c.start, CLOSURE_START, i));
        timeline.push(event_key(c.end, CLOSURE_END, i));
    }
    timeline.sort_unstable();
    model.timeline = timeline;
    model.leg_offset = std::iter::once(0)
        .chain(model.trains.iter().scan(0, |total, t| {
            *total += t.route.len();
            Some(*total)
        }))
        .collect();
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

/// Reusable per-worker buffers for `evaluate`. Every buffer is reset before use, so one
/// scratch can serve any sequence of candidates and models.
#[derive(Default)]
pub struct Scratch {
    trains: Vec<TrainState>,
    resources: Vec<ResourceState>,
    weighted_priority: Vec<f64>,
    since_of: Vec<u64>,
    tail_of: Vec<u64>,
    ready_on: Vec<u128>,
    events: BinaryHeap<Reverse<u64>>,
    intervals: Vec<Interval>,
    verify: VerifyScratch,
}

/// Reusable buffers for the interval verifier.
#[derive(Default)]
struct VerifyScratch {
    slots: Vec<usize>,
    starts: Vec<usize>,
    fill: Vec<usize>,
    by_resource: Vec<(u64, u64)>,
}

pub fn evaluate(
    model: &Model,
    parameters: &[f64],
    trace: bool,
    work: &mut Work<'_>,
    scratch: &mut Scratch,
) -> Evaluation {
    let &[priority, age] = parameters else {
        return Err(invalid("expected priority and waiting-time weights"));
    };
    if ![priority, age].iter().all(|v| (0.0..=10.0).contains(v)) {
        return Err(invalid("weights must be in [0,10]"));
    }
    let Scratch {
        trains,
        resources,
        weighted_priority,
        since_of,
        tail_of,
        ready_on,
        events,
        intervals,
        verify,
    } = scratch;
    let n = model.trains.len();
    trains.clear();
    trains.resize_with(n, TrainState::default);
    resources.clear();
    resources.resize_with(model.resources.len(), ResourceState::default);
    weighted_priority.clear();
    weighted_priority.extend(model.trains.iter().map(|t| priority * t.priority));
    // Dense dispatch state: waiting start and tie-break tail per ready train.
    since_of.clear();
    since_of.resize(n, 0);
    tail_of.clear();
    tail_of.resize(n, 0);
    // Prepared static events are merged with completions and clearance wakes.
    let timeline = model.timeline.as_slice();
    let mut cursor = 0;
    events.clear();
    intervals.clear();
    let mut completed = 0;
    // Ready trains grouped by the resource of their next leg, plus the resources
    // that have at least one ready train. Both models are limited to 128 entries.
    ready_on.clear();
    ready_on.resize(model.resources.len(), 0);
    let mut waiting_resources = 0_u128;
    let mut time = 0;
    let release = |mut interval: Interval,
                   time,
                   resources: &mut [ResourceState],
                   intervals: &mut Vec<Interval>,
                   events: &mut BinaryHeap<Reverse<u64>>| {
        interval.exit = time;
        let r = interval.resource;
        resources[r].owner = None;
        resources[r].free_after = time + model.resources[r].clearance;
        if resources[r].free_after > time {
            events.push(Reverse(event_key(resources[r].free_after, WAKE, r)));
        }
        intervals.push(interval);
    };
    // The next event is the smaller of the timeline head and the heap top.
    let next_event = |cursor: usize, events: &BinaryHeap<Reverse<u64>>| {
        let fixed = timeline.get(cursor).copied().unwrap_or(u64::MAX);
        let dynamic = events.peek().map_or(u64::MAX, |e| e.0);
        (fixed.min(dynamic), fixed <= dynamic)
    };
    loop {
        let (first, _) = next_event(cursor, events);
        if first == u64::MAX {
            break;
        }
        let at = first >> 12;
        if at > model.horizon {
            break;
        }
        time = at;
        loop {
            let (key, from_timeline) = next_event(cursor, events);
            if key == u64::MAX || key >> 12 != at {
                break;
            }
            if let Err(e) = work.step() {
                return Err(e.with_metrics(&StopMetrics {
                    closed_intervals: trace.then_some(&intervals[..]),
                    completed,
                    time,
                }));
            }
            if from_timeline {
                cursor += 1;
            } else {
                events.pop();
            }
            let i = (key & 511) as usize;
            match (key >> 9) & 7 {
                COMPLETION => {
                    let state = &mut trains[i];
                    if state.next == model.trains[i].route.len() {
                        release(
                            state.held.take().expect("active train"),
                            time,
                            resources,
                            intervals,
                            events,
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
                CLOSURE_END => resources[model.closures[i].resource].closures -= 1,
                CLOSURE_START => resources[model.closures[i].resource].closures += 1,
                RELEASE => {
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
                return Err(e.with_metrics(&StopMetrics {
                    closed_intervals: trace.then_some(&intervals[..]),
                    completed,
                    time,
                }));
            }
            let state = &mut trains[i];
            if let Some(previous) = state.held.take() {
                release(previous, time, resources, intervals, events);
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
            events.push(Reverse(event_key(end, COMPLETION, i)));
        }
        if completed == trains.len() {
            break;
        }
    }
    if completed != trains.len() {
        let active: Vec<_> = trains.iter().filter_map(|t| t.held.as_ref()).collect();
        let reason = if events.is_empty() && cursor == timeline.len() {
            "deadlock"
        } else {
            "horizon"
        };
        return Err(infeasible(reason).with_metrics(&UnfinishedMetrics {
            active: trace.then_some(active),
            closed_intervals: trace.then_some(&intervals[..]),
            completed,
            time,
        }));
    }
    validate_with(model, intervals, verify).map_err(invalid)?;
    let delay =
        |(s, t): (&TrainState, &Train)| s.arrival.expect("completed").saturating_sub(t.planned);
    // Same per-train terms, added in the same order, as summing a delay vector.
    let score: f64 = trains
        .iter()
        .zip(&model.trains)
        .map(|pair| delay(pair) as f64 * (1.0 + pair.1.priority))
        .sum();
    Ok((
        score,
        work.metrics(|| {
            let delays: Vec<_> = trains.iter().zip(&model.trains).map(delay).collect();
            let metrics = CompleteMetrics {
                completed,
                delays: &delays,
                intervals: trace.then_some(&intervals[..]),
                max_delay: delays.iter().max(),
            };
            crate::encode_metrics(&metrics)
        }),
    ))
}

pub fn validate_trace(model: &Model, intervals: &[Interval]) -> Result<(), String> {
    validate_with(model, intervals, &mut VerifyScratch::default())
}

fn validate_with(
    model: &Model,
    intervals: &[Interval],
    scratch: &mut VerifyScratch,
) -> Result<(), String> {
    const EMPTY: usize = usize::MAX;
    let VerifyScratch {
        slots,
        starts,
        fill,
        by_resource,
    } = scratch;
    let legs = model.leg_offset[model.trains.len()];
    // Movement index per (train, leg) slot, and movement counts per resource.
    slots.clear();
    slots.resize(legs, EMPTY);
    starts.clear();
    starts.resize(model.resources.len() + 1, 0);
    let mut repeated = false;
    for (k, row) in intervals.iter().enumerate() {
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
        let slot = &mut slots[model.leg_offset[row.train] + row.leg];
        repeated |= *slot != EMPTY;
        *slot = k;
        starts[row.resource + 1] += 1;
    }
    // Without a repeated slot, every train has all of its legs exactly when the movement
    // count equals the total leg count. Slots are then in leg order.
    if repeated || intervals.len() != legs {
        return Err("missing, repeated, or disconnected movement".into());
    }
    for range in model.leg_offset.windows(2) {
        let rows = &slots[range[0]..range[1]];
        if rows
            .windows(2)
            .any(|w| intervals[w[0]].exit != intervals[w[1]].entry)
            || rows
                .last()
                .is_none_or(|&r| intervals[r].exit != intervals[r].service_end)
        {
            return Err("missing, repeated, or disconnected movement".into());
        }
    }
    // Stable counting pass by resource, then entry order inside each resource. Rows with
    // equal entries on one resource fail the clearance check in either order, because
    // every exit is after its entry.
    for r in 0..model.resources.len() {
        starts[r + 1] += starts[r];
    }
    fill.clear();
    fill.extend_from_slice(starts);
    by_resource.clear();
    by_resource.resize(intervals.len(), (0, 0));
    for row in intervals {
        by_resource[fill[row.resource]] = (row.entry, row.exit);
        fill[row.resource] += 1;
    }
    for (range, resource) in starts.windows(2).zip(&model.resources) {
        let rows = &mut by_resource[range[0]..range[1]];
        if !rows.is_sorted_by_key(|row| row.0) {
            rows.sort_by_key(|row| row.0);
        }
        if rows
            .windows(2)
            .any(|w| w[0].1 + resource.clearance > w[1].0)
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
                        &json!({"closed_intervals": trace.then_some(&intervals), "completed":completed, "time":time}),
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
                        &json!({"closed_intervals":trace.then_some(&intervals),"completed":completed,"time":time}),
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
                &json!({"completed":completed,"time":time,"closed_intervals":trace.then_some(&intervals),"active":trace.then_some(active)}),
            ));
        }
        reference_validate(model, &intervals).map_err(invalid)?;
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
                crate::encode_metrics(
                    &json!({"completed":completed,"max_delay":delay.iter().max(),"delays":delay,
            "intervals":trace.then_some(intervals)}),
                )
            }),
        ))
    }

    /// The 0.2.0 interval verifier.
    fn reference_validate(model: &Model, intervals: &[Interval]) -> Result<(), String> {
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
        mut kernel: impl FnMut(&Model, &[f64], bool, &mut Work<'_>) -> Evaluation,
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
    fn event_keys_preserve_tuple_order() {
        let ticks = [0, 1, 2, 999_999_999, 1_000_000_000, 2_000_000_000];
        let mut tuples = Vec::new();
        for tick in ticks {
            for kind in [COMPLETION, CLOSURE_END, CLOSURE_START, RELEASE, WAKE] {
                for index in [0, 1, 127, 511] {
                    tuples.push((tick, kind, index));
                }
            }
        }
        for &a in &tuples {
            let key = event_key(a.0, a.1, a.2);
            assert_eq!((key >> 12, (key >> 9) & 7, (key & 511) as usize), a);
            assert!(key < u64::MAX);
            for &b in &tuples {
                assert_eq!(key.cmp(&event_key(b.0, b.1, b.2)), a.cmp(&b));
            }
        }
    }

    #[test]
    fn verifier_matches_the_reference_on_corrupted_traces() {
        let mut verdicts = std::collections::BTreeMap::new();
        let mut shared = VerifyScratch::default();
        for seed in 0..300 {
            let text = random_model(seed).to_string();
            let Ok(model) = prepare(serde_json::from_str(&text).unwrap()) else {
                continue;
            };
            let budget = Budget::default();
            let row = run(
                |m, p, t, w| evaluate(m, p, t, w, &mut Scratch::default()),
                &model,
                &[1.0, 0.5],
                true,
                &budget,
            );
            if row["status"] != "complete" {
                continue;
            }
            let trace: Vec<Interval> =
                serde_json::from_value(row["metrics"]["intervals"].clone()).unwrap();
            let mut rng = Lcg(seed ^ 0x5eed);
            for _ in 0..40 {
                let mut rows = trace.clone();
                for _ in 0..1 + rng.below(3) {
                    if rows.is_empty() {
                        break;
                    }
                    let k = rng.below(rows.len() as u64) as usize;
                    let delta = rng.below(3);
                    match rng.below(11) {
                        0 => rows[k].entry = rows[k].entry.saturating_sub(delta),
                        1 => rows[k].exit += delta,
                        2 => rows[k].service_end = rows[k].service_end.saturating_sub(delta),
                        3 => rows[k].leg = (rows[k].leg + 1) % 4,
                        4 => rows[k].train = rng.below(model.trains.len() as u64 + 1) as usize,
                        5 => rows[k].resource = rng.below(model.resources.len() as u64) as usize,
                        6 => {
                            let copy = rows[k].clone();
                            rows.push(copy);
                        }
                        7 => {
                            rows.remove(k);
                        }
                        8 => {
                            let j = rng.below(rows.len() as u64) as usize;
                            rows.swap(j, k);
                        }
                        9 => {
                            rows[k].entry += delta;
                            rows[k].service_end += delta;
                            rows[k].exit += delta;
                        }
                        _ => {}
                    }
                }
                let expected = reference_validate(&model, &rows);
                assert_eq!(validate_trace(&model, &rows), expected, "seed {seed}");
                assert_eq!(
                    validate_with(&model, &rows, &mut shared),
                    expected,
                    "seed {seed}"
                );
                *verdicts
                    .entry(expected.err().unwrap_or_default())
                    .or_insert(0) += 1;
            }
        }
        assert_eq!(verdicts.len(), 6, "{verdicts:?}");
    }

    #[test]
    fn typed_metrics_keep_sorted_value_bytes() {
        let mut shapes = std::collections::BTreeSet::new();
        let mut scratch = Scratch::default();
        for seed in 0..200 {
            let text = random_model(seed).to_string();
            let Ok(model) = prepare(serde_json::from_str(&text).unwrap()) else {
                continue;
            };
            for trace in [false, true] {
                for max_steps in [1_000_000, 1 + seed % 30] {
                    let budget = Budget {
                        max_steps,
                        ..Budget::default()
                    };
                    let mut work = Work {
                        budget: &budget,
                        deadline: None,
                        used: 0,
                        monitored: false,
                        metrics_enabled: true,
                    };
                    let metrics =
                        match evaluate(&model, &[1.0, 0.25], trace, &mut work, &mut scratch) {
                            Ok((_, metrics)) => metrics,
                            Err(e) => e.metrics,
                        }
                        .expect("metrics enabled");
                    let resorted = serde_json::to_string(
                        &serde_json::from_str::<Value>(metrics.get()).unwrap(),
                    )
                    .unwrap();
                    assert_eq!(metrics.get(), resorted, "seed {seed}");
                    let keys: Vec<_> = serde_json::from_str::<Value>(metrics.get())
                        .unwrap()
                        .as_object()
                        .unwrap()
                        .keys()
                        .cloned()
                        .collect();
                    shapes.insert((keys.join(","), trace));
                }
            }
        }
        assert_eq!(shapes.len(), 6, "{shapes:?}");
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
        // One scratch serves every model and candidate, as a worker's scratch does.
        let mut scratch = Scratch::default();
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
                    let actual = run(
                        |m, p, t, w| evaluate(m, p, t, w, &mut scratch),
                        &model,
                        parameters,
                        trace,
                        &budget,
                    );
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
