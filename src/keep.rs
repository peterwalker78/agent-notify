//! `agent-notify keep AGENT [ARGS]`: starts a session in the background and attaches this
//! terminal to it, so it carries on when the window closes or you log out.
//!
//! The terminal is outside the session's process tree, so this process stays in the window,
//! under its shell, while it is attached: the hooks name it with the sender-pid hint as the
//! session's window, and it leaves the desktop a note that attaches the window to the session
//! again at the next login (the layout record looks for notes under the window's shell, which
//! the session isn't). Only Claude Code runs a background session; any other agent, and anything
//! that isn't an interactive session (print mode, a subcommand, no terminal), runs as it is. An
//! agent reopening one of its sessions leaves its note first, so it comes back again next time.
//! Codex runs its sessions in a shared background server unless told otherwise; started here, a
//! session runs in its own process in the window instead, as it used to.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::process::CommandExt;
use std::process::{ExitCode, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::agents::{self, claude};
use crate::env::Env;
use crate::state;
use crate::text::shell_quote;

pub fn run(env: &Env, agent: &OsStr, args: &[OsString]) -> ExitCode {
    let interactive = is_terminal(libc::STDIN_FILENO) && is_terminal(libc::STDOUT_FILENO);
    if agent != "claude" || !interactive {
        if interactive {
            reopen_note(env, agent, args);
            if agent == "codex" && runs_shared(env, args) {
                return exec(env, agent, &on_its_own(args));
            }
        }
        return exec(env, agent, args);
    }
    // The ID goes into the note run again at the next login, so only one shaped like an ID is
    // kept there; anything else is claude's to answer.
    if let [attach, id] = args {
        if let Some(id) = id
            .to_str()
            .filter(|id| attach == "attach" && claude::is_id(id))
        {
            attach_to(env, "claude", id);
        }
    }
    let first = args.first().and_then(|arg| arg.to_str());
    if first.is_some_and(|first| claude::COMMANDS.contains(&first))
        || args.iter().any(|arg| {
            arg.to_str()
                .is_some_and(|arg| claude::NOT_A_SESSION.contains(&arg))
        })
    {
        return exec(env, agent, args);
    }
    // Claude won't start a background session in a folder it hasn't been trusted in yet: then
    // it runs as usual, and asks.
    let started = env
        .command("claude")
        .arg("--bg")
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    let printed = started.map(|output| String::from_utf8_lossy(&output.stdout).into_owned());
    match printed.as_deref().ok().and_then(claude::attach_id) {
        Some(id) => attach_to(env, "claude", id),
        None => exec(env, agent, args),
    }
}

/// The note that reopens session `id` with `command` (the agent's own, such as `codex resume`),
/// through `keep`.
pub fn note(env: &Env, command: &str, id: &str) -> String {
    format!(
        "{} keep {command} {id}",
        shell_quote(&env.self_path().to_string_lossy())
    )
}

/// An agent that is reopened rather than kept running, being reopened: the note that brought it
/// back is left again under this process, which is about to become the agent, so the session
/// comes back at the next login as well, whether or not anything is typed into it first.
fn reopen_note(env: &Env, agent: &OsStr, args: &[OsString]) {
    let Some(agent) = agent.to_str() else { return };
    let Some(command) = agents::resume_command(agent) else {
        return;
    };
    let Some(id) = resumed(command, agent, args) else {
        return;
    };
    let Some(dir) = env
        .resume_dir()
        .filter(|dir| fs::create_dir_all(dir).is_ok())
    else {
        return;
    };
    let _ = state::write_line(
        &dir.join(std::process::id().to_string()),
        note(env, command, id),
    );
}

/// The session ID in `agent args`, when they are exactly `command` followed by one.
fn resumed<'a>(command: &str, agent: &str, args: &'a [OsString]) -> Option<&'a str> {
    let mut words = command.split(' ');
    if words.next() != Some(agent) {
        return None;
    }
    let words: Vec<&str> = words.collect();
    let (id, before) = args.split_last()?;
    let id = id.to_str().filter(|id| crate::hook::resumable_id(id))?;
    (before.len() == words.len() && before.iter().zip(&words).all(|(arg, word)| arg == word))
        .then_some(id)
}

