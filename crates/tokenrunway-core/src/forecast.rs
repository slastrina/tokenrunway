//! Forecasts (SPEC.md §10.3): at the pace of the recent readings, when does a window run out, and
//! is that before it resets? This is the first, rate-only part; the hour-of-week profile comes
//! once there's history to learn it from.
//!
//! `window_view` turns a reading plus its history into what the popover draws (pct, pace tick,
//! projection, status and the forecast words), and `summary` picks the shortest runway.

use crate::allowance::Money;
use crate::model::WindowReading;
use crate::view::{AccountView, Settings, Status, Summary, WindowView};
use chrono::{Local, TimeZone};
use serde::Serialize;

/// Readings older than this don't count towards the pace.
pub const LOOKBACK_MS: i64 = 90 * 60 * 1000;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "status")]
pub enum Outlook {
    /// Not enough readings in the lookback, or nothing is being used.
    Unknown,
    /// At this pace it lasts until it resets (or there's no reset and it lasts beyond `out_at`).
    OnTrack { out_at: Option<i64> },
    /// It runs out at `out_at`, `margin_ms` before it resets.
    RunsOut { out_at: i64, margin_ms: i64 },
    /// It's already used up.
    Exhausted,
}

/// The pace, as a fraction of the window per millisecond, over the readings of one window
/// instance (same `resets_at`) within the lookback of the latest. Fit by least squares, so a
/// single noisy reading doesn't swing it.
pub fn rate(readings: &[WindowReading]) -> Option<f64> {
    let last = readings.iter().max_by_key(|r| r.at)?;
    let pts: Vec<(f64, f64)> =
        readings.iter().filter(|r| r.resets_at == last.resets_at && last.at - r.at <= LOOKBACK_MS).map(|r| (r.at as f64, r.used)).collect();
    if pts.len() < 2 {
        return None;
    }
    // Relative to the first reading: raw epoch milliseconds (~1.8e12) lose enough precision that a
    // flat series came out as a tiny positive rate.
    let t0 = pts[0].0;
    let pts: Vec<(f64, f64)> = pts.iter().map(|&(t, u)| (t - t0, u)).collect();
    if pts.iter().all(|p| p.1 == pts[0].1) {
        return Some(0.0);
    }
    let n = pts.len() as f64;
    let (mx, my) = (pts.iter().map(|p| p.0).sum::<f64>() / n, pts.iter().map(|p| p.1).sum::<f64>() / n);
    let sxx: f64 = pts.iter().map(|p| (p.0 - mx).powi(2)).sum();
    if sxx == 0.0 {
        return None;
    }
    Some(pts.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum::<f64>() / sxx)
}

/// Where the latest reading is heading.
pub fn outlook(readings: &[WindowReading]) -> Outlook {
    let Some(last) = readings.iter().max_by_key(|r| r.at) else { return Outlook::Unknown };
    if last.used >= 1.0 {
        return Outlook::Exhausted;
    }
    let Some(r) = rate(readings).filter(|r| *r > 0.0) else { return Outlook::Unknown };
    let out_at = last.at.saturating_add(((1.0 - last.used) / r) as i64);
    match last.resets_at {
        Some(reset) if out_at < reset => Outlook::RunsOut { out_at, margin_ms: reset - out_at },
        Some(_) => Outlook::OnTrack { out_at: None },
        None => Outlook::OnTrack { out_at: Some(out_at) },
    }
}

/// A recent rate needs readings spanning at least this long; until then the window's average
/// pace stands in.
pub const MIN_SPAN_MS: i64 = 15 * 60 * 1000;
/// The average pace isn't trusted this early in a window (share elapsed, %).
const MIN_PACE: f64 = 10.0;
const MIN: i64 = 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Thresholds {
    pub warn_at: f64,
    pub crit_at: f64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self { warn_at: 75.0, crit_at: 90.0 }
    }
}

impl From<&Settings> for Thresholds {
    fn from(s: &Settings) -> Self {
        Self { warn_at: s.warn_at, crit_at: s.crit_at }
    }
}

/// One window's latest reading, as `window_view` needs it.
#[derive(Debug, Clone)]
pub struct WindowInput<'a> {
    pub id: &'a str,
    pub label: &'a str,
    pub abs: Option<String>,
    /// 0.0–1.0
    pub used: f64,
    pub resets_at: Option<i64>,
    /// The window's fixed length (5 h, 7 d, the calendar month), for the pace tick
    pub length_ms: Option<i64>,
    /// Set for spend and budget windows
    pub money: Option<Money>,
}

