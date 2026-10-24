//! Notification rules (SPEC.md §8.3, docs/design/Notifications.dc.html). Each rule fires once
//! per window instance (account + window + `resets_at`); what has fired is remembered across
//! restarts by the app (`Memory` is serialisable).
//!
//! - Thresholds: the highest of warn_at / crit_at / 100% that was crossed.
//! - Projection: projected to run out before the reset, and either soon (within the lead time) or
//!   well before the reset (by at least the lead time).
//! - Reset: a window that just started over (off by default).
//! - Auth: an account that needs signing in again (always on, once per expiry).
//! - Budget (`evaluate_live`): 80% and 100% of the daily and monthly spend goals, once per day
//!   or month.
//! - Burn (`evaluate_live`): a session spending much faster than usual, at most every 2 hours.
//!
//! A projection alert carries the "switch to save" tip for its window when there is one.
//!
//! Every rule that fires comes back with `desktop` saying whether its switch is on; the desktop
//! shows only those, and Rattleboard (rattleboard.rs) picks by its own level. Pausing holds both.

use crate::forecast;
use crate::view::{AccountState, AccountView, HotSession, Settings, WindowView};
use chrono::TimeZone;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

const MIN: i64 = 60_000;
const DAY: i64 = 24 * 60 * MIN;

#[derive(Debug, Clone, PartialEq)]
pub struct Alert {
    pub account_id: String,
    /// The window it's about (None for a login)
    pub window_id: Option<String>,
    pub kind: Kind,
    /// Its desktop notification switch is on
    pub desktop: bool,
    pub title: String,
    pub body: String,
}

/// Which rule fired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The warning threshold
    Warn,
    /// The critical threshold or 100%
    Crit,
    /// Projected to run out before the reset
    Projection,
    Reset,
    /// Needs signing in again
    Login,
    /// A spend goal reached 80% or 100%
    Budget,
    /// A session spending much faster than usual
    Burn,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Memory {
    /// Rule key → when it can be forgotten (ms)
    fired: BTreeMap<String, i64>,
    /// "account/window" → the resets_at last seen
    resets: BTreeMap<String, i64>,
    /// Accounts already told they need to sign in
    auth: BTreeSet<String>,
}

fn lower_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) if s.split_whitespace().count() == 1 => f.to_lowercase().collect::<String>() + c.as_str(),
        _ => s.to_string(),
    }
}

/// "the week", "the 5-hour window", "the month"
fn period(w: &WindowView) -> &'static str {
    if w.id.starts_with("weekly") {
        "the week"
    } else if w.id == "monthly" || w.id == "budget" {
        "the month"
    } else {
        "the window"
    }
}

/// "at 13:05" today, "on Thu 15:00" another day (local time).
fn out_words(at: i64, now: i64) -> String {
    let w = forecast::when(at, now);
    if w.contains(' ') {
        format!("on {w}")
    } else {
        format!("at {w}")
    }
}

impl Memory {
    fn fire(&mut self, key: String, until: i64) -> bool {
        self.fired.insert(key, until).is_none()
    }

    /// Like `fire`, but the key can fire again once `until` has passed.
    fn fire_again_after(&mut self, key: String, until: i64, now: i64) -> bool {
        if self.fired.get(&key).is_some_and(|u| *u > now) {
            return false;
        }
        self.fired.insert(key, until);
        true
    }

