mod domain;
pub use domain::verify_json;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json, value::RawValue};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{
        Mutex, OnceLock, PoisonError,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

/// A score with optional pre-encoded metrics, or a classified failure.
pub type Evaluation = Result<(f64, Option<Box<RawValue>>), Failure>;

#[derive(Debug, Clone)]
pub struct Failure {
    status: &'static str,
    detail: String,
    metrics: Option<Box<RawValue>>,
}

/// Encode metrics once; the raw JSON is spliced into records without a `Value` tree.
pub fn encode_metrics(metrics: &impl Serialize) -> Box<RawValue> {
    serde_json::value::to_raw_value(metrics).expect("metrics encode as JSON")
}
pub fn invalid(detail: impl Into<String>) -> Failure {
    Failure {
        status: "invalid",
        detail: detail.into(),
        metrics: None,
    }
}
pub fn infeasible(detail: impl Into<String>) -> Failure {
    Failure {
        status: "infeasible",
        detail: detail.into(),
        metrics: None,
    }
}

impl Failure {
    pub fn with_metrics(mut self, metrics: &impl Serialize) -> Self {
        self.metrics = Some(encode_metrics(metrics));
        self
    }
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    pub id: String,
    pub parameters: Vec<f64>,
    #[serde(default)]
    pub trace: bool,
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Budget {
    workers: usize,
    max_steps: u64,
    wall_ms: Option<u64>,
    cancel_file: Option<PathBuf>,
}
impl Default for Budget {
    fn default() -> Self {
        Self {
            workers: 1,
            max_steps: 1_000_000,
            wall_ms: None,
            cancel_file: None,
        }
    }
}
impl Budget {
    fn limits_valid(&self, count: usize) -> bool {
        (1..=64).contains(&self.workers)
            && (1..=100_000_000).contains(&self.max_steps)
            && !self.wall_ms.is_some_and(|n| n == 0 || n > 86_400_000)
            && (1..=4096).contains(&count)
    }

    fn deadline(&self) -> Option<Instant> {
        self.wall_ms
            .map(|n| Instant::now() + Duration::from_millis(n))
    }
}

/// One untraced candidate's outcome from `Prepared::score_matrix`.
#[derive(Clone, Debug, PartialEq)]
pub struct MatrixScore {
    /// Same status strings as the JSON records.
    pub status: &'static str,
    pub steps: u64,
    /// `Some` only for `"complete"`.
    pub score: Option<f64>,
}

/// Reusable domain buffers shared by the calls of one `Prepared` evaluator.
#[derive(Default)]
struct ScratchPool(Mutex<Vec<domain::Scratch>>);

impl ScratchPool {
    /// Most scratch sets retained between calls; more concurrent users allocate their own.
    const LIMIT: usize = 64;

    fn take(&self) -> domain::Scratch {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop()
            .unwrap_or_default()
    }

    fn give(&self, scratch: domain::Scratch) {
        let mut pool = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if pool.len() < Self::LIMIT {
            pool.push(scratch);
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).len()
    }
}

/// Evaluate `count` independent items inline or on scoped workers, in input order.
/// The inline caller and each worker reuse one pooled domain scratch for all items.
fn dispatch<T: Send + Sync>(
    pool: &ScratchPool,
    count: usize,
    workers: usize,
    evaluate: impl Fn(&mut domain::Scratch, usize) -> T + Sync,
) -> Vec<T> {
    if workers == 1 || count == 1 {
        let mut scratch = pool.take();
        let rows = (0..count).map(|i| evaluate(&mut scratch, i)).collect();
        pool.give(scratch);
        return rows;
    }
    let next = AtomicUsize::new(0);
    let slots: Vec<_> = (0..count).map(|_| OnceLock::new()).collect();
    let work = || {
        let mut scratch = pool.take();
        loop {
            let index = next.fetch_add(1, Ordering::Relaxed);
            if index >= count {
                break;
            }
            assert!(
                slots[index].set(evaluate(&mut scratch, index)).is_ok(),
                "duplicate work index"
            );
        }
        pool.give(scratch);
    };
    // The calling thread is one of the `min(workers, count)` concurrent evaluators.
    std::thread::scope(|scope| {
        for _ in 1..workers.min(count) {
            scope.spawn(work);
        }
        work();
    });
    slots
        .into_iter()
        .map(|slot| slot.into_inner().expect("missing evaluation"))
        .collect()
}

pub struct Work<'a> {
    budget: &'a Budget,
    deadline: Option<Instant>,
    used: u64,
    monitored: bool,
    metrics_enabled: bool,
}
impl Work<'_> {
    /// Build metrics only when the caller requested full records.
    pub fn metrics(&self, make: impl FnOnce() -> Box<RawValue>) -> Option<Box<RawValue>> {
        self.metrics_enabled.then(make)
    }

    #[inline]
    pub fn step(&mut self) -> Result<(), Failure> {
        if self.used < self.budget.max_steps && !self.monitored {
            self.used += 1;
            return Ok(());
        }
        self.checked_step()
    }

    #[cold]
    fn checked_step(&mut self) -> Result<(), Failure> {
        if self.budget.cancel_file.as_ref().is_some_and(|p| p.exists()) {
            return Err(Failure {
                status: "cancelled",
                detail: "cancel file exists".into(),
                metrics: None,
            });
        }
        if self.deadline.is_some_and(|d| Instant::now() >= d) {
            return Err(Failure {
                status: "wall_limit",
                detail: "batch deadline reached".into(),
                metrics: None,
            });
        }
        if self.used == self.budget.max_steps {
            return Err(Failure {
                status: "work_limit",
                detail: "candidate step budget reached".into(),
                metrics: None,
            });
        }
        self.used += 1;
        Ok(())
    }
}

