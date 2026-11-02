//! Per-URL health state and its transitions (pure, no I/O).
//!
//! ```text
//! Active --dead--> Suspect --ok--> Active
//! Suspect --dead (consecutive, >= 24h after the first)--> Hidden
//! Hidden --ok--> Recovering --dead--> Hidden
//! Recovering --ok x3 (consecutive)--> Active
//! ```

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// How long dead results must continue before a target is hidden.
pub const HIDE_AFTER: time::Duration = time::Duration::hours(24);
/// Consecutive healthy results needed to go from Recovering back to Active.
pub const RECOVER_AFTER: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    #[default]
    Active,
    Suspect,
    Hidden,
    Recovering,
}

/// Result of one check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    /// Counts as dead (404, 410, DNS failure, parked domain, ...).
    Dead(String),
    /// 401 / 403 / 429 / 5xx / timeout / TLS error: changes nothing.
    Undecided(String),
    /// Not checked on purpose (x.com, twitter.com).
    Unchecked,
}

impl Outcome {
    pub fn describe(&self) -> String {
        match self {
            Outcome::Ok => "ok".into(),
            Outcome::Dead(r) => format!("dead: {r}"),
            Outcome::Undecided(r) => format!("undecided: {r}"),
            Outcome::Unchecked => "unchecked".into(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub state: State,
    pub consecutive_dead: u32,
    pub consecutive_ok: u32,
    pub first_dead_at: Option<String>,
    pub last_checked_at: Option<String>,
    pub last_result: Option<String>,
    /// Reason of the latest dead result (used for `hidden_reason`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_dead_reason: Option<String>,
}

/// `health.json`: URL (or `repo:owner/name`) -> entry.
pub type Health = BTreeMap<String, Entry>;

fn fmt(t: OffsetDateTime) -> String {
    t.format(&Rfc3339).expect("RFC 3339 formatting")
}

/// Apply `outcome` observed at `now`. Returns `(from, to)` when the state
/// changed.
pub fn step(entry: &mut Entry, outcome: &Outcome, now: OffsetDateTime) -> Option<(State, State)> {
    let from = entry.state;
    entry.last_checked_at = Some(fmt(now));
    entry.last_result = Some(outcome.describe());
    match outcome {
        Outcome::Undecided(_) | Outcome::Unchecked => return None,
        Outcome::Dead(reason) => {
            entry.last_dead_reason = Some(reason.clone());
            entry.consecutive_ok = 0;
            entry.consecutive_dead += 1;
            match entry.state {
                State::Active => {
                    entry.state = State::Suspect;
                    entry.consecutive_dead = 1;
                    entry.first_dead_at = Some(fmt(now));
                }
                State::Suspect => {
                    let first = entry
                        .first_dead_at
                        .as_deref()
                        .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok())
                        .unwrap_or(now);
                    if now - first >= HIDE_AFTER {
                        entry.state = State::Hidden;
                    }
                }
                State::Recovering => entry.state = State::Hidden,
                State::Hidden => {}
            }
        }
        Outcome::Ok => {
            entry.consecutive_dead = 0;
            entry.consecutive_ok += 1;
            match entry.state {
                State::Active => entry.consecutive_ok = 0,
                State::Suspect => {
                    entry.state = State::Active;
                    entry.consecutive_ok = 0;
                    entry.first_dead_at = None;
                }
                State::Hidden => {
                    entry.state = State::Recovering;
                    entry.consecutive_ok = 1;
                }
                State::Recovering => {
                    if entry.consecutive_ok >= RECOVER_AFTER {
                        entry.state = State::Active;
                        entry.consecutive_ok = 0;
                        entry.first_dead_at = None;
                    }
                }
            }
        }
    }
    (entry.state != from).then_some((from, entry.state))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(h: i64) -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap() + time::Duration::hours(h)
    }
    fn dead() -> Outcome {
        Outcome::Dead("404".into())
    }

    #[test]
    fn dead_once_is_suspect_and_ok_clears_it() {
        let mut e = Entry::default();
        assert_eq!(
            step(&mut e, &dead(), t(0)),
            Some((State::Active, State::Suspect))
        );
        assert!(e.first_dead_at.is_some());
        assert_eq!(
            step(&mut e, &Outcome::Ok, t(1)),
            Some((State::Suspect, State::Active))
        );
        assert_eq!(e.consecutive_dead, 0);
        assert!(e.first_dead_at.is_none());
    }

    #[test]
    fn hidden_only_after_24_hours_of_dead() {
        let mut e = Entry::default();
        step(&mut e, &dead(), t(0));
        assert_eq!(step(&mut e, &dead(), t(6)), None);
        assert_eq!(step(&mut e, &dead(), t(23)), None);
        assert_eq!(e.state, State::Suspect);
        assert_eq!(e.consecutive_dead, 3);
        assert_eq!(
            step(&mut e, &dead(), t(24)),
            Some((State::Suspect, State::Hidden))
        );
    }

    #[test]
    fn undecided_changes_nothing_but_the_log() {
        let mut e = Entry::default();
        step(&mut e, &dead(), t(0));
        assert_eq!(step(&mut e, &Outcome::Undecided("503".into()), t(30)), None);
        assert_eq!(e.state, State::Suspect);
        assert_eq!(e.consecutive_dead, 1);
        assert_eq!(e.last_result.as_deref(), Some("undecided: 503"));
        assert_eq!(step(&mut e, &Outcome::Unchecked, t(31)), None);
    }

    #[test]
    fn recovery_needs_three_ok_in_a_row() {
        let mut e = Entry {
            state: State::Hidden,
            ..Entry::default()
        };
        assert_eq!(
            step(&mut e, &Outcome::Ok, t(0)),
            Some((State::Hidden, State::Recovering))
        );
        assert_eq!(step(&mut e, &Outcome::Ok, t(1)), None);
        assert_eq!(
            step(&mut e, &Outcome::Ok, t(2)),
            Some((State::Recovering, State::Active))
        );
        assert_eq!(e.consecutive_ok, 0);
    }

    #[test]
    fn dead_while_recovering_hides_again() {
        let mut e = Entry {
            state: State::Hidden,
            ..Entry::default()
        };
        step(&mut e, &Outcome::Ok, t(0));
        step(&mut e, &Outcome::Ok, t(1));
        assert_eq!(
            step(&mut e, &dead(), t(2)),
            Some((State::Recovering, State::Hidden))
        );
        assert_eq!(e.consecutive_ok, 0);
        // The three-in-a-row count restarts.
        step(&mut e, &Outcome::Ok, t(3));
        step(&mut e, &Outcome::Ok, t(4));
        assert_eq!(e.state, State::Recovering);
    }

    #[test]
    fn ok_when_active_stays_clean() {
        let mut e = Entry::default();
        assert_eq!(step(&mut e, &Outcome::Ok, t(0)), None);
        assert_eq!(e.consecutive_ok, 0);
    }
}
