use std::fs;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::time::{self, Duration};
use warp::Filter;

#[tokio::main]
async fn main() {
    let state = Arc::new(Mutex::new(String::from("idle")));

    // Clone state for the background task
    let state_clone = state.clone();
    // Background task to update state from file
    tokio::spawn(async move {
        loop {
            if let Ok(content) = fs::read_to_string("/tmp/orion.state") {
                let mut state = state_clone.lock();
                *state = content.trim().to_string();
            }
            time::sleep(Duration::from_secs(1)).await;
        }
    });

    // GET /state endpoint
    let state_clone = state.clone();
    let state_route = warp::path!("state")
        .map(move || {
            let state = state_clone.lock();
            // Fix: properly dereference the MutexGuard
            let state_str = state.clone(); // Clone the String inside the MutexGuard
            format!("{{\"state\": \"{}\"}}", state_str)
        });

    // GET / endpoint - serve HTML
    let html_route = warp::path::end()
        .map(|| {
            warp::reply::html(include_str!("../gui.html"))
        });

    // POST /command endpoint
    let cmd_route = warp::path!("command")
        .and(warp::post())
        .and(warp::body::form())
        .map(|mut form: warp::form::Form| {
            let command = form.remove("command").unwrap_or_default();
            let output = if command.trim().is_empty() {
                "Please enter a command".to_string()
            } else {
                // Run the orion command
                let output = std::process::Command::new("orion")
                    .arg(&command)
                    .output()
                    .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
                    .unwrap_or_else(|e| format!("Error: {}", e));
                format!("{{\"output\": \"{}\"}}", output.replace('"', "\\\""))
            };
            warp::reply::json(&serde_json::json!({ "output": output }))
        });

    let routes = state_route.or(html_route).or(cmd_route);

    warp::serve(routes)
        .run(([127, 0, 0, 1], 3030))
        .await;
}