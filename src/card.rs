//! `agent-notify card`: a card for a desktop's plugin panel. It runs until the desktop stops it
//! and prints one line of JSON each time something on it changes: a line for each open session,
//! by its trace's name, saying whether it is idle or waiting on you and how many subagents it has
//! working; the plan limits `usage` keeps in its cache, as gauges; and, at a glance, how many
//! sessions are working or waiting on you. It wants attention while one is waiting.
//!
//! It only reads: the working notes the hooks write, the hooks' waiting markers and the usage
//! cache, which the hooks refresh. It asks no one over the network, sends no notifications and
//! opens no agent's credentials, so a desktop can run it with nothing but those folders to read.
//! It watches them with inotify, so it wakes only when one changes, and every few seconds to
//! notice a session whose process has gone.

use std::ffi::CString;
use std::fs;
use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::env::{Env, uid};
use crate::hook::waiting_marker;
use crate::note::Note;
use crate::procs::Procs;
use crate::text::{is_number, last_part};
use crate::usage;

/// How often the processes behind the notes are checked, since a process ending changes no note.
const RECHECK: Duration = Duration::from_secs(5);
/// How long to let a burst of changes settle before reading them.
const SETTLE: Duration = Duration::from_millis(100);

pub fn run(env: &Env) -> ExitCode {
    let procs = Procs::system();
    let clock = Clock::read();
    let watched: Vec<PathBuf> = env
        .notes_dir()
        .into_iter()
        .chain([env.state_dir(), env.usage_dir()])
        .collect();
    let mut shown = None;
    let mut watch: Option<Watch> = None;
    let mut stdout = io::stdout().lock();
    loop {
        // A folder made after the card started is watched from then on.
        if watch.as_ref().is_none_or(|watch| watch.missing(&watched)) {
            watch = Watch::new(&watched);
        }
        let line = card(&sessions(env, &procs, &clock), usage::cached_sections(env)).to_string();
        if shown.as_ref() != Some(&line) {
            // The desktop has gone: so has the reason to run.
            if writeln!(stdout, "{line}")
                .and_then(|()| stdout.flush())
                .is_err()
            {
                return ExitCode::SUCCESS;
            }
            shown = Some(line);
        }
        match &watch {
            Some(current) => {
                if !current.wait(RECHECK) {
                    watch = None;
                }
            }
            None => std::thread::sleep(RECHECK),
        }
    }
}

/// An open session, as its note and the hooks' state describe it.
pub struct Session {
    pub pid: u32,
    pub note: Note,
    pub waiting: bool,
}

/// When processes started, for telling a note from a process that has gone from one left by a
/// new process with the same number.
struct Clock {
    boot: u64,
    ticks: u64,
}

impl Clock {
    fn read() -> Clock {
        let stat = fs::read_to_string("/proc/stat").unwrap_or_default();
        let boot = stat
            .lines()
            .find_map(|line| line.strip_prefix("btime ")?.trim().parse().ok());
        // SAFETY: sysconf only reads a system value.
        let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        Clock {
            boot: boot.unwrap_or(0),
            ticks: u64::try_from(ticks).unwrap_or(100).max(1),
        }
    }

    fn started(&self, procs: &Procs, pid: u32) -> Option<u64> {
        Some(self.boot + procs.start_ticks(pid)? / self.ticks)
    }
}