/// Whether `codex ARGS` would run its session in Codex's shared background server. That server
/// runs every window's session, and their hooks, in one process outside any terminal, so a
/// session there leaves its note where no window's shell is, and its work is counted to no
/// window. Only a session being started or reopened is asked, and only when this Codex knows
/// `--no-daemon`.
fn runs_shared(env: &Env, args: &[OsString]) -> bool {
    let starts_a_session = match args.first().and_then(|arg| arg.to_str()) {
        None => true,
        Some(first) => first.starts_with('-') || first == "resume" || first == "fork",
    };
    if !starts_a_session || args.iter().any(|arg| arg == NO_DAEMON) {
        return false;
    }
    env.command("codex")
        .arg("--help")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .is_ok_and(|help| String::from_utf8_lossy(&help.stdout).contains(NO_DAEMON))
}

const NO_DAEMON: &str = "--no-daemon";

/// `args` with Codex told to run the session in its own process, after `resume` or `fork` when
/// it reopens one.
fn on_its_own(args: &[OsString]) -> Vec<OsString> {
    let at = usize::from(
        args.first()
            .is_some_and(|arg| arg == "resume" || arg == "fork"),
    );
    let mut args = args.to_vec();
    args.insert(at, NO_DAEMON.into());
    args
}

fn is_terminal(fd: libc::c_int) -> bool {
    // SAFETY: isatty only inspects the descriptor.
    unsafe { libc::isatty(fd) == 1 }
}

/// Runs the agent in this process's place.
fn exec(env: &Env, agent: &OsStr, args: &[OsString]) -> ExitCode {
    let err = env.command(agent).args(args).exec();
    eprintln!("agent-notify: {}: {err}", agent.to_string_lossy());
    ExitCode::from(if err.kind() == std::io::ErrorKind::NotFound {
        127
    } else {
        126
    })
}

/// Set by SIGHUP: the terminal has gone.
static HUNG_UP: AtomicBool = AtomicBool::new(false);
/// Set by SIGTERM: the desktop session is ending, and brings the window back at the next login.
static TERMINATED: AtomicBool = AtomicBool::new(false);

extern "C" fn hung_up(_signal: libc::c_int) {
    HUNG_UP.store(true, Ordering::SeqCst);
}

extern "C" fn terminated(_signal: libc::c_int) {
    TERMINATED.store(true, Ordering::SeqCst);
}

fn on_signal(signal: libc::c_int, handler: libc::sighandler_t) {
    // SAFETY: the handler only stores to an atomic, which is async-signal-safe; sigaction is
    // given a zeroed, then filled, struct.
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = handler;
        libc::sigemptyset(&mut action.sa_mask);
        libc::sigaction(signal, &action, std::ptr::null_mut());
    }
}

/// Whether this process still has a terminal: a hung-up one can still be closing round it.
fn terminal_still_there() -> bool {
    // SAFETY: both only read the terminal's settings into structs owned here.
    unsafe {
        let mut settings: libc::termios = std::mem::zeroed();
        let mut size: libc::winsize = std::mem::zeroed();
        libc::tcgetattr(libc::STDIN_FILENO, &mut settings) == 0
            && libc::ioctl(libc::STDIN_FILENO, libc::TIOCGWINSZ, &mut size) == 0
    }
}