    /// Spend goals and fast sessions, from the agents' logs. Paused like `evaluate`.
    pub fn evaluate_live(&mut self, today: Option<f64>, month: Option<f64>, hot: &[HotSession], s: &Settings, now: i64) -> Vec<Alert> {
        let mut out = Vec::new();
        let date = chrono::Local.timestamp_millis_opt(now).earliest().map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_default();
        let goals = [("d", "today's", s.budget_daily, today, 2 * DAY), ("m", "this month's", s.budget_monthly, month, 32 * DAY)];
        for (k, name, goal, spent, keep) in goals {
            let (Some(spent), true) = (spent, goal > 0.0) else { continue };
            let pct = spent / goal * 100.0;
            let period = if k == "d" { date.clone() } else { date.get(..7).unwrap_or_default().to_string() };
            let Some(level) = [100, 80].into_iter().find(|l| pct >= *l as f64) else { continue };
            // Crossing 100% also counts as having passed 80%, so 80% doesn't follow it.
            let mut new = false;
            for l in [80, 100].into_iter().filter(|l| *l <= level) {
                new |= self.fire(format!("budget/{k}/{period}/{l}"), now + keep) && l == level;
            }
            if new {
                let (title, body) = if level == 100 {
                    (
                        format!("You've reached {name} {} goal", crate::allowance::usd(goal)),
                        format!("Your agents have spent {} (API-equivalent).", crate::allowance::usd(spent)),
                    )
                } else {
                    (
                        format!("{pct:.0}% of {name} {} goal", crate::allowance::usd(goal)),
                        format!("Your agents have spent {} so far (API-equivalent).", crate::allowance::usd(spent)),
                    )
                };
                out.push(Alert { account_id: String::new(), window_id: None, kind: Kind::Budget, desktop: s.notify_budget, title, body });
            }
        }
        for h in hot {
            if self.fire_again_after(format!("burn/{}/{}", h.harness, h.session_id), now + 2 * 60 * MIN, now) {
                let pace = if h.ratio > 0.0 { format!(", {:.0}× your usual pace", h.ratio) } else { String::new() };
                out.push(Alert {
                    account_id: String::new(),
                    window_id: None,
                    kind: Kind::Burn,
                    desktop: s.notify_burn,
                    title: format!("Fast session in {}", h.project),
                    body: format!(
                        "This {} session spent {} in the last {} minutes{pace}.",
                        h.harness,
                        crate::allowance::usd(h.cost),
                        h.minutes
                    ),
                });
            }
        }
        if now < s.paused_until {
            return Vec::new();
        }
        out
    }