/// The open sessions, by process ID.
fn sessions(env: &Env, procs: &Procs, clock: &Clock) -> Vec<Session> {
    let Some(dir) = env.notes_dir() else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let state = env.state_dir();
    let mut sessions: Vec<Session> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry
                .file_name()
                .into_string()
                .ok()
                .filter(|name| is_number(name))?;
            let pid: u32 = name.parse().ok()?;
            if procs.owner(pid) != Some(uid()) {
                return None;
            }
            // A note older than its process belongs to one that had the number before.
            let written = entry.metadata().ok()?.modified().ok()?;
            let written = written.duration_since(UNIX_EPOCH).ok()?.as_secs();
            if written + 2 < clock.started(procs, pid)? {
                return None;
            }
            let mut note = Note::parse(&fs::read_to_string(entry.path()).ok()?);
            if note.name.is_empty() {
                note.name = procs
                    .cwd(pid)
                    .map(|cwd| last_part(&cwd.to_string_lossy()).to_owned())
                    .unwrap_or_default();
            }
            let waiting = note.idle && waiting_marker(&state, pid).exists();
            Some(Session { pid, note, waiting })
        })
        .collect();
    sessions.sort_by_key(|session| session.pid);
    sessions
}

/// The card for these sessions and usage sections.
fn card(sessions: &[Session], sections: Vec<Value>) -> Value {
    let working = sessions.iter().filter(|session| !session.note.idle).count();
    let waiting = sessions.iter().filter(|session| session.waiting).count();
    let mut lines: Vec<String> = sessions
        .iter()
        .map(|session| {
            let mut line = session.note.name.clone();
            if session.waiting {
                line.push_str(" · waiting on you");
            } else if session.note.idle {
                line.push_str(" · idle");
            }
            match session.note.helpers.len() {
                0 => {}
                1 => line.push_str(" · 1 subagent"),
                many => line.push_str(&format!(" · {many} subagents")),
            }
            line
        })
        .collect();
    if lines.is_empty() && sections.is_empty() {
        lines.push("No agent session open".into());
    }
    let mut card = json!({"lines": lines, "sections": sections, "attention": waiting > 0});
    let glance = if waiting > 0 {
        Some(format!("{waiting} waiting"))
    } else if working > 0 {
        Some(format!("{working} working"))
    } else {
        // Left to the desktop, which shows the first usage gauges.
        None
    };
    if let Some(glance) = glance {
        card["glance"] = json!(glance);
    }
    card
}

/// An inotify watch on the folders the card reads.
struct Watch {
    fd: OwnedFd,
    /// The folders that were there to watch.
    watching: Vec<PathBuf>,
}

impl Watch {
    /// Watches whichever of `dirs` exist; none if none does.
    fn new(dirs: &[PathBuf]) -> Option<Watch> {
        // SAFETY: inotify_init1 returns a new descriptor or -1, and OwnedFd takes sole charge of
        // it.
        let fd = unsafe {
            let fd = libc::inotify_init1(libc::IN_CLOEXEC | libc::IN_NONBLOCK);
            if fd < 0 {
                return None;
            }
            OwnedFd::from_raw_fd(fd)
        };
        let watching: Vec<PathBuf> = dirs
            .iter()
            .filter(|dir| add_watch(&fd, dir))
            .cloned()
            .collect();
        (!watching.is_empty()).then_some(Watch { fd, watching })
    }

    /// Whether one of `dirs` exists now but isn't watched.
    fn missing(&self, dirs: &[PathBuf]) -> bool {
        dirs.iter()
            .any(|dir| !self.watching.contains(dir) && dir.is_dir())
    }