#[derive(Serialize)]
struct Record<'a> {
    id: &'a str,
    status: &'static str,
    steps: u64,
    score: Option<f64>,
    metrics: Option<Box<RawValue>>,
    detail: Option<String>,
}

#[derive(Serialize)]
struct Batch<'a> {
    model_sha256: &'a str,
    records: Vec<Record<'a>>,
    schema: &'static str,
    version: &'static str,
}

pub struct Prepared {
    model: domain::Model,
    model_sha256: String,
    scratch: ScratchPool,
}
impl Prepared {
    pub fn new(model: &str) -> Result<Self, String> {
        let model_sha256 = format!("{:x}", Sha256::digest(model.as_bytes()));
        let model = domain::prepare(serde_json::from_str(model).map_err(|e| e.to_string())?)?;
        Ok(Self {
            model,
            model_sha256,
            scratch: ScratchPool::default(),
        })
    }

    pub fn evaluate_json(&self, candidates: &str, budget: &str) -> Result<String, String> {
        let candidates: Vec<Candidate> =
            serde_json::from_str(candidates).map_err(|e| e.to_string())?;
        let budget = serde_json::from_str(budget).map_err(|e| e.to_string())?;
        self.evaluate_encoded(candidates, budget, true)
    }

    /// Evaluate untraced candidates with full feasibility checks and null metrics.
    pub fn scores_json(&self, candidates: &str, budget: &str) -> Result<String, String> {
        let candidates: Vec<Candidate> =
            serde_json::from_str(candidates).map_err(|e| e.to_string())?;
        let budget = serde_json::from_str(budget).map_err(|e| e.to_string())?;
        if candidates.iter().any(|c| c.trace) {
            return Err("scores mode accepts untraced candidates only".into());
        }
        self.evaluate_encoded(candidates, budget, false)
    }

    fn evaluate_encoded(
        &self,
        candidates: Vec<Candidate>,
        budget: Budget,
        metrics_enabled: bool,
    ) -> Result<String, String> {
        serde_json::to_string(&self.evaluate(&candidates, budget, metrics_enabled)?)
            .map_err(|e| e.to_string())
    }

    /// SHA-256 of the exact model bytes passed to `Prepared::new`.
    pub fn model_sha256(&self) -> &str {
        &self.model_sha256
    }