/// crit 3, warn 2, ok 1, est 0: how urgent a status is.
pub fn severity(s: Status) -> u8 {
    match s {
        Status::Crit => 3,
        Status::Warn => 2,
        Status::Ok => 1,
        Status::Est => 0,
    }
}

/// The recent pace (fraction per ms) of the current window instance: a least-squares fit over
/// the lookback, only when the readings span at least `MIN_SPAN_MS`.
pub fn recent_rate(history: &[WindowReading], resets_at: Option<i64>, now: i64) -> Option<f64> {
    let same = |r: &WindowReading| match (r.resets_at, resets_at) {
        (Some(a), Some(b)) => (a - b).abs() <= 2 * MIN,
        (None, None) => true,
        _ => false,
    };
    let pts: Vec<&WindowReading> = history.iter().filter(|r| same(r) && now - r.at <= LOOKBACK_MS && r.at <= now).collect();
    let (first, last) = (pts.iter().map(|r| r.at).min()?, pts.iter().map(|r| r.at).max()?);
    if pts.len() < 3 || last - first < MIN_SPAN_MS {
        return None;
    }
    let normalised: Vec<WindowReading> = pts.iter().map(|r| WindowReading { resets_at: None, ..(*r).clone() }).collect();
    rate(&normalised)
}

/// Everything the popover draws for one window, in local time.
pub fn window_view(w: &WindowInput, history: &[WindowReading], now: i64, th: &Thresholds) -> WindowView {
    window_view_in(w, history, now, th, &Local)
}

pub fn window_view_in<Tz: TimeZone>(w: &WindowInput, history: &[WindowReading], now: i64, th: &Thresholds, tz: &Tz) -> WindowView
where
    Tz::Offset: std::fmt::Display,
{
    let pct = w.used * 100.0;
    let start = match (w.resets_at, w.length_ms) {
        (Some(r), Some(l)) if l > 0 => Some((r - l, l)),
        _ => None,
    };
    let pace = start.map(|(s, l)| ((now - s) as f64 / l as f64 * 100.0).clamp(0.0, 100.0));
    let rate = recent_rate(history, w.resets_at, now).map(|r| r.max(0.0));
    let has_limit = w.money.is_none_or(|m| m.limit.is_some());
    // Projected % at reset, and the run-out time if that's before the reset.
    let (proj, out_at) = match w.resets_at {
        Some(reset) if has_limit && reset > now => {
            let left = (reset - now) as f64;
            let r = rate.or_else(|| {
                // The window's average pace so far.
                let (s, _) = start?;
                (pace? >= MIN_PACE && now > s).then(|| w.used / (now - s) as f64)
            });
            match r {
                Some(r) => {
                    let proj = (w.used + r * left) * 100.0;
                    // Compared in f64 first: a near-zero rate puts the run-out time centuries
                    // away, which overflowed i64 and wrapped round to "already out".
                    let to_out = (1.0 - w.used) / r;
                    let out = (w.used < 1.0 && r > 0.0 && to_out < left).then(|| now + to_out.round() as i64);
                    (Some(proj), out)
                }
                None => (None, None),
            }
        }
        _ => (None, None),
    };
    let status = if pct >= 100.0 || pct >= th.crit_at || out_at.is_some() {
        Status::Crit
    } else if pct >= th.warn_at || proj.is_some_and(|p| p >= 95.0) {
        Status::Warn
    } else if w.money.is_some() {
        Status::Est
    } else {
        Status::Ok
    };
    let forecast = match status {
        Status::Crit if pct >= 100.0 => "Used up until reset".to_string(),
        Status::Crit if out_at.is_some() => {
            let o = out_at.unwrap_or(now);
            format!("Out {}, {} before reset", out_words(o, now, tz), duration(w.resets_at.unwrap_or(o) - o))
        }
        Status::Crit | Status::Warn => match proj {
            Some(p) => format!("Tight · ~{:.0}% at reset", p.min(999.0)),
            None => format!("{pct:.0}% used"),
        },
        Status::Est => {
            let m = w.money.map(|m| m.used).unwrap_or(0.0);
            let at_reset = match pace {
                Some(p) if p >= 3.0 => m / (p / 100.0),
                _ => m,
            };
            format!("Rough estimate · ~{}", crate::allowance::usd(at_reset))
        }
        Status::Ok => "On track".to_string(),
    };
    WindowView {
        id: w.id.to_string(),
        label: w.label.to_string(),
        abs: w.abs.clone(),
        pct,
        pace,
        proj: proj.map(|p| (p * 10.0).round() / 10.0),
        resets_at: w.resets_at,
        status,
        forecast,
        out_at,
    }
}

