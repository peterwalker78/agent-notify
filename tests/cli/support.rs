//! A scratch machine for each test: its own home, config, cache and runtime folders, fake agents
//! and tools on its PATH, and a record of the notifications that would have been sent.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// The process ID the hooks are told is the agent's, unless a test walks a real process tree.
pub const AGENT_PID: &str = "4242";

/// One notification call.
#[derive(Debug, PartialEq, Clone)]
pub enum Call {
    Notify {
        app: String,
        replaces: u32,
        summary: String,
        body: String,
        hints: BTreeMap<String, String>,
    },
    Close(u32),
}

/// A notification with the hints spelled out as pairs.
pub fn notify<K: AsRef<str>, V: AsRef<str>>(
    app: &str,
    replaces: u32,
    summary: &str,
    body: &str,
    hints: &[(K, V)],
) -> Call {
    Call::Notify {
        app: app.into(),
        replaces,
        summary: summary.into(),
        body: body.into(),
        hints: hints
            .iter()
            .map(|(key, value)| (key.as_ref().to_owned(), value.as_ref().to_owned()))
            .collect(),
    }
}

pub const QUIET: &[(&str, &str)] = &[("urgency", "y 1"), ("suppress-sound", "b true")];

pub struct Scratch {
    pub root: PathBuf,
    /// Extra variables for every command.
    vars: std::cell::RefCell<Vec<(String, String)>>,
}

/// A shell script that writes its arguments to `log`, each ended by \x1f, and the run by \x1e.
fn recorder(log: &Path, then: &str) -> String {
    format!(
        "#!/bin/sh\nprintf '%s\\037' \"$0\" \"$@\" >>'{}'\nprintf '\\036' >>'{}'\n{then}\n",
        log.display(),
        log.display()
    )
}

impl Scratch {
    pub fn new(test: &str) -> Scratch {
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join("cli")
            .join(test);
        let _ = fs::remove_dir_all(&root);
        for dir in ["home", "config", "cache", "state", "run", "bin", "logs"] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        fs::set_permissions(root.join("run"), fs::Permissions::from_mode(0o700)).unwrap();
        symlink(
            env!("CARGO_BIN_EXE_agent-notify"),
            root.join("bin/agent-notify"),
        )
        .unwrap();
        Scratch {
            root,
            vars: Default::default(),
        }
    }

    /// Sets a variable for every command run from now on.
    pub fn set_var(&self, key: &str, value: &str) {
        self.vars.borrow_mut().push((key.into(), value.into()));
    }

    /// Writes an executable into the scratch PATH.
    pub fn tool(&self, name: &str, script: &str) {
        self.tool_at(&self.root.join("bin").join(name), script);
    }

    /// Writes an executable.
    pub fn tool_at(&self, path: &Path, script: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, script).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// A fake agent (or tool) that records how it was run, then runs `then`.
    pub fn fake(&self, name: &str, then: &str) {
        self.tool(
            name,
            &recorder(&self.root.join(format!("logs/{name}")), then),
        );
    }

    /// How the fake called `name` was run, one list of arguments per run.
    pub fn runs(&self, name: &str) -> Vec<Vec<String>> {
        let log = fs::read_to_string(self.root.join(format!("logs/{name}"))).unwrap_or_default();
        log.split('\x1e')
            .filter(|run| !run.is_empty())
            .map(|line| {
                let mut args: Vec<String> = line.split('\x1f').map(str::to_owned).collect();
                args.pop();
                args.remove(0);
                args
            })
            .collect()
    }

    pub fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    pub fn write(&self, relative: &str, contents: &str) {
        let path = self.path(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            path,
            contents.replace("@ROOT@", &self.root.display().to_string()),
        )
        .unwrap();
    }

    pub fn read(&self, relative: &str) -> Option<String> {
        fs::read_to_string(self.path(relative)).ok()
    }

    /// The folder a desktop that shows agents at work makes for their notes.
    pub fn desktop(&self) {
        fs::create_dir_all(self.path("run/slipstream/working")).unwrap();
    }

    pub fn note(&self, pid: &str) -> Option<String> {
        self.read(&format!("run/slipstream/working/{pid}"))
    }

    pub fn config(&self, text: &str) {
        self.write("config/agent-notify/config", text);
    }

    /// A task that started `ago` seconds before now, by the clock the hooks use (seconds since
    /// boot), or at boot on a machine that has been up for less.
    /// Records that the session's task started `ago` seconds since, on the clock since boot,
    /// however long the machine has been up.
    pub fn started(&self, agent: &str, session: &str, ago: u64) {
        let uptime = fs::read_to_string("/proc/uptime").unwrap();
        let now: i64 = uptime.split('.').next().unwrap().parse().unwrap();
        self.write(
            &format!("run/agent-notify/{agent}-{session}.started"),
            &format!("{}\n", now - ago as i64),
        );
    }

    pub fn command(&self, args: &[&str]) -> Command {
        self.command_running(self.path("bin/agent-notify"), args)
    }

