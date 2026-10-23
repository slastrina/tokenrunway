//! "Request more" (SPEC.md §9, docs/design/RequestMore.dc.html): the message to an admin, with
//! the evidence, and the links that send it.

use crate::forecast;
use crate::view::{AccountView, AdminContact, RequestDraft, WindowView};

/// "Claude" for the sentence "I'm close to my Claude limit".
fn short_provider(provider: &str) -> &'static str {
    match provider {
        "anthropic" => "Claude",
        "openai" => "ChatGPT",
        "bedrock" => "Bedrock",
        "anthropic-api" => "Anthropic API",
        "openai-api" => "OpenAI API",
        p => super::provider_name(p),
    }
}

fn for_when(w: &WindowView) -> &'static str {
    if w.id.starts_with("weekly") {
        "this week"
    } else if w.id == "monthly" || w.id == "budget" {
        "this month"
    } else {
        "today"
    }
}

/// The draft for one window. `request_url` is set when the provider has a request flow of its
/// own for this plan.
pub fn draft(a: &AccountView, w: &WindowView, to: AdminContact, my_name: &str, request_url: Option<&str>, now: i64) -> RequestDraft {
    let mut channels: Vec<String> = Vec::new();
    if request_url.is_some() {
        channels.push("provider".into());
    }
    channels.extend(["email", "slack", "teams"].map(String::from));
    let hello = if to.name.trim().is_empty() { "Hi,".to_string() } else { format!("Hi {},", to.name.trim()) };
    let seat = if a.plan.is_empty() {
        String::new()
    } else {
        format!(" on the {} {}", a.plan, if a.provider.ends_with("-api") { "plan" } else { "seat" })
    };
    let mut lines = vec![format!("{hello} I'm close to my {} limit{seat}.", short_provider(&a.provider)), String::new()];
    let mut first = format!("• {}: {:.0}% used", w.label, w.pct);
    if let Some(abs) = &w.abs {
        first += &format!(" ({abs})");
    }
    if let Some(r) = w.resets_at {
        first += &format!(", resets {}", forecast::when(r, now));
    }
    lines.push(first);
    match (w.out_at, w.resets_at, w.proj) {
        (Some(o), Some(r), _) => {
            let at = forecast::when(o, now);
            let at = if at.contains(' ') { format!("on {at}") } else { format!("at {at}") };
            lines.push(format!("• At this pace I run out {at}, {} before reset", forecast::duration(r - o)));
        }
        (None, _, Some(p)) => lines.push(format!("• At this pace I'll be at ~{p:.0}% when it resets")),
        _ => {}
    }
    let others: Vec<&WindowView> = a.windows.iter().filter(|o| o.id != w.id).collect();
    if !others.is_empty() {
        let list = others.iter().map(|o| format!("{} {:.0}%", o.label, o.pct)).collect::<Vec<_>>().join(" · ");
        let resets: Vec<Option<i64>> = others.iter().map(|o| o.resets_at.map(|r| r / 60_000)).collect();
        let same = resets.iter().all(|r| *r == resets[0]);
        let tail = match (same, others[0].resets_at) {
            (true, Some(r)) if others.len() == 1 => format!(", resets {}", forecast::when(r, now)),
            (true, Some(r)) if others.len() == 2 => format!(", both reset {}", forecast::when(r, now)),
            (true, Some(r)) => format!(", all reset {}", forecast::when(r, now)),
            _ => String::new(),
        };
        lines.push(format!("• {list}{tail}"));
    }
    lines.push(String::new());
    lines.push(format!("Could you raise my limit or add usage for {}? I'm in the middle of [what you're working on].", for_when(w)));
    lines.push(String::new());
    lines.push(if my_name.trim().is_empty() { "Thanks".to_string() } else { format!("Thanks, {}", my_name.trim()) });
    RequestDraft {
        account_id: a.id.clone(),
        title: format!("{} · {} at {:.0}%", a.name, w.label, w.pct),
        forecast: w.forecast.clone(),
        channels,
        to,
        body: lines.join("\n"),
    }
}

