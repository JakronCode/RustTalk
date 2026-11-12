//! Bounded admin-password guessing limits, retained across client reconnects.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

const MAX_SOURCES: usize = 1024;
const RETENTION: Duration = Duration::from_secs(300);
const LIMITED: &str = "Too many admin attempts; wait before trying again";

struct Attempt {
    failures: u32,
    last: Instant,
    next: Instant,
}

pub struct AdminAuth {
    digest: Option<[u8; 32]>,
    attempts: HashMap<String, Attempt>,
    window: Instant,
    window_attempts: u8,
}

impl AdminAuth {
    pub fn new(password: Option<&str>) -> Self {
        Self {
            digest: password
                .filter(|s| !s.is_empty())
                .map(|s| Sha256::digest(s.as_bytes()).into()),
            attempts: HashMap::new(),
            window: Instant::now(),
            window_attempts: 0,
        }
    }

    pub fn verify(
        &mut self,
        source: &str,
        password: Option<&str>,
        now: Instant,
    ) -> Result<(), &'static str> {
        let expected = self.digest.ok_or("This server already has an admin")?;
        self.attempts
            .retain(|_, a| now.saturating_duration_since(a.last) < RETENTION);
        if self.attempts.get(source).is_some_and(|a| now < a.next)
            || (!self.attempts.contains_key(source) && self.attempts.len() >= MAX_SOURCES)
        {
            return Err(LIMITED);
        }
        if now.saturating_duration_since(self.window) >= Duration::from_secs(1) {
            self.window = now;
            self.window_attempts = 0;
        }
        if self.window_attempts >= 10 {
            return Err(LIMITED);
        }
        self.window_attempts += 1;
        // Hash first so the constant-time comparison always processes exactly 32 bytes.
        let supplied: [u8; 32] = Sha256::digest(password.unwrap_or_default().as_bytes()).into();
        if bool::from(expected.ct_eq(&supplied)) {
            self.attempts.remove(source);
            return Ok(());
        }
        let attempt = self.attempts.entry(source.to_string()).or_insert(Attempt {
            failures: 0,
            last: now,
            next: now,
        });
        attempt.failures = (attempt.failures + 1).min(7);
        attempt.last = now;
        attempt.next = now + Duration::from_secs((1u64 << (attempt.failures - 1)).min(60));
        Err("Wrong admin password")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_backoff_survives_reconnect_and_resets_after_success() {
        let mut auth = AdminAuth::new(Some("correct password"));
        let start = Instant::now();
        assert_eq!(
            auth.verify("same-ip", Some("wrong"), start),
            Err("Wrong admin password")
        );
        // A new client id/identity still supplies the same network source.
        assert_eq!(
            auth.verify("same-ip", Some("correct password"), start),
            Err(LIMITED)
        );
        let later = start + Duration::from_secs(1);
        assert_eq!(
            auth.verify("same-ip", Some("wrong again"), later),
            Err("Wrong admin password")
        );
        assert_eq!(
            auth.verify(
                "same-ip",
                Some("correct password"),
                later + Duration::from_secs(1)
            ),
            Err(LIMITED)
        );
        assert!(auth
            .verify(
                "same-ip",
                Some("correct password"),
                later + Duration::from_secs(2)
            )
            .is_ok());
        assert!(!auth.attempts.contains_key("same-ip"));
        assert!(auth
            .verify("other-ip", Some("correct password"), start)
            .is_ok());
    }

    #[test]
    fn global_budget_bounds_rotating_sources_and_expires_records() {
        let mut auth = AdminAuth::new(Some("secret"));
        let now = Instant::now();
        for i in 0..10 {
            assert_eq!(
                auth.verify(&i.to_string(), None, now),
                Err("Wrong admin password")
            );
        }
        assert_eq!(auth.verify("new", None, now), Err(LIMITED));
        for i in 10..MAX_SOURCES {
            let time = now + Duration::from_secs((i / 10) as u64);
            assert_eq!(
                auth.verify(&i.to_string(), None, time),
                Err("Wrong admin password")
            );
        }
        assert_eq!(auth.attempts.len(), MAX_SOURCES);
        assert_eq!(
            auth.verify("overflow", None, now + Duration::from_secs(104)),
            Err(LIMITED)
        );
        assert!(auth
            .verify("fresh", Some("secret"), now + Duration::from_secs(500))
            .is_ok());
        assert!(auth.attempts.is_empty());
    }
}
