//! Keeps a background session's work counted with the terminal it is attached to.
//!
//! Desktops tell how much each app is using by its cgroup, and terminals keep each tab in a
//! cgroup of its own. A background session runs under the agent's own service instead, which
//! stays in the cgroup of whichever terminal first started it, so its work is counted there, or
//! to a window long closed. So systemd is asked to move the session in with the terminal it is
//! attached to.
//!
//! Only systemd moves it, and it only takes a process into a unit that has handed its cgroups to
//! the app (`Delegate=yes`), as Konsole's does. When it says no, the session stays where it is.
//! Moving it changes nothing else: its terminal, its signals and its parent are all as they were.

use std::path::Path;
use std::time::Duration;

use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::OwnedObjectPath;

use crate::procs::Procs;
use crate::state;

const SYSTEMD: &str = "org.freedesktop.systemd1";
const MANAGER: &str = "/org/freedesktop/systemd1";
const TIMEOUT: Duration = Duration::from_secs(2);

/// Asks for the session's process `agent`, and everything under it, to be moved into the cgroup
/// that `window` is in. `tried` keeps the last cgroup asked for, so each is asked for once: a
/// terminal that has no cgroup to share costs nothing after the first look.
pub fn follow(procs: &Procs, tried: &Path, agent: u32, window: u32) -> zbus::Result<()> {
    let Some(target) = procs.cgroup(window) else {
        return Ok(());
    };
    if procs.cgroup(agent).as_ref() == Some(&target)
        || state::first_line(tried).as_ref() == Some(&target)
    {
        return Ok(());
    }
    let _ = state::write_line(tried, &target);
    let bus = zbus::blocking::connection::Builder::session()?
        .method_timeout(TIMEOUT)
        .build()?;
    let manager = Proxy::new(&bus, SYSTEMD, MANAGER, "org.freedesktop.systemd1.Manager")?;
    let unit: OwnedObjectPath = manager.call("GetUnitByPID", &(window,))?;
    let id: String = property(&bus, &unit, "Unit", "Id")?;
    let kind = match id.rsplit_once('.') {
        Some((_, "scope")) => "Scope",
        Some((_, "service")) => "Service",
        _ => return Ok(()),
    };
    let own: String = property(&bus, &unit, kind, "ControlGroup")?;
    let Some(below) = below(&target, &own) else {
        return Ok(());
    };
    manager.call::<_, _, ()>("AttachProcessesToUnit", &(id, below, procs.tree(agent)))
}

fn property(
    bus: &Connection,
    unit: &OwnedObjectPath,
    interface: &str,
    name: &str,
) -> zbus::Result<String> {
    let interface = format!("org.freedesktop.systemd1.{interface}");
    Proxy::new(bus, SYSTEMD, unit.as_str(), interface)?.get_property(name)
}

/// Where `target` is within a unit's own cgroup `own`, as systemd takes it: empty for the unit's
/// own, `/tab(7).scope` for one the app made inside it. None when it isn't inside at all.
fn below<'a>(target: &'a str, own: &str) -> Option<&'a str> {
    let rest = target.strip_prefix(own)?;
    (rest.is_empty() || rest.starts_with('/')).then_some(rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tab_is_named_from_inside_its_terminal() {
        let own = "/user.slice/app.slice/app-org.kde.konsole-9.scope";
        assert_eq!(
            below(&format!("{own}/tab(12).scope"), own),
            Some("/tab(12).scope")
        );
        assert_eq!(below(own, own), Some(""));
        assert_eq!(below(&format!("{own}0/tab(1).scope"), own), None);
        assert_eq!(below("/user.slice/session-2.scope", own), None);
    }
}