/// The email subject.
pub fn subject(a: &AccountView) -> String {
    format!("More {} usage for {}", short_provider(&a.provider), a.name)
}

/// Percent-encodes for a URL query value.
pub fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>().replace('+', "%20")
}

pub fn mailto(to: &str, subject: &str, body: &str) -> String {
    format!("mailto:{}?subject={}&body={}", enc(to.trim()).replace("%40", "@"), enc(subject), enc(body))
}

/// A Teams chat with the message filled in.
pub fn teams(to: &str, body: &str) -> String {
    let to = to.trim();
    if to.starts_with("https://teams.microsoft.com/") || to.starts_with("msteams:") {
        return to.to_string();
    }
    format!("https://teams.microsoft.com/l/chat/0/0?users={}&message={}", enc(to), enc(body))
}

/// Where a Slack message goes: a link as given, else the Slack app.
pub fn slack(to: &str) -> String {
    let to = to.trim();
    if to.starts_with("https://") && to.contains("slack.com") || to.starts_with("slack://") {
        to.to_string()
    } else {
        "slack://open".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::Status;

    #[test]
    fn design_message() {
        let now = crate::now_ms();
        let reset = now + 161 * 60_000;
        let week = now + 4 * 24 * 3600 * 1000;
        let w5 = WindowView {
            id: "5h".into(),
            label: "5-hour".into(),
            abs: Some("412k / 530k tokens".into()),
            pct: 78.0,
            resets_at: Some(reset),
            out_at: Some(reset - 75 * 60_000),
            status: Status::Crit,
            forecast: "Out at 13:05, 1h 15m before reset".into(),
            ..Default::default()
        };
        let a = AccountView {
            id: "acct-1".into(),
            provider: "anthropic".into(),
            name: "Claude — work".into(),
            plan: "Enterprise".into(),
            windows: vec![
                w5.clone(),
                WindowView { id: "weekly".into(), label: "Weekly".into(), pct: 64.0, resets_at: Some(week), ..Default::default() },
                WindowView {
                    id: "weekly-opus".into(),
                    label: "Weekly Opus".into(),
                    pct: 83.0,
                    resets_at: Some(week),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let to = AdminContact { name: "Jordan".into(), ..Default::default() };
        let d = draft(&a, &w5, to, "Sam", Some("https://claude.ai/settings/usage"), now);
        assert_eq!(d.title, "Claude — work · 5-hour at 78%");
        assert_eq!(d.channels, ["provider", "email", "slack", "teams"]);
        let lines: Vec<&str> = d.body.lines().collect();
        assert_eq!(lines[0], "Hi Jordan, I'm close to my Claude limit on the Enterprise seat.");
        assert!(lines[2].starts_with("• 5-hour: 78% used (412k / 530k tokens), resets "), "{}", lines[2]);
        assert!(lines[3].ends_with(", 1h 15m before reset"), "{}", lines[3]);
        assert!(lines[4].starts_with("• Weekly 64% · Weekly Opus 83%, both reset "), "{}", lines[4]);
        assert_eq!(lines[6], "Could you raise my limit or add usage for today? I'm in the middle of [what you're working on].");
        assert_eq!(*lines.last().unwrap(), "Thanks, Sam");
    }

    #[test]
    fn links() {
        assert_eq!(mailto("jo@x.com", "Hi there", "a\nb"), "mailto:jo@x.com?subject=Hi%20there&body=a%0Ab");
        assert!(teams("jo@x.com", "hi").starts_with("https://teams.microsoft.com/l/chat/0/0?users=jo%40x.com&message=hi"));
        assert_eq!(slack("https://acme.slack.com/team/U1"), "https://acme.slack.com/team/U1");
        assert_eq!(slack("@jordan"), "slack://open");
    }
}
