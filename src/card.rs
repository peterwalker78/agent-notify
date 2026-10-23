//! `agent-notify card`: a card for a desktop's plugin panel. It runs until the desktop stops it
//! and prints one line of JSON each time the plan limits `usage` keeps in its cache change, as
//! gauges. It leaves the chip's headline to the desktop, which shows the first gauges, and lists no
//! sessions: whoever opens the panel is already working with their agents.
//!
//! It only reads the usage cache, which the hooks refresh. It asks no one over the network, sends
//! no notifications and opens no agent's credentials, so a desktop can run it with nothing but
//! that folder to read. It watches the folder with inotify, so it wakes only when it changes.

use std::ffi::CString;
use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use serde_json::{Value, json};

use crate::env::Env;
use crate::usage;

/// How often the cache is read anyway, in case a change was missed.
const RECHECK: Duration = Duration::from_secs(60);
/// How long to let a burst of changes settle before reading them.
const SETTLE: Duration = Duration::from_millis(100);

pub fn run(env: &Env) -> ExitCode {
    let watched = vec![env.usage_dir()];
    let mut shown = None;
    let mut watch: Option<Watch> = None;
    let mut stdout = io::stdout().lock();
    loop {
        // A folder made after the card started is watched from then on.
        if watch.as_ref().is_none_or(|watch| watch.missing(&watched)) {
            watch = Watch::new(&watched);
        }
        let line = card(usage::cached_sections(env)).to_string();
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

/// The card for these usage sections.
fn card(sections: Vec<Value>) -> Value {
    let lines: Vec<&str> = if sections.is_empty() {
        vec!["No plan limits yet: they come once an agent has run"]
    } else {
        Vec::new()
    };
    json!({"lines": lines, "sections": sections})
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_no_usage_yet_the_card_says_when_it_comes() {
        assert_eq!(
            card(Vec::new()),
            json!({"lines": ["No plan limits yet: they come once an agent has run"], "sections": []})
        );
    }

    #[test]
    fn the_card_is_the_usage_and_leaves_the_headline_to_the_desktop() {
        let usage =
            vec![json!({"name": "Claude", "meters": [{"label": "Session", "percent": 40}]})];
        let card = card(usage.clone());
        assert_eq!(card, json!({"lines": [], "sections": usage}));
        assert_eq!(card.get("glance"), None);
    }
}
