//! `keep`: a session started in the background and attached to the terminal it was started in.

use std::os::fd::{FromRawFd, OwnedFd};
use std::process::{Command, ExitStatus, Stdio};

use crate::support::{Scratch, scenario};

const JOB: &str = "7f3e9a21";

/// A fake Claude Code: `--bg` says how to attach, `attach` notes what it finds and can hang up
/// the terminal (as closing the window does) when HANGUP is set, or have another window attach
/// meanwhile when TAKEN_OVER is set; `stop` does nothing.
fn fake_claude(s: &Scratch, says_how_to_attach: bool) {
    let root = s.root.display();
    let bg = if says_how_to_attach {
        format!("echo 'Started in the background.'; echo '  claude attach {JOB} to open it again'")
    } else {
        "echo 'Trust this folder first.' >&2".to_owned()
    };
    s.fake(
        "claude",
        &format!(
            r#"case $1 in
    --bg) {bg} ;;
    attach)
        cat '{root}'/run/slipstream/resume/* >'{root}/logs/resume-during' 2>/dev/null
        cat '{root}/run/agent-notify/claude-'"$2"'.window' >'{root}/logs/window-during' 2>/dev/null
        echo "$PPID" >'{root}/logs/attach-parent'
        if [ -n "${{TAKEN_OVER:-}}" ]; then echo 99999 >'{root}/run/agent-notify/claude-'"$2"'.window'; fi
        if [ -n "${{HANGUP:-}}" ]; then kill -HUP "$PPID"; fi
        sleep 0.2 ;;
esac"#
        ),
    );
}

/// Runs `command` with a terminal for its stdin and stdout, as in a terminal window.
fn in_terminal(mut command: Command) -> ExitStatus {
    let (mut controller, mut terminal) = (-1, -1);
    // SAFETY: openpty fills in two new descriptors, each then owned by one OwnedFd.
    let opened = unsafe {
        libc::openpty(
            &mut controller,
            &mut terminal,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    assert_eq!(opened, 0, "a pseudo-terminal");
    let (controller, terminal) = unsafe {
        (
            OwnedFd::from_raw_fd(controller),
            OwnedFd::from_raw_fd(terminal),
        )
    };
    let status = command
        .stdin(Stdio::from(terminal.try_clone().unwrap()))
        .stdout(Stdio::from(terminal))
        .stderr(Stdio::null())
        .status()
        .unwrap();
    drop(controller);
    status
}

fn keep(s: &Scratch, args: &[&str], terminal: bool) -> ExitStatus {
    let mut all = vec!["keep"];
    all.extend(args);
    let command = s.command(&all);
    if terminal {
        in_terminal(command)
    } else {
        let mut command = command;
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .status()
            .unwrap()
    }
}

fn runs(s: &Scratch) -> Vec<Vec<String>> {
    s.runs("claude")
}

fn strings(runs: &[&[&str]]) -> Vec<Vec<String>> {
    runs.iter()
        .map(|run| run.iter().map(|arg| arg.to_string()).collect())
        .collect()
}

#[test]
fn without_a_terminal_the_agent_runs_as_it_is() {
    scenario("keep_no_terminal", |s| {
        fake_claude(s, true);
        s.fake("gemini", "");
        assert!(keep(s, &["claude", "--model", "opus"], false).success());
        assert!(keep(s, &["gemini", "-y"], true).success());
        assert_eq!(runs(s), strings(&[&["--model", "opus"]]));
        assert_eq!(s.runs("gemini"), strings(&[&["-y"]]));
    });
}

#[test]
fn in_a_terminal_a_session_starts_in_the_background_and_is_attached() {
    scenario("keep_attach", |s| {
        fake_claude(s, true);
        assert!(keep(s, &["claude", "--model", "opus"], true).success());
        assert_eq!(
            runs(s),
            strings(&[
                &["--bg", "--model", "opus"],
                &["attach", JOB],
                &["stop", JOB]
            ])
        );
        let again = format!(
            "'{}' keep claude attach {JOB}\n",
            s.path("bin/agent-notify").display()
        );
        assert_eq!(
            s.read("logs/resume-during"),
            Some(again),
            "the note that attaches the window again"
        );
        assert_eq!(
            s.read("logs/window-during"),
            s.read("logs/attach-parent"),
            "this process is the window"
        );
        assert!(
            s.files().is_empty(),
            "detaching stops it and tidies up: {:?}",
            s.files()
        );
    });
}

#[test]
fn closing_the_window_leaves_a_working_session_running() {
    scenario("keep_hangup_working", |s| {
        fake_claude(s, true);
        s.write(
            &format!("home/.claude/jobs/{JOB}/state.json"),
            r#"{"state":"working"}"#,
        );
        s.set_var("HANGUP", "1");
        assert!(keep(s, &["claude"], true).success());
        assert_eq!(runs(s), strings(&[&["--bg"], &["attach", JOB]]));
        assert!(
            s.read(&format!("run/agent-notify/claude-{JOB}.window"))
                .is_some(),
            "it stays the window"
        );
        assert_eq!(
            s.files()
                .keys()
                .filter(|name| name.contains("resume"))
                .count(),
            1,
            "and reattaches at login"
        );
    });
}

#[test]
fn closing_the_window_stops_a_session_with_nothing_under_way() {
    scenario("keep_hangup_idle", |s| {
        fake_claude(s, true);
        s.write(
            &format!("home/.claude/jobs/{JOB}/state.json"),
            r#"{"state":"idle"}"#,
        );
        s.set_var("HANGUP", "1");
        assert!(keep(s, &["claude"], true).success());
        assert_eq!(
            runs(s),
            strings(&[&["--bg"], &["attach", JOB], &["stop", JOB]])
        );
    });
}

#[test]
fn logging_out_leaves_a_session_waiting_on_an_answer_running() {
    scenario("keep_hangup_blocked", |s| {
        fake_claude(s, true);
        s.write(
            &format!("home/.claude/jobs/{JOB}/state.json"),
            r#"{"state":"blocked"}"#,
        );
        s.set_var("HANGUP", "1");
        assert!(keep(s, &["claude"], true).success());
        assert_eq!(runs(s), strings(&[&["--bg"], &["attach", JOB]]));
    });
}

#[test]
fn a_session_another_window_attached_to_is_left_to_that_window() {
    scenario("keep_taken_over", |s| {
        fake_claude(s, true);
        s.set_var("TAKEN_OVER", "1");
        assert!(keep(s, &["claude"], true).success());
        assert_eq!(
            runs(s),
            strings(&[&["--bg"], &["attach", JOB]]),
            "not stopped under the other window"
        );
        assert_eq!(
            s.files()
                .keys()
                .filter(|name| name.contains("resume"))
                .count(),
            0,
            "this window doesn't come back with it"
        );
        assert_eq!(
            s.read(&format!("run/agent-notify/claude-{JOB}.window"))
                .as_deref(),
            Some("99999\n"),
            "the other window keeps it"
        );
        // Closing this window afterwards doesn't stop it either.
        s.set_var("HANGUP", "1");
        keep(s, &["claude", "attach", JOB], true);
        assert!(!runs(s).contains(&vec!["stop".to_string(), JOB.to_string()]));
    });
}

#[test]
fn what_isn_t_an_interactive_session_runs_as_it_is() {
    scenario("keep_not_a_session", |s| {
        fake_claude(s, true);
        keep(s, &["claude", "mcp", "list"], true);
        keep(s, &["claude", "-p", "count the tests"], true);
        keep(s, &["claude", "--version"], true);
        keep(s, &["claude", "attach", "../elsewhere"], true);
        let expected: &[&[&str]] = &[
            &["mcp", "list"],
            &["-p", "count the tests"],
            &["--version"],
            &["attach", "../elsewhere"],
        ];
        assert_eq!(runs(s), strings(expected));
    });
}

#[test]
fn a_folder_claude_won_t_start_in_the_background_runs_it_as_usual() {
    scenario("keep_untrusted", |s| {
        fake_claude(s, false);
        keep(s, &["claude", "fix the kiln"], true);
        assert_eq!(
            runs(s),
            strings(&[&["--bg", "fix the kiln"], &["fix the kiln"]])
        );
    });
}

#[test]
fn attaching_by_id_reopens_that_session() {
    scenario("keep_attach_id", |s| {
        fake_claude(s, true);
        keep(s, &["claude", "attach", JOB], true);
        assert_eq!(runs(s), strings(&[&["attach", JOB], &["stop", JOB]]));
    });
}
