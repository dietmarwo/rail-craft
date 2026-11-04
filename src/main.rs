use std::io::{Read, stdin};
fn main() {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    let result = match arguments.as_slice() {
        [command] if command == "describe" => Ok(rail_craft::capabilities()),
        [command, model, trace] if command == "verify" => std::fs::read_to_string(model)
            .and_then(|m| std::fs::read_to_string(trace).map(|t| (m, t)))
            .map_err(|e| e.to_string())
            .and_then(|(m, t)| rail_craft::verify_json(&m, &t)),
        [command, path] if command == "run" => {
            let text = if path == "-" {
                let mut input = String::new();
                stdin()
                    .take(16 * 1024 * 1024 + 1)
                    .read_to_string(&mut input)
                    .map(|_| input)
            } else {
                std::fs::File::open(path).and_then(|f| {
                    let mut input = String::new();
                    f.take(16 * 1024 * 1024 + 1)
                        .read_to_string(&mut input)
                        .map(|_| input)
                })
            };
            text.map_err(|e| e.to_string()).and_then(|text| {
                if text.len() > 16 * 1024 * 1024 {
                    Err("request exceeds 16 MiB".into())
                } else {
                    rail_craft::run(&text)
                }
            })
        }
        _ => Err(
            "usage: rail-craft describe | run FILE | verify MODEL TRACE (use - for run stdin)"
                .into(),
        ),
    };
    match result {
        Ok(value) => {
            let complete = value
                .get("records")
                .and_then(serde_json::Value::as_array)
                .is_none_or(|rows| rows.iter().all(|row| row["status"] == "complete"));
            println!("{value}");
            if !complete {
                std::process::exit(2);
            }
        }
        Err(error) => {
            eprintln!(
                "{}",
                serde_json::json!({"status": "rejected", "detail": error})
            );
            std::process::exit(1);
        }
    }
}