    /// The notifications to show now. While paused, rules still count as fired, so resuming
    /// doesn't bring a backlog.
    pub fn evaluate(&mut self, accounts: &[AccountView], s: &Settings, now: i64) -> Vec<Alert> {
        let mut out = Vec::new();
        let tips = super::tips::tips(accounts);
        for a in accounts {
            if a.state == AccountState::Login {
                if self.auth.insert(a.id.clone()) {
                    out.push(Alert {
                        account_id: a.id.clone(),
                        window_id: None,
                        kind: Kind::Login,
                        desktop: true,
                        title: format!("{} needs you to sign in again", a.name),
                        body: a.problem.clone().unwrap_or_else(|| "The login expired.".into()),
                    });
                }
            } else if a.state == AccountState::Ok {
                self.auth.remove(&a.id);
            }
            for w in &a.windows {
                let base = format!("{}/{}", a.id, w.id);
                let inst = format!("{base}/{}", w.resets_at.unwrap_or(0));
                let until = w.resets_at.unwrap_or(now + 7 * DAY);
                // Reset
                if let Some(r) = w.resets_at {
                    let prev = self.resets.insert(base.clone(), r);
                    if prev.is_some_and(|p| r > p + 10 * MIN) {
                        out.push(Alert {
                            account_id: a.id.clone(),
                            window_id: Some(w.id.clone()),
                            kind: Kind::Reset,
                            desktop: s.notify_reset,
                            title: format!("{} · {} just reset", a.name, w.label),
                            body: format!("You're back to {:.0}%.", w.pct),
                        });
                    }
                }
                // Thresholds: only the highest one crossed is announced.
                let levels = [s.warn_at, s.crit_at, 100.0];
                if let Some(top) = levels.iter().copied().filter(|l| w.pct >= *l).reduce(f64::max) {
                    let mut new = false;
                    for l in levels.iter().filter(|l| **l <= top) {
                        new |= self.fire(format!("{inst}/t{l}"), until) && *l == top;
                    }
                    if new {
                        let mut body = format!("{:.0}% of {} used", w.pct, lower_first(&w.label));
                        if let Some(r) = w.resets_at {
                            body += &format!(", resets {}", forecast::when(r, now));
                        }
                        body.push('.');
                        match (w.out_at, w.proj) {
                            (Some(o), _) => body += &format!(" At this pace you'll run out {}.", out_words(o, now)),
                            (None, Some(p)) if w.pct < 100.0 => body += &format!(" On track to finish {} at ~{:.0}%.", period(w), p),
                            _ => {}
                        }
                        out.push(Alert {
                            account_id: a.id.clone(),
                            window_id: Some(w.id.clone()),
                            kind: if top >= s.crit_at { Kind::Crit } else { Kind::Warn },
                            desktop: s.notify_thresholds,
                            title: a.name.clone(),
                            body,
                        });
                    }
                }
                // Projection
                if let (Some(o), Some(r)) = (w.out_at, w.resets_at) {
                    let lead = s.lead_minutes as i64 * MIN;
                    if (o - now <= lead || r - o >= lead) && self.fire(format!("{inst}/proj"), until) {
                        out.push(Alert {
                            account_id: a.id.clone(),
                            window_id: Some(w.id.clone()),
                            kind: Kind::Projection,
                            desktop: s.notify_projection,
                            title: format!("{} · {} at {:.0}%", a.name, w.label, w.pct),
                            body: {
                                let mut b = format!(
                                    "At this pace you'll run out {}, {} before reset.",
                                    out_words(o, now),
                                    forecast::duration(r - o)
                                );
                                if let Some(t) = tips.iter().find(|t| t.account_id == a.id && t.window_id == w.id) {
                                    b = format!("{b} {}", t.text);
                                }
                                b
                            },
                        });
                    }
                }
            }
        }
        self.fired.retain(|_, until| *until > now - DAY);
        if now < s.paused_until {
            return Vec::new();
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::Status;

    fn acct(pct: f64, out_at: Option<i64>, resets_at: i64) -> AccountView {
        AccountView {
            id: "a".into(),
            name: "Claude — work".into(),
            state: AccountState::Ok,
            windows: vec![WindowView {
                id: "weekly".into(),
                label: "Weekly".into(),
                pct,
                proj: Some(96.0),
                resets_at: Some(resets_at),
                out_at,
                status: Status::Warn,
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    /// What the desktop shows.
    fn desk(v: Vec<Alert>) -> Vec<Alert> {
        v.into_iter().filter(|a| a.desktop).collect()
    }

    #[test]
    fn thresholds_fire_once_per_window() {
        let (now, reset) = (1_000 * DAY, 1_000 * DAY + 3 * DAY);
        let s = Settings::default();
        let mut m = Memory::default();
        assert!(m.evaluate(&[acct(50.0, None, reset)], &s, now).is_empty());
        let a = m.evaluate(&[acct(91.0, None, reset)], &s, now);
        assert_eq!(a.len(), 1, "only the highest threshold: {a:?}");
        assert_eq!(a[0].title, "Claude — work");
        assert!(a[0].body.starts_with("91% of weekly used, resets "), "{}", a[0].body);
        assert!(a[0].body.ends_with("On track to finish the week at ~96%."), "{}", a[0].body);
        assert!(m.evaluate(&[acct(92.0, None, reset)], &s, now).is_empty(), "once");
        assert_eq!(m.evaluate(&[acct(100.0, None, reset)], &s, now).len(), 1, "then 100%");
        // A new window instance starts over.
        let a = m.evaluate(&[acct(80.0, None, reset + 7 * DAY)], &s, now + 4 * DAY);
        assert_eq!(desk(a.clone()).len(), 1);
        assert_eq!(desk(a.clone())[0].kind, Kind::Warn);
        // The reset fired too, for the board, with its desktop switch off.
        assert_eq!(a.iter().map(|x| (x.kind, x.desktop)).collect::<Vec<_>>(), [(Kind::Reset, false), (Kind::Warn, true)]);
    }

    #[test]
    fn switches_only_affect_the_desktop() {
        let (now, reset) = (1_000 * DAY, 1_000 * DAY + 160 * MIN);
        let s = Settings { notify_thresholds: false, notify_projection: false, ..Default::default() };
        let mut m = Memory::default();
        let a = m.evaluate(&[acct(95.0, Some(now + 30 * MIN), reset)], &s, now);
        assert_eq!(a.iter().map(|x| (x.kind, x.desktop)).collect::<Vec<_>>(), [(Kind::Crit, false), (Kind::Projection, false)]);
        assert!(a.iter().all(|x| x.window_id.as_deref() == Some("weekly")));
        assert!(m.evaluate(&[acct(96.0, Some(now + 30 * MIN), reset)], &s, now).is_empty(), "still once per window");
    }

    #[test]
    fn projection_and_pause() {
        let now = 1_000 * DAY;
        let reset = now + 160 * MIN;
        let mut s = Settings::default();
        let mut m = Memory::default();
        let a = m.evaluate(&[acct(60.0, Some(now + 86 * MIN), reset)], &s, now);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].title, "Claude — work · Weekly at 60%");
        assert!(a[0].body.starts_with("At this pace you'll run out at "), "{}", a[0].body);
        assert!(a[0].body.ends_with(", 1h 14m before reset."), "{}", a[0].body);
        s.paused_until = i64::MAX;
        let mut m2 = Memory::default();
        assert!(m2.evaluate(&[acct(95.0, Some(now + 30 * MIN), reset)], &s, now).is_empty());
        s.paused_until = 0;
        assert!(m2.evaluate(&[acct(95.0, Some(now + 30 * MIN), reset)], &s, now).is_empty(), "no backlog after resuming");
    }

    #[test]
    fn auth_once_per_expiry() {
        let s = Settings::default();
        let mut m = Memory::default();
        let mut a = acct(10.0, None, 5 * DAY);
        a.state = AccountState::Login;
        a.problem = Some("Run `claude` to sign in again".into());
        assert_eq!(m.evaluate(&[a.clone()], &s, DAY)[0].title, "Claude — work needs you to sign in again");
        assert!(m.evaluate(&[a.clone()], &s, DAY).is_empty());
        let mut ok = a.clone();
        ok.state = AccountState::Ok;
        m.evaluate(&[ok], &s, DAY);
        assert_eq!(m.evaluate(&[a], &s, DAY).len(), 1, "expired again");
    }

    #[test]
    fn budget_goals_at_80_and_100() {
        let s = Settings { budget_daily: 50.0, budget_monthly: 1000.0, ..Default::default() };
        let mut m = Memory::default();
        let now = 1_792_807_612_967;
        assert!(m.evaluate_live(Some(30.0), Some(500.0), &[], &s, now).is_empty());
        let a = m.evaluate_live(Some(41.0), Some(500.0), &[], &s, now);
        assert_eq!(a.len(), 1);
        assert_eq!((a[0].kind, a[0].title.as_str()), (Kind::Budget, "82% of today's $50 goal"));
        assert_eq!(a[0].body, "Your agents have spent $41 so far (API-equivalent).");
        assert!(m.evaluate_live(Some(45.0), Some(500.0), &[], &s, now).is_empty(), "once");
        let a = m.evaluate_live(Some(52.5), Some(1200.0), &[], &s, now);
        assert_eq!(
            a.iter().map(|x| x.title.as_str()).collect::<Vec<_>>(),
            ["You've reached today's $50 goal", "You've reached this month's $1,000 goal"]
        );
        // A new day starts over; off when the goal is 0.
        assert_eq!(m.evaluate_live(Some(45.0), Some(1200.0), &[], &s, now + DAY).len(), 1);
        assert!(Memory::default().evaluate_live(Some(999.0), Some(9999.0), &[], &Settings::default(), now).is_empty());
    }

    #[test]
    fn burn_every_two_hours_at_most() {
        let s = Settings::default();
        let mut m = Memory::default();
        let h = HotSession {
            harness: "Claude Code".into(),
            session_id: "s1".into(),
            project: "third-eye".into(),
            cost: 18.0,
            minutes: 20,
            ratio: 3.4,
            started: 0,
        };
        let a = m.evaluate_live(None, None, std::slice::from_ref(&h), &s, DAY);
        assert_eq!(a[0].title, "Fast session in third-eye");
        assert_eq!(a[0].body, "This Claude Code session spent $18 in the last 20 minutes, 3× your usual pace.");
        assert!(m.evaluate_live(None, None, std::slice::from_ref(&h), &s, DAY + 60 * MIN).is_empty());
        assert_eq!(m.evaluate_live(None, None, &[h], &s, DAY + 121 * MIN).len(), 1);
    }

    #[test]
    fn projection_carries_the_tip() {
        let now = 1_000 * DAY;
        let reset = now + 160 * MIN;
        let mut tight = acct(60.0, Some(now + 86 * MIN), reset);
        tight.provider = "anthropic".into();
        tight.windows[0].status = Status::Crit;
        let roomy = AccountView {
            id: "b".into(),
            provider: "openai".into(),
            name: "ChatGPT".into(),
            state: AccountState::Ok,
            windows: vec![WindowView {
                id: "weekly".into(),
                label: "Codex weekly".into(),
                pct: 12.0,
                status: Status::Ok,
                ..Default::default()
            }],
            ..Default::default()
        };
        let a = Memory::default().evaluate(&[tight, roomy], &Settings::default(), now);
        let p = a.iter().find(|x| x.kind == Kind::Projection).unwrap();
        assert!(p.body.ends_with("before reset. ChatGPT has room (Codex weekly at 12%): move your next task there."), "{}", p.body);
    }

    #[test]
    fn reset_rule() {
        let s = Settings { notify_reset: true, ..Default::default() };
        let mut m = Memory::default();
        m.evaluate(&[acct(10.0, None, 5 * DAY)], &s, DAY);
        let a = m.evaluate(&[acct(1.0, None, 12 * DAY)], &s, 6 * DAY);
        assert_eq!(a[0].title, "Claude — work · Weekly just reset");
    }
}
