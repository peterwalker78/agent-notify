//! `card`: a line of JSON each time the cached limits change, for a desktop's plugin panel.

use std::io::{BufRead, BufReader};
use std::process::Stdio;
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{Value, json};

use crate::support::Scratch;

#[test]
fn the_card_follows_the_usage_cache() {
    let s = Scratch::new("card");
    s.desktop();
    let mut card = s
        .command(&["card"])
        .env_remove("XDG_RUNTIME_DIR")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let (sender, lines) = mpsc::channel();
    let stdout = card.stdout.take().unwrap();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if sender
                .send(serde_json::from_str::<Value>(&line).unwrap())
                .is_err()
            {
                break;
            }
        }
    });
    let next = || {
        lines
            .recv_timeout(Duration::from_secs(5))
            .expect("a line from the card")
    };
    assert_eq!(
        next(),
        json!({"lines": ["No plan limits yet: they come once an agent has run"], "sections": []})
    );
    // A session's note changes nothing: the card lists no sessions.
    s.write(
        &format!("run/slipstream/working/{}", std::process::id()),
        "claude: kiln\n",
    );
    assert!(lines.recv_timeout(Duration::from_millis(500)).is_err());
    card.kill().unwrap();
    card.wait().unwrap();
}
