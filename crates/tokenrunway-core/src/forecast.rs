//! Forecasts (SPEC.md §10.3): at the pace of the recent readings, when does a window run out, and
//! is that before it resets? This is the first, rate-only part; the hour-of-week profile comes
//! once there's history to learn it from.

use crate::model::WindowReading;
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
    let out_at = last.at + ((1.0 - last.used) / r) as i64;
    match last.resets_at {
        Some(reset) if out_at < reset => Outlook::RunsOut { out_at, margin_ms: reset - out_at },
        Some(_) => Outlook::OnTrack { out_at: None },
        None => Outlook::OnTrack { out_at: Some(out_at) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
