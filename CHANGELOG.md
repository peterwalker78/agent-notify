# Changelog

## Unreleased

- **No more phantom sessions.** Claude Code starts a spare background session ahead of time so the next one opens at once, and its start hook used to leave notes for it as though someone were working there: an extra idle trace on Slipstream's faded desktop, and a session to bring back that was nobody's. A spare leaves no notes until a job takes it.
- **Every kept session carries on through a logout in Slipstream.** One sitting at its prompt used to be stopped at logout and opened again at login; it now keeps running, and its window attaches to it again. Closing a window still stops a session with nothing under way.
- **A session waiting on your answer carries on through a logout.** `keep` stopped a background session at logout unless it was working, which lost the question it was waiting to ask you; one waiting on you now stays, as a working one does.
- **A session attached from a second window is left to that window.** When the first window's attach ends after that, it used to stop the session under the window now showing it; it now leaves the session running, and isn't brought back at login with it, since the other window has it.
- **The card is your plan limits and nothing else.** It no longer lists the open sessions, since whoever opens the panel is working with them already, and it leaves the chip to show the first gauges rather than how many sessions are working. It reads only the usage cache now, and the hooks no longer leave a marker for a session waiting on you.
- **Each plan limit says how long its window is,** as `window` in seconds beside `resets`, so a desktop can show how far through the window you are. It's left out where the length isn't known.
- **`usage-agents` chooses whose limits are reported,** for when you use an agent without wanting its limits on show. Every agent wired up is reported if it's left out.

## 0.1.0 - 2026-10-23

The first release on its own. agent-notify used to come with Slipstream; it is now a project of its own, and a plugin for Slipstream's plugin panel.

- **A card for Slipstream's plugin panel.** `agent-notify card` lists each open session, what it is doing, how many subagents it has working and whether it is waiting on you, with your plan limits as gauges. It only reads the folders its `plugin.toml` names.
- **The hooks leave the command that reopens a session,** so Slipstream brings it back with the layout. A separate script did this before, and `agent-notify setup` replaces its hook.
- **The hooks keep the plan limits fresh,** so they and their alerts no longer wait for something to ask for them.
- **The release is the plugin folder, packed,** with its SHA-256 beside it and an attestation of where it was built.
- **`agent-notify bar` is gone;** the card replaces it.
