//! `card`: a line of JSON each time the agents' sessions or the cached limits change, for a
//! desktop's plugin panel.

use std::io::{BufRead, BufReader};
use std::process::Stdio;
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{Value, json};

use crate::support::Scratch;

#[test]
fn the_card_follows_the_working_notes_and_the_usage_cache() {
    let s = Scratch::new("card");
    s.desktop();
    let mut card = s
        .command(&["card"])
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
        json!({"lines": ["No agent session open"], "sections": [], "attention": false})
    );

    // This test's own process stands in for an agent's.
    let pid = std::process::id();
    let note = format!("run/slipstream/working/{pid}");
    s.write(&note, "claude: Kiln & timer\n\nreading the tests\n");
    assert_eq!(
        next(),
        json!({"lines": ["claude: Kiln & timer · 1 subagent"], "sections": [], "attention": false, "glance": "1 working"})
    );
    s.write(&format!("run/agent-notify/{pid}.waiting"), "");
    s.write(&note, "claude: Kiln & timer\nidle\n");
    assert_eq!(
        next(),
        json!({"lines": ["claude: Kiln & timer · waiting on you"], "sections": [], "attention": true, "glance": "1 waiting"})
    );
    std::fs::remove_file(s.path(&format!("run/agent-notify/{pid}.waiting"))).unwrap();
    s.write(&note, "claude: Kiln & timer\nidle\n");
    assert_eq!(
        next(),
        json!({"lines": ["claude: Kiln & timer · idle"], "sections": [], "attention": false}),
        "idle, but still open"
    );
    std::fs::remove_file(s.path(&note)).unwrap();
    assert_eq!(
        next(),
        json!({"lines": ["No agent session open"], "sections": [], "attention": false})
    );
    card.kill().unwrap();
    card.wait().unwrap();
}

/// A desktop runs plugins with a cleared environment and hands them only the folders they may
/// read: the card finds the notes in what it was handed.
#[test]
fn the_card_reads_the_notes_from_the_folders_it_is_handed() {
    let s = Scratch::new("card-plugin");
    s.desktop();
    let pid = std::process::id();
    s.write(
        &format!("run/slipstream/working/{pid}"),
        "claude: kiln\nidle\n",
    );
    let mut command = s.command(&["card"]);
    command
        .env_remove("XDG_RUNTIME_DIR")
        .env("SLIPSTREAM_PLUGIN_READ_0", s.path("cache/agent-notify"))
        .env("SLIPSTREAM_PLUGIN_READ_1", s.path("run/slipstream/working"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped());
    let mut card = command.spawn().unwrap();
    let mut first = String::new();
    BufReader::new(card.stdout.take().unwrap())
        .read_line(&mut first)
        .unwrap();
    card.kill().unwrap();
    card.wait().unwrap();
    let first: Value = serde_json::from_str(&first).unwrap();
    assert_eq!(first["lines"], json!(["claude: kiln · idle"]));
}