    /// Waits up to `timeout` for a change. False once the folder itself has gone.
    fn wait(&self, timeout: Duration) -> bool {
        let mut poll = libc::pollfd {
            fd: self.fd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: poll waits on the one descriptor in a struct owned here.
        let ready = unsafe { libc::poll(&mut poll, 1, timeout.as_millis() as libc::c_int) };
        if ready <= 0 {
            return true;
        }
        std::thread::sleep(SETTLE);
        self.drain()
    }

    /// Reads every waiting event. False if one says the folder has gone.
    fn drain(&self) -> bool {
        let mut buffer = [0u8; 4096];
        let mut alive = true;
        loop {
            // SAFETY: read fills at most the buffer's length.
            let read = unsafe {
                libc::read(
                    self.fd.as_raw_fd(),
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                )
            };
            let Ok(read) = usize::try_from(read) else {
                return alive;
            };
            if read == 0 {
                return alive;
            }
            let mut at = 0;
            let header = std::mem::size_of::<libc::inotify_event>();
            while at + header <= read {
                // SAFETY: the kernel writes whole events, each a header and then `len` bytes of
                // name; read_unaligned copes with the buffer's alignment.
                let event: libc::inotify_event =
                    unsafe { std::ptr::read_unaligned(buffer[at..].as_ptr().cast()) };
                if event.mask & (libc::IN_IGNORED | libc::IN_DELETE_SELF | libc::IN_MOVE_SELF) != 0
                {
                    alive = false;
                }
                at += header + event.len as usize;
            }
        }
    }
}

/// Adds a watch on `dir` to the inotify descriptor `fd`. False if it can't be watched.
fn add_watch(fd: &OwnedFd, dir: &Path) -> bool {
    let Ok(path) = CString::new(dir.as_os_str().as_bytes()) else {
        return false;
    };
    let events = libc::IN_CREATE
        | libc::IN_DELETE
        | libc::IN_MOVED_TO
        | libc::IN_MOVED_FROM
        | libc::IN_CLOSE_WRITE
        | libc::IN_DELETE_SELF
        | libc::IN_MOVE_SELF
        | libc::IN_ONLYDIR;
    // SAFETY: inotify_add_watch reads the path for the length of the call.
    unsafe { libc::inotify_add_watch(fd.as_raw_fd(), path.as_ptr(), events) >= 0 }
}

/// Seconds since the epoch, for tests that make notes.
#[cfg(test)]
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(pid: u32, name: &str, idle: bool, helpers: &[&str], waiting: bool) -> Session {
        let note = Note {
            name: name.into(),
            idle,
            helpers: helpers.iter().map(|h| h.to_string()).collect(),
        };
        Session { pid, note, waiting }
    }

    #[test]
    fn with_nothing_open_and_no_usage_the_card_says_so() {
        assert_eq!(
            card(&[], Vec::new()),
            json!({"lines": ["No agent session open"], "sections": [], "attention": false})
        );
    }

    #[test]
    fn each_session_has_a_line_and_the_glance_counts_who_is_working() {
        let sessions = [
            session(
                10,
                "claude: fix <the> kiln",
                false,
                &["reading the tests", "fixing the docs"],
                false,
            ),
            session(11, "gemini: ledger", true, &[], false),
            session(12, "codex: orchard", false, &["one"], false),
        ];
        assert_eq!(
            card(&sessions, Vec::new()),
            json!({
                "lines": [
                    "claude: fix <the> kiln · 2 subagents",
                    "gemini: ledger · idle",
                    "codex: orchard · 1 subagent",
                ],
                "sections": [],
                "attention": false,
                "glance": "2 working",
            })
        );
    }

    #[test]
    fn one_waiting_on_you_wants_attention_and_takes_the_glance() {
        let sessions = [
            session(10, "claude: kiln", true, &[], true),
            session(11, "codex: x", false, &[], false),
        ];
        let card = card(&sessions, Vec::new());
        assert_eq!(card["lines"][0], "claude: kiln · waiting on you");
        assert_eq!(card["glance"], "1 waiting");
        assert_eq!(card["attention"], true);
    }

    #[test]
    fn with_every_session_idle_the_glance_is_left_to_the_usage_gauges() {
        let usage =
            vec![json!({"name": "Claude", "meters": [{"label": "Session", "percent": 40}]})];
        let card = card(
            &[session(10, "claude: kiln", true, &[], false)],
            usage.clone(),
        );
        assert_eq!(card.get("glance"), None);
        assert_eq!(card["sections"], json!(usage));
    }

    #[test]
    fn the_clock_dates_this_process_after_boot() {
        let clock = Clock::read();
        let started = clock.started(&Procs::system(), std::process::id()).unwrap();
        assert!(started <= now() + 1 && started + 86_400 * 365 > now());
    }
}
