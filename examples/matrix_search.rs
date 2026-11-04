//! Grid search over the two dispatch weights with the typed score-matrix API.
//!
//! `cargo run --release --locked --example matrix_search -- models/example.json`
use rail_craft::Prepared;
use serde_json::Value;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "models/example.json".into());
    let example: Value = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    let prepared = Prepared::new(&example["model"].to_string())?;
    // 21 x 21 policies in [0, 10]^2, stored row-major: [priority, age] per row.
    let steps = 21;
    let rows: Vec<f64> = (0..steps * steps)
        .flat_map(|k| [(k / steps) as f64 * 0.5, (k % steps) as f64 * 0.5])
        .collect();
    let scores = prepared.score_matrix(steps * steps, &rows, r#"{"workers":2}"#)?;
    let complete = scores.iter().filter(|s| s.status == "complete").count();
    let (best, score) = scores
        .iter()
        .enumerate()
        .filter_map(|(i, s)| s.score.map(|v| (i, v)))
        .min_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)))
        .ok_or("no complete policy")?;
    let worst = scores
        .iter()
        .filter_map(|s| s.score)
        .fold(f64::NEG_INFINITY, f64::max);
    println!(
        "{complete} of {} policies complete; best weighted delay {score} at [{}, {}] \
         ({} steps); worst {worst}",
        scores.len(),
        rows[2 * best],
        rows[2 * best + 1],
        scores[best].steps
    );
    Ok(())
}