    /// A command run in the scratch machine's environment.
    pub fn command_running(&self, program: impl AsRef<std::ffi::OsStr>, args: &[&str]) -> Command {
        let mut command = Command::new(program);
        command
            .args(args)
            .env_clear()
            .env("HOME", self.path("home"))
            .env("XDG_CONFIG_HOME", self.path("config"))
            .env("XDG_CACHE_HOME", self.path("cache"))
            .env("XDG_STATE_HOME", self.path("state"))
            .env("XDG_RUNTIME_DIR", self.path("run"))
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.path("bin").display()),
            )
            .env("LANG", "C.UTF-8")
            .env("AGENT_NOTIFY_RECORD", self.path("logs/record"))
            .current_dir(self.path("home"));
        for (key, value) in self.vars.borrow().iter() {
            command.env(key, value);
        }
        command
    }

    pub fn run(&self, args: &[&str]) -> Output {
        let mut command = self.command(args);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        spawn(&mut command).wait_with_output().unwrap()
    }

    /// Runs a hook with `payload` (a fixture file's contents, or JSON) on stdin, as the agent
    /// with process ID AGENT_PID, and checks it says nothing and succeeds.
    pub fn hook(&self, agent: &str, event: &str, payload: &str) {
        let mut command = self.command(&["hook", agent, event]);
        command.env("AGENT_NOTIFY_PID", AGENT_PID);
        self.hook_with(command, payload);
    }

    pub fn hook_with(&self, mut command: Command, payload: &str) {
        let payload = payload.replace("@ROOT@", &self.root.display().to_string());
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = spawn(&mut command);
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "a hook never fails the agent");
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "",
            "a hook prints nothing"
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            "",
            "a hook prints nothing"
        );
    }

    /// The notification calls made so far, with how long a task took written as `<duration>`,
    /// since a clock that ticks during a run can add a second to it.
    pub fn calls(&self) -> Vec<Call> {
        let mut calls = recorded(&self.path("logs/record"));
        for call in &mut calls {
            if let Call::Notify { body, .. } = call {
                *body = unduration(body);
            }
        }
        calls
    }

    /// Every file the run left, by path, with its mode, and the root written as $ROOT.
    pub fn files(&self) -> BTreeMap<String, String> {
        let mut files = BTreeMap::new();
        for top in ["home", "config", "cache", "run"] {
            walk(&self.root, &self.root.join(top), &mut files);
        }
        files
    }
}

fn walk(root: &Path, dir: &Path, files: &mut BTreeMap<String, String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let meta = fs::symlink_metadata(&path).unwrap();
        let name = path.strip_prefix(root).unwrap().display().to_string();
        if meta.is_dir() {
            walk(root, &path, files);
        } else {
            let text = fs::read_to_string(&path).unwrap_or_else(|_| "<binary>".into());
            let text = text.replace(&root.display().to_string(), "$ROOT");
            // Clock readings, which no two runs share.
            let text = match path.extension().and_then(|extension| extension.to_str()) {
                Some("started") => "<uptime>".to_owned(),
                Some("asked" | "retry") => "<time>".to_owned(),
                _ => text,
            };
            files.insert(
                name,
                format!("[{:o}] {text}", meta.permissions().mode() & 0o777),
            );
        }
    }
}

/// Whether `text` is how long a task took: 42s, 7m 12s, 1h 05m or 5d 23h.
fn is_duration(text: &str) -> bool {
    let unit = |part: &str, units: &str| {
        part.len() > 1
            && part[..part.len() - 1]
                .bytes()
                .all(|byte| byte.is_ascii_digit())
            && units.contains(&part[part.len() - 1..])
    };
    match text.split_once(' ') {
        None => unit(text, "s"),
        Some((big, small)) => unit(big, "mhd") && unit(small, "smh") && small.len() == 3,
    }
}

/// The body with the duration on its first line, after the project or "Resets in", as
/// `<duration>`.
fn unduration(body: &str) -> String {
    if body.strip_prefix("Resets in ").is_some_and(is_duration) {
        return "Resets in <duration>".into();
    }
    let (first, rest) = body
        .split_once('\n')
        .map_or((body, None), |(first, rest)| (first, Some(rest)));
    let (project, last) = first
        .rsplit_once(" · ")
        .map_or(("", first), |(project, last)| (project, last));
    if !is_duration(last) {
        return body.to_owned();
    }
    let mut out = if project.is_empty() {
        String::new()
    } else {
        format!("{project} · ")
    };
    out.push_str("<duration>");
    if let Some(rest) = rest {
        out.push('\n');
        out.push_str(rest);
    }
    out
}

fn recorded(path: &Path) -> Vec<Call> {
    let text = fs::read_to_string(path).unwrap_or_default();
    text.lines()
        .map(|line| {
            let call: serde_json::Value = serde_json::from_str(line).unwrap();
            match call["call"].as_str().unwrap() {
                "Notify" => {
                    assert_eq!(call["icon"], "utilities-terminal");
                    assert_eq!(call["timeout"], -1);
                    let hints = call["hints"].as_object().unwrap();
                    Call::Notify {
                        app: call["app"].as_str().unwrap().into(),
                        replaces: call["replaces"].as_u64().unwrap() as u32,
                        summary: call["summary"].as_str().unwrap().into(),
                        body: call["body"].as_str().unwrap().into(),
                        hints: hints
                            .iter()
                            .map(|(key, value)| (key.clone(), value.as_str().unwrap().into()))
                            .collect(),
                    }
                }
                _ => Call::Close(call["id"].as_u64().unwrap() as u32),
            }
        })
        .collect()
}

/// Starts `command`. A program a test has just written can be busy for a moment: tests run in
/// parallel, and a process another one is starting holds the open file until it runs its own
/// program. So a busy one is tried again.
pub fn spawn(command: &mut Command) -> std::process::Child {
    let mut tries = 0;
    loop {
        match command.spawn() {
            Err(err) if err.kind() == std::io::ErrorKind::ExecutableFileBusy && tries < 100 => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            started => return started.unwrap(),
        }
    }
}

/// Runs `test` on a fresh scratch machine called `name`.
pub fn scenario(name: &str, test: impl Fn(&Scratch)) {
    test(&Scratch::new(name));
}
