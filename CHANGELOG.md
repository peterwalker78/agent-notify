# Changelog

## 0.1.0 - 2026-10-23

The first release on its own. agent-notify used to come with Slipstream; it is now a project of its own, and a plugin for Slipstream's plugin panel.

- **A card for Slipstream's plugin panel.** `agent-notify card` lists each open session, what it is doing, how many subagents it has working and whether it is waiting on you, with your plan limits as gauges. It only reads the folders its `plugin.toml` names.
- **The hooks leave the command that reopens a session,** so Slipstream brings it back with the layout. A separate script did this before, and `agent-notify setup` replaces its hook.
- **The hooks keep the plan limits fresh,** so they and their alerts no longer wait for something to ask for them.
- **The release is the plugin folder, packed,** with its SHA-256 beside it and an attestation of where it was built.
- **`agent-notify bar` is gone;** the card replaces it.