/// Attaches this terminal to background session `id`, reopening it if it has stopped, and
/// stays until it detaches.
///
/// Logging out carries the session on: the desktop sends this process SIGTERM before it ends the
/// terminal, and its layout attaches the window again at the next login. Closing the window
/// only hangs this process up, and a session still working carries on. Leaving the session in
/// the window (detaching, or quitting the agent view) stops it, as quitting the agent does
/// anywhere else: its conversation is kept, and attaching opens it again. A session another
/// window has attached to since is that window's, and is left to it.
fn attach_to(env: &Env, agent: &str, id: &str) -> ! {
    let me = std::process::id();
    let window = state::make_state(env).map(|dir| dir.join(format!("{agent}-{id}.window")));
    if let Some(window) = &window {
        let _ = state::write_line(window, me);
    }
    let resume = env
        .resume_dir()
        .filter(|dir| fs::create_dir_all(dir).is_ok())
        .map(|dir| dir.join(me.to_string()));
    if let Some(resume) = &resume {
        let again = format!(
            "{} keep {agent} attach {id}",
            shell_quote(&env.self_path().to_string_lossy())
        );
        let _ = state::write_line(resume, again);
    }
    on_signal(
        libc::SIGHUP,
        hung_up as extern "C" fn(libc::c_int) as libc::sighandler_t,
    );
    on_signal(
        libc::SIGTERM,
        terminated as extern "C" fn(libc::c_int) as libc::sighandler_t,
    );
    let _ = env.command(agent).args(["attach", id]).status();
    let terminated = TERMINATED.load(Ordering::SeqCst);
    if terminated || HUNG_UP.load(Ordering::SeqCst) {
        if stops_when_closed(
            terminated,
            claude::is_under_way(env, id),
            taken_over(window.as_deref(), me),
        ) {
            stop(env, agent, id);
        }
        std::process::exit(0);
    }
    if !terminal_still_there() {
        std::process::exit(0);
    }
    // This window no longer shows the session, so it isn't brought back with it at login.
    if taken_over(window.as_deref(), me) {
        if let Some(resume) = &resume {
            let _ = fs::remove_file(resume);
        }
        std::process::exit(0);
    }
    // Extra presses of the key that detached mustn't cut the stop short, or close the shell.
    on_signal(libc::SIGINT, libc::SIG_IGN);
    stop(env, agent, id);
    for file in window.iter().chain(&resume) {
        let _ = fs::remove_file(file);
    }
    std::process::exit(0);
}

/// Whether a session whose window has gone is stopped. One left by a logout is brought back with
/// the layout. Otherwise a session with nothing under way would sit in memory with no terminal
/// to reach it by, so it is stopped, unless another window has it: its conversation is kept, and
/// attaching opens it again.
fn stops_when_closed(terminated: bool, under_way: bool, taken_over: bool) -> bool {
    !terminated && !under_way && !taken_over
}

/// Whether another window has attached to the session since this one did: each attach writes
/// its own process into the session's window file.
fn taken_over(window: Option<&std::path::Path>, me: u32) -> bool {
    window
        .and_then(state::number_in)
        .is_some_and(|pid| pid != u64::from(me))
}

fn stop(env: &Env, agent: &str, id: &str) {
    let _ = env
        .command(agent)
        .args(["stop", id])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_agent_s_own_resume_command_is_noted_again() {
        let args = |words: &[&str]| words.iter().map(OsString::from).collect::<Vec<_>>();
        let id = "019a3c41-715c-7e62-8df4-6424ac1b27d6";
        assert_eq!(
            resumed("codex resume", "codex", &args(&["resume", id])),
            Some(id)
        );
        assert_eq!(
            resumed("agy --conversation", "agy", &args(&["--conversation", id])),
            Some(id)
        );
        assert_eq!(resumed("codex resume", "codex", &args(&[])), None);
        assert_eq!(resumed("codex resume", "codex", &args(&[id])), None);
        assert_eq!(
            resumed("codex resume", "codex", &args(&["resume", id, "--yolo"])),
            None
        );
        assert_eq!(
            resumed(
                "codex resume",
                "codex",
                &args(&["resume", "$(reboot)abcdefgh"])
            ),
            None
        );
        assert_eq!(
            resumed("codex resume", "gemini", &args(&["resume", id])),
            None
        );
    }

    #[test]
    fn codex_is_told_to_run_the_session_itself() {
        let args = |words: &[&str]| words.iter().map(OsString::from).collect::<Vec<_>>();
        assert_eq!(on_its_own(&args(&[])), args(&["--no-daemon"]));
        assert_eq!(
            on_its_own(&args(&["resume", "019a3c41-715c"])),
            args(&["resume", "--no-daemon", "019a3c41-715c"])
        );
        assert_eq!(
            on_its_own(&args(&["-m", "gpt-5"])),
            args(&["--no-daemon", "-m", "gpt-5"])
        );
    }

    #[test]
    fn a_logout_never_stops_the_session() {
        for under_way in [false, true] {
            for taken_over in [false, true] {
                assert!(!stops_when_closed(true, under_way, taken_over));
            }
        }
    }

    #[test]
    fn closing_the_window_stops_only_an_idle_session_no_other_window_has() {
        assert!(stops_when_closed(false, false, false));
        assert!(!stops_when_closed(false, true, false));
        assert!(!stops_when_closed(false, false, true));
    }
}
