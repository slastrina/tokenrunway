//! "Switch to save": when a window is tight, say where there's room.
//!
//! - A model's own window (Weekly Opus) tight while the account's general one (Weekly) isn't:
//!   use another model for routine work.
//! - Otherwise, another coding-agent account with plenty left in every window: move the next
//!   task there. API organisations and cloud budgets don't count; they're not a different seat.

use crate::view::{AccountState, AccountView, Status, Tip, WindowView};

/// Every window of an account under this, and none tight, to suggest it.
const ROOMY_PCT: f64 = 60.0;

fn tight(w: &WindowView) -> bool {
    matches!(w.status, Status::Crit | Status::Warn)
}

/// Providers whose accounts are seats you can do agent work in.
fn is_seat(provider: &str) -> bool {
    matches!(provider, "anthropic" | "openai" | "copilot" | "cursor" | "gemini" | "xai")
}

/// The account's busiest window, if every window has room.
fn room(a: &AccountView) -> Option<&WindowView> {
    if a.state != AccountState::Ok || a.windows.is_empty() || a.windows.iter().any(|w| tight(w) || w.pct >= ROOMY_PCT) {
        return None;
    }
    a.windows.iter().max_by(|x, y| x.pct.total_cmp(&y.pct))
}

fn model_tip(a: &AccountView, w: &WindowView) -> Option<String> {
    let model = w.id.strip_prefix("weekly-")?;
    let general = a.windows.iter().find(|g| g.id == "weekly").filter(|g| !tight(g))?;
    let name = w.label.strip_prefix("Weekly ").unwrap_or(model);
    let instead = match (a.provider.as_str(), model) {
        ("anthropic", "opus") => "switch to Sonnet for routine work".to_string(),
        _ => format!("use a model other than {name} for routine work"),
    };
    Some(format!("{} is at {:.0}% but Weekly is only at {:.0}%: {instead}.", w.label, w.pct, general.pct))
}

/// Tips for every tight window, most urgent account first (the order `accounts` is in).
pub fn tips(accounts: &[AccountView]) -> Vec<Tip> {
    let mut out = Vec::new();
    for a in accounts.iter().filter(|a| is_seat(&a.provider)) {
        for w in a.windows.iter().filter(|w| tight(w)) {
            let text = model_tip(a, w).or_else(|| {
                let (b, bw) = accounts
                    .iter()
                    .filter(|b| b.id != a.id && is_seat(&b.provider))
                    .filter_map(|b| room(b).map(|bw| (b, bw)))
                    .min_by(|x, y| x.1.pct.total_cmp(&y.1.pct))?;
                Some(format!("{} has room ({} at {:.0}%): move your next task there.", b.name, bw.label, bw.pct))
            });
            if let Some(text) = text {
                out.push(Tip { account_id: a.id.clone(), window_id: w.id.clone(), text });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(id: &str, label: &str, pct: f64, status: Status) -> WindowView {
        WindowView { id: id.into(), label: label.into(), pct, status, ..Default::default() }
    }

    fn acct(id: &str, provider: &str, name: &str, windows: Vec<WindowView>) -> AccountView {
        AccountView { id: id.into(), provider: provider.into(), name: name.into(), state: AccountState::Ok, windows, ..Default::default() }
    }

    #[test]
    fn model_window_before_another_account() {
        let claude = acct(
            "c",
            "anthropic",
            "Claude — work",
            vec![w("weekly", "Weekly", 64.0, Status::Ok), w("weekly-opus", "Weekly Opus", 83.0, Status::Warn)],
        );
        let codex = acct(
            "o",
            "openai",
            "ChatGPT — personal",
            vec![w("5h", "Codex 5-hour", 12.0, Status::Ok), w("weekly", "Codex weekly", 31.0, Status::Ok)],
        );
        let t = tips(&[claude.clone(), codex.clone()]);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].window_id, "weekly-opus");
        assert_eq!(t[0].text, "Weekly Opus is at 83% but Weekly is only at 64%: switch to Sonnet for routine work.");

        // The 5-hour window is the tight one: Codex has room.
        let mut c2 = claude;
        c2.windows = vec![w("5h", "5-hour", 78.0, Status::Crit), w("weekly", "Weekly", 40.0, Status::Ok)];
        let t = tips(&[c2.clone(), codex.clone()]);
        assert_eq!(t[0].text, "ChatGPT — personal has room (Codex weekly at 31%): move your next task there.");

        // No suggestion when the other account is busy too, or is only an API org.
        let mut busy = codex;
        busy.windows[1].pct = 70.0;
        assert!(tips(&[c2.clone(), busy]).is_empty());
        let api = acct("k", "openai-api", "OpenAI API", vec![w("monthly", "Monthly", 5.0, Status::Ok)]);
        assert!(tips(&[c2, api]).is_empty());
    }
}