/// "at 13:05" today, "Thu 15:00" on another day.
fn out_words<Tz: TimeZone>(at: i64, now: i64, tz: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let (Some(a), Some(n)) = (tz.timestamp_millis_opt(at).single(), tz.timestamp_millis_opt(now).single()) else { return String::new() };
    if a.date_naive() == n.date_naive() {
        format!("at {}", a.format("%H:%M"))
    } else {
        a.format("%a %H:%M").to_string()
    }
}

/// When something happens, in the design's style: "14:20" today, "Mon 09:00" this week,
/// "1 Nov" further out. Local time.
pub fn when(at: i64, now: i64) -> String {
    when_in(at, now, &Local)
}

pub fn when_in<Tz: TimeZone>(at: i64, now: i64, tz: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let (Some(a), Some(n)) = (tz.timestamp_millis_opt(at).single(), tz.timestamp_millis_opt(now).single()) else { return String::new() };
    if a.date_naive() == n.date_naive() {
        a.format("%H:%M").to_string()
    } else if at - now < 6 * 24 * 3600 * 1000 {
        a.format("%a %H:%M").to_string()
    } else {
        a.format("%-d %b").to_string()
    }
}

/// "1h 26m", "2d 21h", "45m", "<1m".
pub fn duration(ms: i64) -> String {
    let m = ms.max(0) / MIN;
    let (d, h, mm) = (m / 1440, m / 60 % 24, m % 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {mm}m")
    } else if mm > 0 {
        format!("{mm}m")
    } else {
        "<1m".into()
    }
}

/// The tightest window across accounts: the most severe, then the highest projection.
pub fn tightest(accounts: &[AccountView]) -> Option<(&AccountView, &WindowView)> {
    accounts.iter().flat_map(|a| a.windows.iter().map(move |w| (a, w))).max_by(|x, y| {
        let k = |w: &WindowView| (severity(w.status), w.out_at.map(|o| -o).unwrap_or(i64::MIN), w.proj.unwrap_or(w.pct));
        let (a, b) = (k(x.1), k(y.1));
        a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal))
    })
}

