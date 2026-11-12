//! Turns raw connect/login failures into a short title, explanation and action.

use super::icons;

/// Longest raw message kept for the Details view (server text is untrusted).
const MAX_RAW_CHARS: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Server protocol is older than ours.
    ServerOutdated {
        client: u32,
        server: u32,
    },
    /// Our protocol is older than the server's (or the versions could not be read).
    ClientOutdated {
        client: Option<u32>,
        server: Option<u32>,
    },
    WrongPassword,
    InvalidInvite,
    Full,
    Banned,
    Kicked,
    Unreachable,
    NoAnswer,
    SecureFailed,
    ConnectionLost,
    /// Pinned certificate mismatch: hard stop, the user decides.
    CertChanged,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Retry,
    EditPassword,
    ShowUpdate,
    OpenReleases,
}

#[derive(Debug, Clone)]
pub struct JoinError {
    pub kind: Kind,
    pub raw: String,
    pub details_open: bool,
}

impl JoinError {
    pub fn classify(raw: &str) -> Self {
        let raw: String = raw.trim().chars().take(MAX_RAW_CHARS).collect();
        let l = raw.to_lowercase();
        let has = |needles: &[&str]| needles.iter().any(|n| l.contains(n));
        let kind = if raw.starts_with(rusttalk_shared::secure::CERT_CHANGED) {
            Kind::CertChanged
        } else if l.contains("protocol version not supported") {
            match parse_versions(&l) {
                Some((client, server)) if client > server => {
                    Kind::ServerOutdated { client, server }
                }
                v => Kind::ClientOutdated {
                    client: v.map(|v| v.0),
                    server: v.map(|v| v.1),
                },
            }
        } else if l.contains("wrong or missing server password") {
            Kind::WrongPassword
        } else if l.contains("invalid or expired invite") {
            Kind::InvalidInvite
        } else if l.contains("server is full") {
            Kind::Full
        } else if l.starts_with("you are banned") || l.starts_with("banned ") {
            Kind::Banned
        } else if l.starts_with("kicked by") {
            Kind::Kicked
        } else if has(&[
            "timeout while connecting",
            "connection refused",
            "actively refused",
            "failed to lookup address",
            "no such host",
            "name or service not known",
            "network is unreachable",
            "host is unreachable",
            "os error 10051",
            "os error 10060",
            "os error 10061",
            "os error 10065",
            "os error 11001",
        ]) {
            Kind::Unreachable
        } else if has(&[
            "timeout during the secure handshake",
            "timeout while waiting for the server",
        ]) {
            Kind::NoAnswer
        } else if l.contains("secure connection failed") {
            Kind::SecureFailed
        } else if has(&["the server closed the connection", "network error"]) {
            Kind::ConnectionLost
        } else {
            Kind::Other
        };
        Self {
            kind,
            raw,
            details_open: false,
        }
    }

    /// Retrying won't help: stop automatic reconnects and show the panel.
    pub fn is_fatal(&self) -> bool {
        matches!(
            self.kind,
            Kind::ServerOutdated { .. }
                | Kind::ClientOutdated { .. }
                | Kind::WrongPassword
                | Kind::InvalidInvite
                | Kind::Banned
                | Kind::Kicked
                | Kind::CertChanged
        )
    }

