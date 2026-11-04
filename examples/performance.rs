use rail_craft::Prepared;
use serde_json::{Value, json};
use std::{hint::black_box, time::Instant};

fn main() {
    let mut args = std::env::args().skip(1);
    let example = std::fs::read_to_string(args.next().expect("example JSON")).unwrap();
    let example: Value = serde_json::from_str(&example).unwrap();
    let mode = args.next().expect("workload");
    let mut model = example["model"].clone();
    #[allow(unused_mut)]
    let mut parameters = example["parameters"].clone();
    let (count, workers, traced, repeats) = match mode.as_str() {
        "one" => (1, 1, false, 40),
        "serial" => (96, 1, false, 4),
        "parallel" => (96, 4, false, 4),
        "trace" => (12, 1, true, 6),
        "invalid" => (1024, 1, false, 6),
        "domain" => {
            model["horizon"] = json!(2000);
            model["trains"] = json!((0..110).map(|i| json!({"name":format!("T{i}"),"release":i%8,"planned":6,"priority":i%6,"route":[{"resource":2,"duration":3+i%4}]})).collect::<Vec<_>>());
            (48, 1, false, 5)
        }
        "domain2" => {
            model["horizon"] = json!(2000);
            model["trains"] = json!((0..100).map(|i| json!({"name":format!("T{i}"),"release":i%8,"planned":6,"priority":i%6,"route":[{"resource":2,"duration":3}]})).collect::<Vec<_>>());
            model["closures"] = json!(
                (0..300)
                    .map(|i| json!({"resource":2,"start":i*5+1,"end":i*5+2}))
                    .collect::<Vec<_>>()
            );
            (48, 1, false, 5)
        }
        _ => panic!("unknown workload"),
    };
    // Optional override for worker-scaling studies; the named workloads keep their defaults.
    let workers = std::env::var("CRAFT_WORKERS").ok().map_or(workers, |w| {
        w.parse().expect("CRAFT_WORKERS is a worker count")
    });
    let candidates: Vec<_> = (0..count)
        .map(|i| {
            let mut values = parameters.clone();
            if mode == "invalid" {
                values = json!(vec![1.0; 63]);
            } else if let Some(first) = values.as_array_mut().unwrap().first_mut() {
                *first = json!(first.as_f64().unwrap() * (1.0 + (i % 13) as f64 * 0.003));
            }
            json!({"id": format!("candidate-{i:04}"), "parameters": values, "trace": traced})
        })
        .collect();
    let budget = json!({"workers":workers}).to_string();
    let evaluator = Prepared::new(&model.to_string()).unwrap();
    if std::env::args().nth(3).as_deref() == Some("breakdown") {
        breakdown(&mode, &evaluator, &candidates, &budget, traced, repeats);
        return;
    }
    if std::env::var_os("CRAFT_MATRIX").is_some() {
        assert!(!traced, "matrix mode accepts untraced workloads only");
        let rows: Vec<f64> = candidates
            .iter()
            .flat_map(|c| c["parameters"].as_array().unwrap())
            .map(|v| v.as_f64().unwrap())
            .collect();
        let evaluate = || evaluator.score_matrix(count, &rows, &budget).unwrap();
        let first = evaluate();
        if args.next().as_deref() == Some("probe") {
            let records: Vec<_> = first
                .iter()
                .zip(&candidates)
                .map(|(row, c)| {
                    json!({"id": c["id"], "status": row.status, "steps": row.steps,
                           "score": row.score, "metrics": null, "detail": null})
                })
                .collect();
            println!(
                "{}",
                json!({"model_sha256": evaluator.model_sha256(), "records": records,
                       "schema": concat!(env!("CARGO_PKG_NAME"), ".result/0.1"),
                       "version": env!("CARGO_PKG_VERSION")})
            );
            return;
        }
        black_box(first);
        let start = Instant::now();
        for _ in 0..repeats {
            black_box(evaluate());
        }
        println!("{}", start.elapsed().as_secs_f64() / f64::from(repeats));
        return;
    }
    let candidates = serde_json::to_string(&candidates).unwrap();
    let lean = std::env::var_os("CRAFT_SCORES").is_some();
    let evaluate = || {
        if lean {
            evaluator.scores_json(&candidates, &budget)
        } else {
            evaluator.evaluate_json(&candidates, &budget)
        }
    };
    let first = evaluate().unwrap();
    if args.next().as_deref() == Some("probe") {
        println!("{first}");
        return;
    }
    black_box(first);
    let start = Instant::now();
    for _ in 0..repeats {
        black_box(evaluate().unwrap());
    }
    println!("{}", start.elapsed().as_secs_f64() / f64::from(repeats));
}

/// Median seconds per call over 11 timed loops of `loops` calls each.
fn per_call<T>(loops: u32, mut call: impl FnMut() -> T) -> f64 {
    black_box(call());
    let mut samples: Vec<f64> = (0..11)
        .map(|_| {
            let start = Instant::now();
            for _ in 0..loops {
                black_box(call());
            }
            start.elapsed().as_secs_f64() / f64::from(loops)
        })
        .collect();
    samples.sort_by(f64::total_cmp);
    samples[5]
}

/// Time the components of one call: JSON candidate and budget decoding alone, the typed
/// matrix evaluation (no JSON either way), and the lean and full JSON calls.
fn breakdown(
    mode: &str,
    evaluator: &Prepared,
    candidates: &[Value],
    budget: &str,
    traced: bool,
    repeats: u32,
) {
    let text = serde_json::to_string(candidates).unwrap();
    let rows: Vec<f64> = candidates
        .iter()
        .flat_map(|c| c["parameters"].as_array().unwrap())
        .map(|v| v.as_f64().unwrap())
        .collect();
    let loops = repeats * 10;
    let decode = per_call(loops, || {
        (
            serde_json::from_str::<Vec<rail_craft::Candidate>>(&text).unwrap(),
            serde_json::from_str::<rail_craft::Budget>(budget).unwrap(),
        )
    });
    let full = per_call(loops, || evaluator.evaluate_json(&text, budget).unwrap());
    let (lean, matrix) = if traced {
        (None, None)
    } else {
        (
            Some(per_call(loops, || {
                evaluator.scores_json(&text, budget).unwrap()
            })),
            Some(per_call(loops, || {
                evaluator
                    .score_matrix(candidates.len(), &rows, budget)
                    .unwrap()
            })),
        )
    };
    println!(
        "{}",
        json!({"workload": mode, "candidates": candidates.len(), "budget": budget,
               "per_call_seconds": {"decode": decode, "matrix": matrix, "lean": lean, "full": full}})
    );
}