/// The "Shortest runway" header: runway left on the tightest window (time to its run-out, or to
/// its reset when it's on track) and how many windows are in each state.
pub fn summary(accounts: &[AccountView], now: i64) -> Option<Summary> {
    let (a, w) = tightest(accounts)?;
    let label = match (w.out_at, w.resets_at) {
        (Some(o), _) => duration(o - now),
        (None, Some(r)) => duration(r - now),
        (None, None) => format!("{:.0}%", w.pct),
    };
    let all = accounts.iter().flat_map(|a| a.windows.iter());
    let count = |s: &[Status]| all.clone().filter(|w| s.contains(&w.status)).count() as u32;
    Some(Summary {
        label,
        status: w.status,
        window: format!("{} · {}", a.name, w.label),
        crit: count(&[Status::Crit]),
        warn: count(&[Status::Warn]),
        ok: count(&[Status::Ok, Status::Est]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::AccountView;
    use chrono::Utc;

    const MIN: i64 = 60 * 1000;

    fn r(at_min: i64, used: f64, reset_min: Option<i64>) -> WindowReading {
        WindowReading { window: "5h".into(), used, limit: None, resets_at: reset_min.map(|m| m * MIN), at: at_min * MIN }
    }

    #[test]
    fn runs_out_before_reset() {
        // 10% every 30 minutes from 60% at minute 60: out 120 minutes later, at 180, which is
        // 120 minutes before the reset at 300.
        let rs = [r(0, 0.4, Some(300)), r(30, 0.5, Some(300)), r(60, 0.6, Some(300))];
        match outlook(&rs) {
            Outlook::RunsOut { out_at, margin_ms } => {
                assert!((out_at - 180 * MIN).abs() < MIN, "{}", out_at / MIN);
                assert!((margin_ms - 120 * MIN).abs() < MIN, "{}", margin_ms / MIN);
            }
            o => panic!("{o:?}"),
        }
        let late = [r(0, 0.4, Some(150)), r(30, 0.5, Some(150)), r(60, 0.6, Some(150))];
        assert_eq!(outlook(&late), Outlook::OnTrack { out_at: None }, "resets at 150, before it would run out");
    }

    #[test]
    fn on_track_when_reset_comes_first() {
        let rs = [r(0, 0.10, Some(120)), r(60, 0.15, Some(120))];
        assert_eq!(outlook(&rs), Outlook::OnTrack { out_at: None });
    }

    #[test]
    fn ignores_old_and_previous_windows() {
        // The reading from the previous window (different reset) and the one from 3 hours ago
        // don't count; with only one left, there's no pace.
        let rs = [r(-200, 0.9, Some(300)), r(0, 0.95, Some(0)), r(60, 0.2, Some(300))];
        assert_eq!(rate(&rs), None);
        assert_eq!(outlook(&rs), Outlook::Unknown);
    }

    #[test]
    fn edge_cases() {
        assert_eq!(outlook(&[]), Outlook::Unknown);
        assert_eq!(outlook(&[r(0, 1.0, Some(60))]), Outlook::Exhausted);
        assert_eq!(outlook(&[r(0, 0.3, Some(60)), r(10, 0.3, Some(60))]), Outlook::Unknown, "idle");
        assert!(matches!(outlook(&[r(0, 0.5, None), r(10, 0.6, None)]), Outlook::OnTrack { out_at: Some(_) }));
    }

    // ---- window_view ----

    /// 11:39 on 23 Oct 2026, UTC.
    fn at(h: i64, m: i64) -> i64 {
        1_792_713_600_000 + (h * 60 + m) * MIN
    }

    fn input(used: f64, reset: i64, len: i64) -> WindowInput<'static> {
        WindowInput { id: "5h", label: "5-hour", abs: None, used, resets_at: Some(reset), length_ms: Some(len), money: None }
    }

    fn hist(points: &[(i64, f64)], reset: i64) -> Vec<WindowReading> {
        points.iter().map(|&(t, u)| WindowReading { window: "5h".into(), used: u, limit: None, resets_at: Some(reset), at: t }).collect()
    }

    #[test]
    fn design_crit_out_at() {
        // 78% at 11:39, resetting at 14:20; the last 30 minutes ran at 22% per 86 minutes, so it's
        // out at 13:05, 1h 15m before the reset.
        let (now, reset) = (at(11, 39), at(14, 20));
        let slope = 0.22 / (86.0 * MIN as f64);
        let pts: Vec<(i64, f64)> = (0..=6).map(|i| (now - (30 - i * 5) * MIN, 0.78 - slope * ((30 - i * 5) * MIN) as f64)).collect();
        let v = window_view_in(&input(0.78, reset, 300 * MIN), &hist(&pts, reset), now, &Thresholds::default(), &Utc);
        assert_eq!(v.status, Status::Crit);
        assert_eq!(v.forecast, "Out at 13:05, 1h 15m before reset");
        assert!((v.out_at.unwrap() - at(13, 5)).abs() < MIN);
        assert!((v.pace.unwrap() - 46.33).abs() < 0.1, "{:?}", v.pace);
        assert!(v.proj.unwrap() > 100.0);
    }

    #[test]
    fn flat_or_barely_moving_never_runs_out() {
        // The reading behind "106751991167d 7h" in the menubar: Claude weekly flat at 4% for the
        // last hour, three days from its reset, at real epoch times.
        let (now, reset) = (1_792_807_612_967, 1_793_098_799_531);
        let week = 7 * 24 * 60 * MIN;
        let th = Thresholds::default();
        let flat: Vec<(i64, f64)> = (0..=12).map(|i| (now - (60 - i * 5) * MIN, 0.04)).collect();
        let v = window_view_in(&input(0.04, reset, week), &hist(&flat, reset), now, &th, &Utc);
        assert_eq!((v.status, v.out_at, v.forecast.as_str()), (Status::Ok, None, "On track"));
        // A rate so small its run-out time doesn't fit in an i64: still on track, not wrapped.
        let creep: Vec<(i64, f64)> = (0..=12).map(|i| (now - (60 - i * 5) * MIN, 0.04 + i as f64 * 1e-18)).collect();
        let v = window_view_in(&input(0.04, reset, week), &hist(&creep, reset), now, &th, &Utc);
        assert_eq!((v.status, v.out_at), (Status::Ok, None));
        let acct = AccountView { name: "Claude".into(), windows: vec![v], ..Default::default() };
        assert_eq!(summary(&[acct], now).unwrap().label, "3d 8h", "time to the reset");
        // And the reading-based outlook saturates rather than overflowing.
        let rs: Vec<WindowReading> =
            creep.iter().map(|&(t, u)| WindowReading { window: "w".into(), used: u, limit: None, resets_at: None, at: t }).collect();
        assert!(matches!(outlook(&rs), Outlook::OnTrack { out_at: Some(o) } if o > now) || outlook(&rs) == Outlook::Unknown);
    }

    #[test]
    fn design_tight_and_on_track() {
        let now = at(11, 39);
        let reset = now + (2 * 24 * 60 + 21 * 60) * MIN;
        let week = 7 * 24 * 60 * MIN;
        // Weekly Opus: 83% and slowly climbing: ~97% at reset.
        let slope = 0.14 / (reset - now) as f64;
        let pts: Vec<(i64, f64)> = (0..4).map(|i| (now - (60 - i * 20) * MIN, 0.83 - slope * ((60 - i * 20) * MIN) as f64)).collect();
        let v = window_view_in(&input(0.83, reset, week), &hist(&pts, reset), now, &Thresholds::default(), &Utc);
        assert_eq!((v.status, v.forecast.as_str()), (Status::Warn, "Tight · ~97% at reset"));
        // Weekly: 64% with the pace tick at 59% and no recent history: 108% at reset by the
        // average pace, so it's tight too; at 40% it's on track.
        let v = window_view_in(&input(0.40, reset, week), &[], now, &Thresholds::default(), &Utc);
        assert_eq!((v.status, v.forecast.as_str()), (Status::Ok, "On track"));
        assert!((v.proj.unwrap() - 40.0 / v.pace.unwrap() * 100.0).abs() < 0.2);
        assert_eq!(v.out_at, None);
    }

    #[test]
    fn thresholds_and_edges() {
        let now = at(11, 39);
        let reset = at(14, 20);
        let th = Thresholds::default();
        // Early in the window there's no projection yet; the % alone decides.
        let early = window_view_in(&input(0.05, now + 295 * MIN, 300 * MIN), &[], now, &th, &Utc);
        assert_eq!((early.proj, early.status), (None, Status::Ok));
        let hot = window_view_in(&input(0.92, now + 295 * MIN, 300 * MIN), &[], now, &th, &Utc);
        assert_eq!((hot.status, hot.forecast.as_str()), (Status::Crit, "92% used"));
        let warn = window_view_in(&input(0.80, now + 295 * MIN, 300 * MIN), &[], now, &th, &Utc);
        assert_eq!(warn.status, Status::Warn);
        let done = window_view_in(&input(1.0, reset, 300 * MIN), &[], now, &th, &Utc);
        assert_eq!((done.status, done.forecast.as_str()), (Status::Crit, "Used up until reset"));
        // Idle for the last half hour: on track whatever the average says.
        let pts = [(now - 30 * MIN, 0.5), (now - 15 * MIN, 0.5), (now, 0.5)];
        let idle = window_view_in(&input(0.5, reset, 300 * MIN), &hist(&pts, reset), now, &th, &Utc);
        assert_eq!((idle.status, idle.proj), (Status::Ok, Some(50.0)));
        // Readings from the previous window instance don't count.
        let old = hist(&[(now - 60 * MIN, 0.1), (now - 30 * MIN, 0.2), (now - 10 * MIN, 0.3)], reset - 300 * MIN);
        assert_eq!(recent_rate(&old, Some(reset), now), None);
        // Too short a span for a rate.
        let short = hist(&[(now - 4 * MIN, 0.1), (now - 2 * MIN, 0.2), (now, 0.3)], reset);
        assert_eq!(recent_rate(&short, Some(reset), now), None);
    }

    #[test]
    fn out_on_another_day() {
        let now = at(11, 39);
        let reset = now + 3 * 24 * 60 * MIN;
        let slope = 0.4 / (36.0 * 60.0 * MIN as f64);
        let pts: Vec<(i64, f64)> = (0..4).map(|i| (now - (60 - i * 20) * MIN, 0.6 - slope * ((60 - i * 20) * MIN) as f64)).collect();
        let v = window_view_in(&input(0.6, reset, 7 * 24 * 60 * MIN), &hist(&pts, reset), now, &Thresholds::default(), &Utc);
        assert_eq!(v.forecast, "Out Sat 23:39, 1d 12h before reset");
    }

    #[test]
    fn money_estimate() {
        // $184 a third of the way through the month: roughly $552 by the end; no limit, so est.
        let now = at(11, 39);
        let len = 30 * 24 * 60 * MIN;
        let w = WindowInput {
            id: "monthly",
            label: "Monthly spend",
            abs: Some("$184 this month".into()),
            used: 0.0,
            resets_at: Some(now + 2 * len / 3),
            length_ms: Some(len),
            money: Some(Money { used: 184.0, limit: None }),
        };
        let v = window_view_in(&w, &[], now, &Thresholds::default(), &Utc);
        assert_eq!((v.status, v.forecast.as_str(), v.proj), (Status::Est, "Rough estimate · ~$552", None));
        // With a $500 budget, 37% used at 72% of the month: est, ~51% at reset.
        let w =
            WindowInput { used: 0.368, money: Some(Money { used: 184.0, limit: Some(500.0) }), resets_at: Some(now + len * 28 / 100), ..w };
        let v = window_view_in(&w, &[], now, &Thresholds::default(), &Utc);
        assert_eq!(v.status, Status::Est);
        assert_eq!(v.forecast, "Rough estimate · ~$256");
    }

    #[test]
    fn words() {
        assert_eq!(duration(86 * MIN), "1h 26m");
        assert_eq!(duration((2 * 1440 + 21 * 60 + 5) * MIN), "2d 21h");
        assert_eq!(duration(45 * MIN + 30_000), "45m");
        assert_eq!(duration(20_000), "<1m");
        let now = at(11, 39);
        assert_eq!(when_in(at(14, 20), now, &Utc), "14:20");
        assert_eq!(when_in(now + 2 * 1440 * MIN, now, &Utc), "Sun 11:39");
        assert_eq!(when_in(now + 9 * 1440 * MIN, now, &Utc), "1 Nov");
    }

    #[test]
    fn summary_picks_tightest() {
        let now = at(11, 39);
        let w = |id: &str, pct: f64, status: Status, out_at: Option<i64>, resets_at: i64, proj: f64| WindowView {
            id: id.into(),
            label: id.into(),
            pct,
            status,
            out_at,
            resets_at: Some(resets_at),
            proj: Some(proj),
            ..Default::default()
        };
        let accounts = vec![
            AccountView {
                name: "ChatGPT — personal".into(),
                windows: vec![w("Codex weekly", 31.0, Status::Ok, None, now + 3000 * MIN, 79.0)],
                ..Default::default()
            },
            AccountView {
                name: "Claude — work".into(),
                windows: vec![
                    w("5-hour", 78.0, Status::Crit, Some(now + 86 * MIN), at(14, 20), 130.0),
                    w("Weekly Opus", 83.0, Status::Warn, None, now + 4000 * MIN, 97.0),
                    w("Weekly", 64.0, Status::Ok, None, now + 4000 * MIN, 79.0),
                ],
                ..Default::default()
            },
        ];
        let s = summary(&accounts, now).unwrap();
        assert_eq!((s.label.as_str(), s.window.as_str(), s.status), ("1h 26m", "Claude — work · 5-hour", Status::Crit));
        assert_eq!((s.crit, s.warn, s.ok), (1, 1, 2));
        // All on track: the time to the reset of the one with the highest projection.
        let calm = vec![AccountView {
            name: "Claude — work".into(),
            windows: vec![
                w("5-hour", 18.0, Status::Ok, None, now + 60 * MIN, 30.0),
                w("Weekly", 52.0, Status::Ok, None, now + 4140 * MIN, 88.0),
            ],
            ..Default::default()
        }];
        assert_eq!(summary(&calm, now).unwrap().label, "2d 21h");
        assert!(summary(&[], now).is_none());
    }
}