    pub fn icon(&self) -> &'static str {
        match self.kind {
            Kind::ServerOutdated { .. } => icons::regular::ARROW_CIRCLE_DOWN,
            Kind::ClientOutdated { .. } => icons::regular::ARROW_CIRCLE_UP,
            Kind::WrongPassword | Kind::InvalidInvite => icons::regular::LOCK_KEY,
            Kind::Full => icons::regular::USERS_THREE,
            Kind::Banned | Kind::Kicked => icons::regular::PROHIBIT,
            Kind::Unreachable => icons::regular::WIFI_SLASH,
            Kind::NoAnswer => icons::regular::HOURGLASS,
            Kind::SecureFailed | Kind::CertChanged => icons::regular::SHIELD_WARNING,
            Kind::ConnectionLost => icons::regular::LINK_BREAK,
            Kind::Other => icons::regular::WARNING_CIRCLE,
        }
    }

    pub fn title(&self) -> &'static str {
        match self.kind {
            Kind::ServerOutdated { .. } => "Server is running an older version",
            Kind::ClientOutdated { .. } => "Your RustTalk is out of date",
            Kind::WrongPassword => "Wrong password",
            Kind::InvalidInvite => "Invite not valid",
            Kind::Full => "Server is full",
            Kind::Banned => "You are banned",
            Kind::Kicked => "You were kicked",
            Kind::Unreachable => "Server not reachable",
            Kind::NoAnswer => "Server did not answer",
            Kind::SecureFailed => "Secure connection failed",
            Kind::ConnectionLost => "Connection lost",
            Kind::CertChanged => "Server identity changed",
            Kind::Other => "Could not connect",
        }
    }

    /// One sentence on what happened.
    pub fn explanation(&self) -> String {
        match self.kind {
            Kind::ServerOutdated { client, server } => {
                format!("This server speaks protocol v{server}, your RustTalk speaks v{client}. Both must match.")
            }
            Kind::ClientOutdated { client: Some(c), server: Some(s) } => {
                format!("This server speaks protocol v{s}, your RustTalk speaks v{c}. Both must match.")
            }
            Kind::ClientOutdated { .. } => "This server needs a different RustTalk version than yours.".into(),
            Kind::WrongPassword => "This server needs a password, and the one entered was missing or not accepted.".into(),
            Kind::InvalidInvite => "The invite link was already used up or has expired.".into(),
            Kind::Full => "The server has reached its user limit.".into(),
            // The server's own sentence carries the duration and reason.
            Kind::Banned | Kind::Kicked => sentence(&self.raw),
            Kind::Unreachable => "No RustTalk server could be reached at this address.".into(),
            Kind::NoAnswer => "A connection was made, but the server did not respond in time.".into(),
            Kind::SecureFailed => "The encrypted connection could not be set up.".into(),
            Kind::ConnectionLost => "The connection to the server dropped.".into(),
            Kind::CertChanged => "Its encryption certificate is not the one RustTalk saw before. That is expected after the \
                                   server was reinstalled or its data folder was reset. Otherwise someone may be listening in."
                .into(),
            Kind::Other => "The server refused the connection or something went wrong on the way.".into(),
        }
    }

    pub fn suggestion(&self) -> &'static str {
        match self.kind {
            Kind::ServerOutdated { .. } => "Ask the server owner to update the server. Connecting across versions is not supported yet.",
            Kind::ClientOutdated { .. } => "Update RustTalk, then connect again.",
            Kind::WrongPassword => "Enter the server password, or use an invite link.",
            Kind::InvalidInvite => "Ask for a new invite link, or enter the server password.",
            Kind::Full => "Try again in a little while.",
            Kind::Banned => "Ask a moderator of this server if you think this is a mistake.",
            Kind::Kicked => "You can join again.",
            Kind::Unreachable => "Check the address and port (default 7331), your internet connection and whether the server is running.",
            Kind::NoAnswer => "The server may be busy, or this address is not a RustTalk server. Try again in a moment.",
            Kind::SecureFailed => "Check that this address points to a RustTalk server. Very old servers are not supported.",
            Kind::ConnectionLost => "Check your internet connection and try again.",
            Kind::CertChanged => "Ask the server owner whether the fingerprint below matches the one in the server log.",
            Kind::Other => "Try again, or show the details to the server owner.",
        }
    }

    pub fn actions(&self) -> &'static [Action] {
        match self.kind {
            Kind::ServerOutdated { .. } => &[Action::OpenReleases],
            Kind::ClientOutdated { .. } => &[Action::ShowUpdate, Action::OpenReleases],
            Kind::WrongPassword | Kind::InvalidInvite => &[Action::EditPassword],
            Kind::Banned | Kind::CertChanged => &[],
            Kind::Full
            | Kind::Kicked
            | Kind::Unreachable
            | Kind::NoAnswer
            | Kind::SecureFailed
            | Kind::ConnectionLost
            | Kind::Other => &[Action::Retry],
        }
    }
}

impl Action {
    pub fn label(self) -> String {
        let (icon, text) = match self {
            Action::Retry => (icons::regular::ARROW_CLOCKWISE, "Try again"),
            Action::EditPassword => (icons::regular::PENCIL_SIMPLE, "Enter password"),
            Action::ShowUpdate => (icons::regular::ARROW_CIRCLE_UP, "Check for update"),
            Action::OpenReleases => (icons::regular::LINK, "Open releases page"),
        };
        format!("{icon}  {text}")
    }
}

/// "client: 15, server: 14" from the server's version error.
fn parse_versions(lower: &str) -> Option<(u32, u32)> {
    let num_after = |key: &str| -> Option<u32> {
        let rest = &lower[lower.find(key)? + key.len()..];
        let digits: String = rest
            .trim_start()
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        digits.parse().ok()
    };
    Some((num_after("client:")?, num_after("server:")?))
}

