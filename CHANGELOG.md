# Changelog

Each release's section becomes its GitHub release notes and the in-app "What's new" (see
`.github/workflows/release.yml`). Write for people using the app: what they can do now, in a
sentence or two each.

## [0.3.0] - 2026-10-29

### Added
- **This cycle's spend on the Now tab**: each allowance window shows what the work since its
  last reset would have cost at API prices ("$413 this cycle"), from the account's own agent
  (Claude Code for Claude, Codex for ChatGPT).
- **$ left in the menubar**: a new label shows the dollars left in a window: the real amount
  for budgets and credit limits, and for plan windows an estimate at API prices from this
  cycle's work ("~$590"). Settings → Show picks what the label follows: auto (the lowest of
  everything), one account's lowest window, or one window.
- **`tokenrunway guard`**: a check for scripts and agent loops. It exits quietly with 0 when every
  window is under your limit (90% by default, `--max 80` to change it) and 1 with a one-line
  reason when one isn't, so `tokenrunway guard && claude -p "…"` stops a long batch before it hits
  the wall. Narrow it with `--account` and `--window`, add `--runout` to also stop when a window
  is forecast to run out before it resets, and `--json` for the details. When the app isn't
  running it can't tell, and stops (exit 3) unless you add `--allow-unknown`.
- **Use it or lose it**: turn on Settings → Notifications → *Use it or lose it* and Tokenrunway
  reminds you when a weekly or monthly allowance (Claude, ChatGPT, Copilot and other seats)
  resets in the next 12 hours with 30% or more still unused, even at your recent pace. It goes to
  your Rattleboard too, with its own switch under Settings → Rattleboard.
- **New models priced within a day**: Tokenrunway fetches the latest price list from GitHub once a
  day, so a new model gets its price without waiting for an app update. The prices built into the
  app stay as the fallback: behind a strict firewall it quietly uses those, and Settings → Fetch
  the latest prices shows which prices are in use (turn it off to always use the built-in ones).

### Fixed
- **Weekly forecasts that cried wolf**: an hour of heavy use early in the week no longer reads as
  "Out Monday" at 2% used. The last hour's pace now counts for the next few hours; after that,
  the forecast uses your pace across the window so far, or the recent pace if you've slowed down.
- **Check for updates**: turning it off in Settings now stops the daily check (it kept checking).
  *Check now* still works.
- **Stats per login**: with two Claude (or two ChatGPT) accounts, the dashboard's account filter
  now shows each login's own sessions instead of sharing them. Codex says which account a session
  ran under, so that goes back through your history; for Claude Code, Tokenrunway notes which
  login is signed in as it reads the logs, so it counts from when you update (the spend tile says
  "since" that day). With one account per agent nothing changes.
- **Session drill-down**: click a session in the dashboard to see why it cost what it did: its
  cost over time, the tools it ran, and every step in order with its tokens and cost. The most
  expensive steps and the cache misses (a big prompt written to the cache again after a break)
  are marked. Only tool names are kept, never what they were given, and they're recorded from
  this version on.

## [0.2.3] - 2026-10-29

### Fixed
- **Codex costs showed $0**: the price list now has the current OpenAI models (GPT-5.6 Sol, Terra
  and Luna, GPT-5.5, GPT-5.4, GPT-5.3 Codex and the GPT-6 family), so Codex work is priced again,
  including what's already in your stats. If a model is ever newer than the list, Stats now says
  so instead of quietly counting it as $0.
- **The dashboard's account filter**: picking an account now narrows spend, tokens, sessions, the
  chart and the heatmap to that account's own agent (Claude Code for Claude, Codex for ChatGPT),
  not just the resets. Small amounts show cents ($0.12 rather than $0).
- **Multiple displays on macOS**: the popover opens under the menubar icon you clicked, on that
  screen, instead of always on the main one.

## [0.2.2] - 2026-10-25

### Fixed
- **"Claude asked Tokenrunway to slow down"**: Claude's usage is now read every 5 minutes at most
  (it was every minute while you were coding), which matters most with several Macs on the same
  login. A short rate limit no longer turns the card into a warning: it keeps the last reading
  until it's 30 minutes old, and Refresh waits out the limit instead of asking again.

## [0.2.1] - 2026-10-24

### Added
- **Shorter command names**: Homebrew installs `runway` and `trw` alongside `tokenrunway`, so
  `runway status` or `trw statusline` work too.
- **LM Studio on the Now tab**: when LM Studio's server is running on this computer, a card shows
  up by itself with the loaded model (quantisation and context), and today's requests and tokens,
  marked Free. Nothing to set up: it reads the port from LM Studio's settings and only ever asks
  127.0.0.1. When the server stops, the card stays for the rest of a day you used it, as *Not
  running*.

## [0.2.0] - 2026-10-24

### Added
- **OpenRouter**: add an OpenRouter API key to see its spend limit or budget with the reset, what
  it has spent today, this week and this month, and free-model requests on the free tier. A
  management key also shows your account's credit balance.