    /// Evaluate `rows` untraced candidates whose parameters are stored row-major in
    /// `parameters`. Each row has `parameters.len() / rows` values; dimension 0 is
    /// allowed. Budget JSON, limits, worker dispatch, deadlines, cancellation and
    /// failure classification are the same as `scores_json`.
    pub fn score_matrix(
        &self,
        rows: usize,
        parameters: &[f64],
        budget: &str,
    ) -> Result<Vec<MatrixScore>, String> {
        if !(1..=4096).contains(&rows) {
            return Err("matrix rows must be between 1 and 4096".into());
        }
        if !parameters.len().is_multiple_of(rows) {
            return Err("parameter matrix length is not a multiple of rows".into());
        }
        let dimension = parameters.len() / rows;
        if dimension > 64 {
            return Err("parameter vector exceeds limit".into());
        }
        let budget: Budget = serde_json::from_str(budget).map_err(|e| e.to_string())?;
        if !budget.limits_valid(rows) {
            return Err("invalid budget or batch size".into());
        }
        let deadline = budget.deadline();
        Ok(dispatch(
            &self.scratch,
            rows,
            budget.workers,
            |scratch, row| {
                let values = &parameters[row * dimension..(row + 1) * dimension];
                let (result, steps) =
                    self.outcome(scratch, values, false, &budget, deadline, false);
                match result {
                    Ok((score, _)) => MatrixScore {
                        status: "complete",
                        steps,
                        score: Some(score),
                    },
                    Err(e) => MatrixScore {
                        status: e.status,
                        steps,
                        score: None,
                    },
                }
            },
        ))
    }

    fn outcome(
        &self,
        scratch: &mut domain::Scratch,
        parameters: &[f64],
        trace: bool,
        budget: &Budget,
        deadline: Option<Instant>,
        metrics_enabled: bool,
    ) -> (Evaluation, u64) {
        let mut work = Work {
            budget,
            deadline,
            used: 0,
            metrics_enabled,
            monitored: deadline.is_some() || budget.cancel_file.is_some(),
        };
        let result = domain::evaluate(&self.model, parameters, trace, &mut work, scratch);
        let result = result.and_then(|(score, metrics)| {
            if score.is_finite() {
                Ok((score, metrics))
            } else {
                Err(invalid("nonfinite objective"))
            }
        });
        (result, work.used)
    }

