//! Who is logged in, and in which session, as the login manager records it.
//!
//! On Linux the login manager (systemd-logind) keeps one small `KEY=value`
//! file per session under `/run/systemd/sessions`. This reader takes those
//! files through an [`FsRoot`] instead of running `loginctl`: it needs no
//! program, no permission and no parsing of a table meant for people, and
//! tests read checked-in fixture trees. A system without that folder has no
//! login manager (or none we know), so the reader gives no sessions and the
//! Users page keeps its process-owner grouping alone.

use crate::fs_root::FsRoot;

/// Where the login manager keeps its session files, under the root.
const SESSIONS_DIR: &str = "run/systemd/sessions";

/// One login session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    /// The session's id (`2`, `c1`), the name of its file.
    pub id: String,
    /// The account that owns it.
    pub user: String,
    /// `true` while it is the session in front on its seat.
    pub active: bool,
    /// The login manager's state: `active`, `online` or `closing`.
    pub state: String,
    /// The terminal (`tty2`, `pts/0`); empty when it has none.
    pub tty: String,
    /// The seat it sits at (`seat0`); empty for a session with no seat.
    pub seat: String,
    /// The host it came from for a remote login; empty for a local one.
    pub remote_host: String,
}

impl Session {
    /// One line telling where and how the session runs, for the Users page.
    pub fn describe(&self) -> String {
        let place = if !self.remote_host.is_empty() {
            format!("from {}", self.remote_host)
        } else if !self.seat.is_empty() {
            format!("on {}", self.seat)
        } else {
            "local".to_string()
        };
        let mut parts = vec![format!("Session {}", self.id), place];
        if !self.tty.is_empty() {
            parts.push(self.tty.clone());
        }
        parts.push(if self.state.is_empty() { "unknown".to_string() } else { self.state.clone() });
        parts.join(" \u{b7} ")
    }
}

/// The user sessions under `root`, by id; none when there is no login
/// manager. Greeter and lock-screen sessions are not users, so only the
/// `user` class is kept.
pub fn read(root: &FsRoot) -> Vec<Session> {
    let Ok(dir) = std::fs::read_dir(root.path(SESSIONS_DIR)) else {
        return Vec::new();
    };
    let mut all: Vec<Session> = dir
        .flatten()
        .filter_map(|e| {
            let id = e.file_name().to_string_lossy().into_owned();
            // A `<id>.ref` file lists a session's holders, not the session.
            if id.contains('.') {
                return None;
            }
            parse(&id, &root.read(&format!("{SESSIONS_DIR}/{id}"))?)
        })
        .collect();
    all.sort_by(|a, b| (a.user.as_str(), id_order(&a.id)).cmp(&(b.user.as_str(), id_order(&b.id))));
    all
}

/// Ids sort by number first (`2` before `10`), then by text.
fn id_order(id: &str) -> (u64, String) {
    (id.parse().unwrap_or(u64::MAX), id.to_string())
}

fn parse(id: &str, text: &str) -> Option<Session> {
    let get = |key: &str| text.lines().find_map(|l| l.strip_prefix(key)?.strip_prefix('=')).unwrap_or("").to_string();
    let class = get("CLASS");
    if !class.is_empty() && class != "user" {
        return None;
    }
    let user = get("USER");
    (!user.is_empty()).then(|| Session {
        id: id.to_string(),
        user,
        active: get("ACTIVE") == "1",
        state: get("STATE"),
        tty: get("TTY"),
        seat: get("SEAT"),
        remote_host: get("REMOTE_HOST"),
    })
}

/// The running system's sessions: the login manager's on Linux, none on
/// the other systems.
pub fn current() -> Vec<Session> {
    #[cfg(target_os = "linux")]
    {
        read(&FsRoot::system())
    }
    #[cfg(not(target_os = "linux"))]
    {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs_root::fixture;

    #[test]
    fn it_reads_the_user_sessions_of_the_login_manager() {
        let sessions = read(&fixture("desktop"));
        let ids: Vec<&str> = sessions.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["2", "7"], "the greeter and the .ref file are not sessions");
        assert_eq!(sessions[0].user, "spata");
        assert!(sessions[0].active);
        assert_eq!(sessions[0].tty, "tty2");
        assert_eq!(sessions[1].remote_host, "192.0.2.7");
        assert!(!sessions[1].active);
    }

    #[test]
    fn a_system_without_a_login_manager_has_no_sessions() {
        assert!(read(&fixture("arm-board")).is_empty());
    }

    #[test]
    fn a_session_is_described_by_place_terminal_and_state() {
        let sessions = read(&fixture("desktop"));
        assert_eq!(sessions[0].describe(), "Session 2 \u{b7} on seat0 \u{b7} tty2 \u{b7} active");
        assert_eq!(sessions[1].describe(), "Session 7 \u{b7} from 192.0.2.7 \u{b7} pts/0 \u{b7} online");
    }
}