- **A `tokenrunway` command**: `tokenrunway status` lists every account's windows, resets and
  forecasts with today's spend; `tokenrunway statusline` prints one short line for your shell
  prompt or Claude Code's status line; `tokenrunway today` breaks today's spend down by harness,
  model and project. Add `--json` for scripts. Homebrew puts it on your PATH.
- **Spend goals**: set a daily and a monthly goal (API-equivalent) in Settings → Spend goals. The
  Now tab shows a ring for each, you're notified at 80% and 100%, and with the *$ today* label the
  menubar icon turns amber, then red, as you get close.
- **Fast-session warnings**: when a session spends 3× your usual pace (at least $5 in 20 minutes),
  you get a notification and a banner on the Now tab, so a runaway agent loop doesn't eat your
  window.
- **Switch to save**: when a window is tight, Tokenrunway says where there's room, like "Weekly
  Opus is at 83% but Weekly is only at 64%: switch to Sonnet" or "ChatGPT has room (Codex weekly
  at 12%)". It's on the Now tab and in the run-out notification.
- **In-app toasts**: alerts also pop up inside Tokenrunway while it's open. Settings →
  Notifications → *Send a test* shows one.
- **Pin a window to the menubar**: Settings → Menubar label → *Window* picks which window the
  *% left* or *Runway* label follows, instead of the most urgent one.
- **Copy usage summary**: the copy button on the Now tab puts a short text summary of every
  account on the clipboard, ready to paste into Slack.
- **LM Studio**: requests to LM Studio's local server now show up in Stats, Harness and the
  dashboard, with each model's input, output and reused (cached) tokens. They're local, so they
  read as *Free* and never count towards your spend. Models running on the MLX engine don't log
  token counts, so they aren't included yet.
- **Projects and branches**: the dashboard lists what each project cost, its sessions, cache hit
  rate and top git branches. Click one to see all its branches, with the charts and sessions
  narrowed to it.
- **Open the dashboard from Stats**: click a model or project under Top models / Top projects to
  see it in the dashboard.
- **Cache tips**: when a project's prompt cache works far worse than your usual, Insights says so
  and what the logs show about why: long pauses, switching models mid-session, or short sessions.
- **Weekly recap**: on Mondays, a notification with last week's spend, the change from the week
  before, your top project and biggest session. The dashboard has it as a card. Turn it off in
  Settings → Notifications.
- **Plan value**: what your Claude, ChatGPT or Gemini subscription did this month at API prices,
  like "Your $200 Max 20x plan did $3,199 of API-equivalent work this month (16×)". For Team,
  Enterprise and Business seats, add the price under the account in Settings.

### Changed
- Stats has a *Local models* section listing your LM Studio models by tokens, since they're free
  and would never make *Top models*.
- Session lengths count working time only: a pause of more than 30 minutes isn't part of the
  session, so a session you come back to days later no longer reads as days long.
- The *% left* menubar label now shows what's left (it showed what was used).
- Clicking the menubar icon opens Tokenrunway on the tab you left it on (Now, Stats or Harness),
  even after a restart.
- Stats → Today no longer shows a daily spend chart with a single bar.

## [0.1.1] - 2026-10-24

### Added
- **$ today in the menubar**: Settings → Menubar label → *$ today* shows what your agents have
  spent since midnight (API-equivalent, from their local logs), updated as they work. On Windows
  it's in the tray tooltip.
- **ChatGPT Business and Enterprise credit limits**: seats with a per-person credit cap and no
  rate limits now show a *Credit limit* window (`5,247 / 10,000 credits`) with its reset date.
  Before, the card was empty.

### Fixed
- A window whose usage wasn't moving could show as critical, with a huge number like
  `106751991167d 7h` under *Shortest runway* and in the menubar.

## [0.1.0] - 2026-10-23

The first release: how much AI runway you have left, in your menubar or system tray.

- **Every allowance in one place.** Claude (Pro, Max, Team, Enterprise) from your Claude Code
  login, ChatGPT/Codex from your Codex login, AWS Bedrock budgets from an AWS profile, Anthropic
  and OpenAI API organisations from an admin key, GitHub Copilot, Cursor, Gemini and Grok. Several
  accounts per provider. Gemini and Grok are early: they were built without a test account.
- **Resets and forecasts.** Each 5-hour, weekly and monthly window shows how much is used, where
  you'd be at an even pace, where your current pace takes you, when it resets, and whether you'll
  run out before then.
- **Warned before you run out.** Notifications at your thresholds and when your pace says you'll
  run out before the reset, with pause for an hour, two, or until you resume.
- **Ask for more before you're stuck.** Request more writes the message to your admin with the
  numbers filled in, by email, Slack or Teams.
- **Where the tokens went.** Stats from Claude Code, Codex and opencode logs on your computer:
  spend at API prices per day, week, month and lifetime, top models and projects, the kind of work
  (building, bug fixing, discovery, tests…), each harness side by side, and a dashboard window.
- **Sign in inside Tokenrunway** for Cursor and Grok: their own sign-in page in a private window;
  only the session cookie is kept, in your keychain.
- **Rattleboard.** Send alerts to a Rattleboard split-flap board and keep an "AI runway" card in
  its rotation.
- Private by default: no account, no server, no telemetry. Updates in place, with these notes.
