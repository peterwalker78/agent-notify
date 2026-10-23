# agent-notify

Desktop notifications, working notes and plan limits for terminal coding agents: Claude Code, Gemini CLI, Codex CLI and Antigravity CLI.

Agents run it from their hooks. When a task finishes, or an agent is waiting on you, you get a notification, sent from inside the agent's own process tree so that a desktop that looks, as Slipstream does, can take you to the agent's terminal window. A task that ran a long time is announced with a sound, and only an agent waiting on you after a long task is critical, so a desktop that honours the notification spec wakes the screen for that and nothing else.

It is also the first plugin for [Slipstream](https://github.com/peterwalker78/slipstream-desktop)'s plugin panel, and an example of how one is made: a card with each open session, what it is doing and how many subagents it has working, and your plan limits as gauges. It works with any desktop that shows freedesktop notifications; the card, the working notes and the session restore need Slipstream.

## Install

**With Slipstream:** Slipstream's installer asks whether to add agent-notify along with its other extras. To add it later, run `scripts/install-extras --ask` from Slipstream's download.

**On its own:** download `agent-notify-x86_64.tar.gz` from the [latest release](https://github.com/peterwalker78/agent-notify/releases/latest), check it against the `.sha256` beside it, unpack it and put `bin/agent-notify` on your PATH. It runs on any current x86-64 distribution and needs nothing installed beside it.

Then wire up your agents:

```sh
agent-notify setup claude gemini   # the agents you use; with none named, the ones in the config
agent-notify status                # what's configured, wired and listening
```

`setup` adds its hooks to each agent's own settings file, beside any hooks you already have, and keeps a backup of the file. `agent-notify setup --remove` takes them all out again. Codex runs new hooks only once you trust them, in its `/hooks` screen.

## What it does

- **Notifications.** A finished task or an agent waiting on you, each session's notification replacing its last, and withdrawn when you send the next prompt.
- **Working notes.** While a session is open, a note in `$XDG_RUNTIME_DIR/slipstream/working/` says what it's called, whether it is working, and what its subagents are doing. Slipstream draws these as traces on the faded desktop, and on the card.
- **Bringing sessions back.** When a session opens, a note in `$XDG_RUNTIME_DIR/slipstream/resume/` holds the command that reopens it, so Slipstream can bring the agent back in its terminal with the rest of your layout at the next login.
- **Plan limits.** `agent-notify usage` prints how much of each agent's limits is used, as JSON, from a cache the hooks refresh every few minutes at most. A notification comes as each limit passes 75%, 90% and 100%, once per window.
- **Working through a logout.** `agent-notify keep claude` starts a session in the background with the terminal attached to it. Closing the window or logging out leaves it working, and at the next login Slipstream reattaches it.

`agent-notify --help` lists every command and setting.

## The plugin, and what it can reach

The release is a plugin folder as it is: `plugin.toml`, `bin/agent-notify` and the licences. The plugin runs `agent-notify card`, which prints its card as a line of JSON each time something on it changes.

The card only reads. It never goes on the network, sends notifications or opens an agent's credentials: the hooks, which the agent runs with your own access, do all of that and leave their results in files. So the card asks Slipstream for nothing but read access to three folders, the working notes, the hooks' own state and the usage cache, which `plugin.toml` lists:

```toml
exec = ["bin/agent-notify", "card"]
read = [
    "$XDG_RUNTIME_DIR/slipstream/working",
    "$XDG_RUNTIME_DIR/agent-notify",
    "$XDG_CACHE_HOME/agent-notify",
]
```

The test `the_card_reads_the_notes_from_the_folders_it_is_handed` runs the card with only those folders to go on.

## Config

`~/.config/agent-notify/config`, one `key = value` per line:

```
agents = claude gemini
long-task-secs = 120
sound-finished = complete
sound-attention = window-attention
usage-refresh-secs = 300
usage-alerts = 75 90 100
```

Hooks that go wrong say why in `~/.local/state/agent-notify/agent-notify.log`, and never fail the agent.

## Building

```sh
cargo build --release --locked
cargo test
scripts/package          # dist/agent-notify-x86_64.tar.gz, the plugin folder packed
```

AI coding tools are used in writing agent-notify's code. Every change is built and tested in CI.

## Licence

GPL-3.0-or-later. The licences of the Rust crates built in are in the release's `licences/crates.txt`.