    fn record<'a>(
        &self,
        scratch: &mut domain::Scratch,
        c: &'a Candidate,
        budget: &Budget,
        deadline: Option<Instant>,
        metrics_enabled: bool,
    ) -> Record<'a> {
        let (result, steps) = self.outcome(
            scratch,
            &c.parameters,
            c.trace,
            budget,
            deadline,
            metrics_enabled,
        );
        match result {
            Ok((score, metrics)) => Record {
                id: &c.id,
                status: "complete",
                steps,
                score: Some(score),
                metrics,
                detail: None,
            },
            Err(e) => Record {
                id: &c.id,
                status: e.status,
                steps,
                score: None,
                metrics: if metrics_enabled { e.metrics } else { None },
                detail: Some(e.detail),
            },
        }
    }

    fn evaluate<'a>(
        &'a self,
        candidates: &'a [Candidate],
        budget: Budget,
        metrics_enabled: bool,
    ) -> Result<Batch<'a>, String> {
        if !budget.limits_valid(candidates.len()) {
            return Err("invalid budget or batch size".into());
        }
        let mut seen = HashSet::with_capacity(candidates.len());
        for c in candidates {
            if c.id.is_empty() || c.id.len() > 128 || !seen.insert(&c.id) || c.parameters.len() > 64
            {
                return Err(
                    "invalid or duplicate candidate id, or parameter vector exceeds limit".into(),
                );
            }
        }
        if candidates.iter().filter(|c| c.trace).count() > 16 {
            return Err("at most 16 traced candidates per batch".into());
        }
        let deadline = budget.deadline();
        let records = dispatch(
            &self.scratch,
            candidates.len(),
            budget.workers,
            |scratch, index| {
                self.record(
                    scratch,
                    &candidates[index],
                    &budget,
                    deadline,
                    metrics_enabled,
                )
            },
        );
        Ok(Batch {
            model_sha256: &self.model_sha256,
            records,
            schema: concat!(env!("CARGO_PKG_NAME"), ".result/0.1"),
            version: env!("CARGO_PKG_VERSION"),
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    schema: String,
    model: Box<RawValue>,
    candidates: Vec<Candidate>,
    #[serde(default)]
    budget: Budget,
}

pub fn run(text: &str) -> Result<Value, String> {
    let req: Request = serde_json::from_str(text).map_err(|e| e.to_string())?;
    if req.schema != concat!(env!("CARGO_PKG_NAME"), ".request/0.1") {
        return Err("unsupported request schema".into());
    }
    let prepared = Prepared::new(req.model.get())?;
    serde_json::to_value(prepared.evaluate(&req.candidates, req.budget, true)?)
        .map_err(|e| e.to_string())
}

pub fn capabilities() -> Value {
    json!({"schema": concat!(env!("CARGO_PKG_NAME"), ".capabilities/0.1"),
           "version": env!("CARGO_PKG_VERSION"), "models": domain::CAPABILITIES,
           "batch": true, "worker_limit": 64})
}

#[cfg(feature = "python")]
mod python {
    use super::*;
    use pyo3::{exceptions::PyValueError, prelude::*};

    #[pyclass(name = "Prepared")]
    struct PyPrepared(Prepared);

    #[pymethods]
    impl PyPrepared {
        #[new]
        fn new(model: &str) -> PyResult<Self> {
            Prepared::new(model)
                .map(Self)
                .map_err(PyValueError::new_err)
        }
        fn scores(&self, py: Python<'_>, candidates: String, budget: String) -> PyResult<String> {
            py.detach(|| self.0.scores_json(&candidates, &budget))
                .map_err(PyValueError::new_err)
        }
        fn batch(&self, py: Python<'_>, candidates: String, budget: String) -> PyResult<String> {
            py.detach(|| self.0.evaluate_json(&candidates, &budget))
                .map_err(PyValueError::new_err)
        }
        /// Score untraced rows; NaN marks every row that is not complete.
        fn score_matrix(
            &self,
            py: Python<'_>,
            rows: Vec<Vec<f64>>,
            budget: String,
        ) -> PyResult<(Vec<f64>, Vec<&'static str>, Vec<u64>)> {
            let dimension = rows.first().map_or(0, Vec::len);
            if rows.iter().any(|row| row.len() != dimension) {
                return Err(PyValueError::new_err("ragged parameter rows"));
            }
            let count = rows.len();
            let flat = rows.concat();
            let results = py
                .detach(|| self.0.score_matrix(count, &flat, &budget))
                .map_err(PyValueError::new_err)?;
            let mut scores = Vec::with_capacity(count);
            let mut statuses = Vec::with_capacity(count);
            let mut steps = Vec::with_capacity(count);
            for result in results {
                scores.push(result.score.unwrap_or(f64::NAN));
                statuses.push(result.status);
                steps.push(result.steps);
            }
            Ok((scores, statuses, steps))
        }
    }

    #[pymodule]
    fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
        m.add_class::<PyPrepared>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Value {
        serde_json::from_str(include_str!("../models/example.json")).unwrap()
    }
    #[test]
    fn prepared_workers_return_same_completed_records() {
        let example = fixture();
        let prepared = Prepared::new(&example["model"].to_string()).unwrap();
        let candidates = json!([
            {"id":"first","parameters":example["parameters"]},
            {"id":"second","parameters":example["parameters"]},
        ]);
        let serial = prepared
            .evaluate_json(&candidates.to_string(), r#"{"workers":1}"#)
            .unwrap();
        let parallel = prepared
            .evaluate_json(&candidates.to_string(), r#"{"workers":4}"#)
            .unwrap();
        assert_eq!(serial, parallel);
        assert!(serial.contains(r#""status":"complete""#));
    }
    #[test]
    fn scores_keep_order_status_steps_and_objective() {
        let example = fixture();
        let prepared = Prepared::new(&example["model"].to_string()).unwrap();
        let mut candidates = json!([
            {"id":"valid","parameters":example["parameters"]},
            {"id":"invalid","parameters": vec![1.0; 63]},
        ]);
        for budget in ["{}", r#"{"workers":4}"#, r#"{"max_steps":1}"#] {
            let mut full: Value = serde_json::from_str(
                &prepared
                    .evaluate_json(&candidates.to_string(), budget)
                    .unwrap(),
            )
            .unwrap();
            for row in full["records"].as_array_mut().unwrap() {
                row["metrics"] = Value::Null;
            }
            let scores: Value = serde_json::from_str(
                &prepared
                    .scores_json(&candidates.to_string(), budget)
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(full, scores);
        }
        candidates[0]["trace"] = json!(true);
        assert!(prepared.scores_json(&candidates.to_string(), "{}").is_err());
    }

    fn assert_matrix_matches_scores(prepared: &Prepared, rows: &[Vec<f64>], budget: &str) {
        let candidates: Vec<_> = rows
            .iter()
            .enumerate()
            .map(|(i, p)| json!({"id": format!("row-{i}"), "parameters": p}))
            .collect();
        let lean: Value = serde_json::from_str(
            &prepared
                .scores_json(&Value::from(candidates).to_string(), budget)
                .unwrap(),
        )
        .unwrap();
        let flat: Vec<f64> = rows.concat();
        let matrix = prepared.score_matrix(rows.len(), &flat, budget).unwrap();
        let records = lean["records"].as_array().unwrap();
        assert_eq!(records.len(), matrix.len());
        for (record, row) in records.iter().zip(&matrix) {
            assert_eq!(record["status"], row.status, "{budget}");
            assert_eq!(record["steps"].as_u64(), Some(row.steps), "{budget}");
            assert_eq!(
                record["score"].as_f64().map(f64::to_bits),
                row.score.map(f64::to_bits),
                "{budget}"
            );
            assert_eq!(row.score.is_some(), row.status == "complete");
        }
    }

    #[test]
    fn score_matrix_matches_scores_json() {
        let example = fixture();
        let prepared = Prepared::new(&example["model"].to_string()).unwrap();
        let rows = vec![
            vec![1.0, 0.0],
            vec![0.0, 0.0],
            vec![2.5, 1.25],
            vec![11.0, 0.0],
            vec![1.0, -0.5],
            vec![0.917_866_537_695_451_6, 0.004_332_725_510_795_259],
        ];
        let cancel =
            std::env::temp_dir().join(format!("rail-matrix-cancel-{}", std::process::id()));
        std::fs::write(&cancel, b"").unwrap();
        let cancel_budget = json!({"cancel_file": cancel}).to_string();
        for budget in [
            "{}",
            r#"{"workers":4}"#,
            r#"{"workers":3,"max_steps":7}"#,
            r#"{"max_steps":1}"#,
            r#"{"max_steps":21}"#,
            r#"{"max_steps":20}"#,
            r#"{"wall_ms":60000}"#,
            cancel_budget.as_str(),
        ] {
            assert_matrix_matches_scores(&prepared, &rows, budget);
        }
        let cancelled = prepared
            .score_matrix(1, &[1.0, 0.0], &cancel_budget)
            .unwrap();
        assert_eq!(cancelled[0].status, "cancelled");
        std::fs::remove_file(&cancel).unwrap();
        let limited = prepared
            .score_matrix(1, &[1.0, 0.0], r#"{"max_steps":3}"#)
            .unwrap();
        assert_eq!((limited[0].status, limited[0].steps), ("work_limit", 3));
        // Wrong dimensions are invalid rows, not batch errors.
        assert_matrix_matches_scores(&prepared, &[vec![], vec![]], "{}");
        assert_matrix_matches_scores(&prepared, &vec![vec![1.0, 0.0, 0.0]; 3], r#"{"workers":2}"#);
        // A horizon failure is classified as infeasible.
        let mut short = example["model"].clone();
        short["horizon"] = json!(12);
        for train in short["trains"].as_array_mut().unwrap() {
            train["planned"] = json!(5);
        }
        let short = Prepared::new(&short.to_string()).unwrap();
        assert_matrix_matches_scores(&short, &rows, "{}");
        assert_eq!(
            short.score_matrix(1, &[1.0, 0.0], "{}").unwrap()[0].status,
            "infeasible"
        );
    }

    #[test]
    fn score_matrix_rejects_invalid_shapes_and_budgets() {
        let example = fixture();
        let prepared = Prepared::new(&example["model"].to_string()).unwrap();
        assert!(prepared.score_matrix(0, &[], "{}").is_err());
        assert!(prepared.score_matrix(4097, &[], "{}").is_err());
        assert!(prepared.score_matrix(4096, &[], "{}").is_ok());
        assert!(prepared.score_matrix(2, &[1.0, 0.0, 1.0], "{}").is_err());
        assert!(prepared.score_matrix(1, &[0.0; 65], "{}").is_err());
        assert!(prepared.score_matrix(1, &[0.0; 64], "{}").is_ok());
        for budget in [
            "",
            "null",
            r#"{"workers":0}"#,
            r#"{"workers":65}"#,
            r#"{"max_steps":0}"#,
            r#"{"wall_ms":0}"#,
            r#"{"unknown":1}"#,
        ] {
            assert!(
                prepared.score_matrix(1, &[1.0, 0.0], budget).is_err(),
                "{budget}"
            );
        }
    }

    #[test]
    fn pooled_scratch_matches_fresh_evaluators() {
        let example = fixture();
        let model = example["model"].to_string();
        let mut short = example["model"].clone();
        short["horizon"] = json!(12);
        for train in short["trains"].as_array_mut().unwrap() {
            train["planned"] = json!(5);
        }
        let short = short.to_string();
        let shared = Prepared::new(&model).unwrap();
        let shared_short = Prepared::new(&short).unwrap();
        let batches: [(&[f64], &str); 6] = [
            (&[1.0, 0.0, 0.0, 0.0, 2.0, 1.5], r#"{"workers":3}"#),
            (&[0.5, 0.25], "{}"),
            (&[11.0, 0.0, 1.0, 1.0], r#"{"max_steps":9}"#),
            (
                &[2.0, 2.0, 3.0, 0.1, 0.2, 0.3, 4.0, 5.0],
                r#"{"workers":2}"#,
            ),
            (&[1.0, 0.0], r#"{"max_steps":21}"#),
            (&[0.0, 7.0, 9.0, 0.0], "{}"),
        ];
        for round in 0..3 {
            for (parameters, budget) in batches {
                let rows = parameters.len() / 2;
                for (text, prepared) in [(&model, &shared), (&short, &shared_short)] {
                    let fresh = Prepared::new(text).unwrap();
                    assert_eq!(
                        prepared.score_matrix(rows, parameters, budget).unwrap(),
                        fresh.score_matrix(rows, parameters, budget).unwrap(),
                        "round {round} {budget}"
                    );
                    let candidates: Vec<_> = parameters
                        .chunks(2)
                        .enumerate()
                        .map(
                            |(i, p)| json!({"id": i.to_string(), "parameters": p, "trace": i == 0}),
                        )
                        .collect();
                    let candidates = Value::from(candidates).to_string();
                    assert_eq!(
                        prepared.evaluate_json(&candidates, budget).unwrap(),
                        fresh.evaluate_json(&candidates, budget).unwrap()
                    );
                }
            }
        }
        assert!((1..=ScratchPool::LIMIT).contains(&shared.scratch.len()));
        let pool = ScratchPool::default();
        for _ in 0..ScratchPool::LIMIT + 3 {
            pool.give(domain::Scratch::default());
        }
        assert_eq!(pool.len(), ScratchPool::LIMIT);
    }

    #[test]
    fn dispatch_keeps_order_and_bounds_concurrent_evaluators() {
        let pool = ScratchPool::default();
        for (count, workers) in [(1, 4), (2, 4), (5, 2), (97, 4), (64, 64), (3, 1)] {
            let calls = AtomicUsize::new(0);
            let threads = Mutex::new(HashSet::new());
            let rows = dispatch(&pool, count, workers, |_, index| {
                calls.fetch_add(1, Ordering::Relaxed);
                threads.lock().unwrap().insert(std::thread::current().id());
                index * 3
            });
            assert_eq!(rows, (0..count).map(|i| i * 3).collect::<Vec<_>>());
            assert_eq!(calls.load(Ordering::Relaxed), count);
            let used = threads.lock().unwrap().len();
            assert!(
                (1..=workers.min(count)).contains(&used),
                "{count} {workers} {used}"
            );
        }
    }

    #[test]
    fn model_and_candidate_validation_are_independent() {
        let mut example = fixture();
        example["model"]["unexpected"] = json!(true);
        assert!(Prepared::new(&example["model"].to_string()).is_err());
        let example = fixture();
        let prepared = Prepared::new(&example["model"].to_string()).unwrap();
        let duplicate = json!([
            {"id":"a","parameters":example["parameters"]},
            {"id":"a","parameters":example["parameters"]},
        ]);
        assert!(
            prepared
                .evaluate_json(&duplicate.to_string(), "{}")
                .is_err()
        );
    }
}