fn sentence(raw: &str) -> String {
    let mut s = raw.trim().to_string();
    if !s.ends_with(['.', '!', '?']) {
        s.push('.');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    // Raw strings as the server (Server/src/tcp.rs, state.rs) and net.rs produce them.
    #[test]
    fn protocol_mismatch_server_older() {
        let e = JoinError::classify(
            "Protocol version not supported (client: 15, server: 14) - please update RustTalk",
        );
        assert_eq!(
            e.kind,
            Kind::ServerOutdated {
                client: 15,
                server: 14
            }
        );
        assert_eq!(e.title(), "Server is running an older version");
        assert!(e.explanation().contains("v14") && e.explanation().contains("v15"));
        assert!(e.is_fatal());
        assert_eq!(e.actions(), &[Action::OpenReleases]);
    }

    #[test]
    fn protocol_mismatch_client_older() {
        let e = JoinError::classify(
            "Protocol version not supported (client: 13, server: 15) - please update RustTalk",
        );
        assert_eq!(
            e.kind,
            Kind::ClientOutdated {
                client: Some(13),
                server: Some(15)
            }
        );
        assert!(e.actions().contains(&Action::ShowUpdate));
        let odd = JoinError::classify("Protocol version not supported");
        assert_eq!(
            odd.kind,
            Kind::ClientOutdated {
                client: None,
                server: None
            }
        );
    }

    #[test]
    fn login_refusals() {
        assert_eq!(
            JoinError::classify("Wrong or missing server password").kind,
            Kind::WrongPassword
        );
        assert_eq!(
            JoinError::classify("Invalid or expired invite").kind,
            Kind::InvalidInvite
        );
        assert_eq!(JoinError::classify("Server is full").kind, Kind::Full);
        let ban = JoinError::classify("You are banned from this server for 5 more minute(s): spam");
        assert_eq!(ban.kind, Kind::Banned);
        assert_eq!(
            ban.explanation(),
            "You are banned from this server for 5 more minute(s): spam."
        );
        assert!(ban.is_fatal() && ban.actions().is_empty());
        assert_eq!(
            JoinError::classify("Banned permanently by anna").kind,
            Kind::Banned
        );
        assert_eq!(
            JoinError::classify("Kicked by anna: calm down").kind,
            Kind::Kicked
        );
        assert!(!JoinError::classify("Server is full").is_fatal());
    }

    #[test]
    fn network_failures() {
        for raw in [
            "Timeout while connecting to example.org:7331",
            "Connection refused (os error 111)",
            "No connection could be made because the target machine actively refused it. (os error 10061)",
            "failed to lookup address information: Name or service not known",
            "No such host is known. (os error 11001)",
        ] {
            let e = JoinError::classify(raw);
            assert_eq!(e.kind, Kind::Unreachable, "{raw}");
            assert!(!e.is_fatal());
        }
        assert_eq!(
            JoinError::classify("Timeout during the secure handshake with x:7331").kind,
            Kind::NoAnswer
        );
        assert_eq!(
            JoinError::classify("Timeout while waiting for the server").kind,
            Kind::NoAnswer
        );
        assert_eq!(
            JoinError::classify(
                "Secure connection failed: received corrupt message (old server version?)"
            )
            .kind,
            Kind::SecureFailed
        );
        assert_eq!(
            JoinError::classify("The server closed the connection").kind,
            Kind::ConnectionLost
        );
    }

    #[test]
    fn changed_certificate_is_a_hard_stop() {
        let e = JoinError::classify(&format!(
            "{}: AB:CD:EF",
            rusttalk_shared::secure::CERT_CHANGED
        ));
        assert_eq!(e.kind, Kind::CertChanged);
        assert!(e.is_fatal());
        assert!(
            !e.actions().contains(&Action::Retry),
            "never offer a plain retry past a changed certificate"
        );
    }

    #[test]
    fn unknown_keeps_raw_and_bounds_it() {
        let e = JoinError::classify("  Something odd happened  ");
        assert_eq!(e.kind, Kind::Other);
        assert_eq!(e.raw, "Something odd happened");
        let long = "x".repeat(10_000);
        assert_eq!(
            JoinError::classify(&long).raw.chars().count(),
            MAX_RAW_CHARS
        );
        let multibyte = "ü".repeat(1_000);
        assert_eq!(
            JoinError::classify(&multibyte).raw.chars().count(),
            MAX_RAW_CHARS
        );
    }

    #[test]
    fn version_parsing_is_robust() {
        assert_eq!(parse_versions("(client: 15, server: 14)"), Some((15, 14)));
        assert_eq!(parse_versions("client: , server: 14"), None);
        assert_eq!(parse_versions("client: 99999999999, server: 1"), None);
        assert_eq!(parse_versions("nothing"), None);
    }
}
