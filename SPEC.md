# Tokenrunway — specification

> **How much AI runway do you have left?** Tokenrunway is a free, open-source menubar / system-tray
> app for macOS, Windows and Linux. It shows every AI subscription and API account you use in one
> place: how much allowance is left, when each one resets, whether you'll run out first, and what
> your agents actually spend their tokens on.

Status: draft v0.1 · 2026-10-23 · Owner: Samuel Lastrina · Project page: https://www.iokig.com/tokenrunway

---

## 1. Problem

People who work with AI coding agents juggle several allowances at once. A typical work laptop has
a Claude Enterprise seat, a ChatGPT (Codex) subscription and a cloud account such as AWS Bedrock,
and each one has its own rolling windows (5-hour, weekly, monthly), its own reset times and its
own dashboard. You usually find out you've run out at the worst moment. Only then does the
provider offer "ask your admin for more", when it's already too late to keep working.

On top of that, none of the providers answer the questions people actually ask:
- Which models do I use most, and what would this have cost at API prices?
- How much am I spending per day, week, month and in total?
- Which harness (Claude Code, Codex, opencode…) does the work?
- What does the AI spend its time on: building, discovery, bug fixing?

## 2. Goals and non-goals

### Goals
1. **One glance.** A menubar/tray icon with an optional short label (e.g. `62%` or `3h 10m`) for the
   most constrained allowance, and a popover with every account's windows, resets and forecasts.
2. **Semi-realtime.** Local agent logs are picked up within seconds; remote allowances are polled
   every 1–5 minutes (adaptive, see §7.3).
3. **Warn before it's too late.** Threshold and "projected to run out before reset" notifications.
4. **Ask for more before you run out.** Raise an allowance request with your administrator from the
   notification or popover (§9).
5. **Many accounts, many providers.** Several accounts per provider (work and personal), and a
   plug-in provider model so Grok, Gemini, Copilot, Cursor and others can be added.
6. **Local stats.** Model mix; spend per day/week/month/lifetime; by harness, project and work
   category; forecasts.
7. **Private by default.** Everything stays on the device. No telemetry, no Tokenrunway server.
8. **Same delivery as Rattleboard.** Tauri 2 + Rust, signed builds from GitHub Actions for all three
   OSes, in-app update checks with changelogs, Homebrew cask.
9. **The iokig.com credit is always visible**: in the popover footer, the tray menu and the
   tooltip (§6.5).

### Non-goals (v1)
- Not a proxy or gateway: Tokenrunway never sits in the request path of an agent.
- Not a team or admin dashboard. It shows *my* usage; org-wide views belong to the providers'
  admin consoles.
- No cloud sync between devices (maybe later, as an end-to-end-encrypted export; see §15).
- No mobile app.

## 3. Prior art

| Project | What it does | How Tokenrunway differs |
|---|---|---|
| ccusage (CLI) | Claude Code / Codex cost reports from local JSONL | GUI, live, many harnesses, remote allowances |
| CodexBar (macOS) | Menubar Claude/Codex limits | Cross-platform, multi-account, Bedrock, categories, admin requests |
| Provider dashboards | One provider each | One view across all of them, plus forecasts and notifications |

We use the same public facts these tools rely on (local log formats, the endpoints the official
CLIs call). We don't copy their code unless the licence allows it, and we credit them.

## 4. Concepts

| Term | Meaning |
|---|---|
| **Provider** | Who you pay: Anthropic (Claude), OpenAI (ChatGPT/Codex), AWS Bedrock, xAI (Grok), Google (Gemini), GitHub (Copilot), Cursor, OpenRouter… |
| **Account** | One login with a provider: e.g. "Claude — work (Enterprise)" and "Claude — personal (Max)". Many per provider. |
| **Plan** | The plan behind an account: Pro, Max, Team, Enterprise, Plus, Business, API pay-as-you-go… |
| **Window** | A limit that resets: `5h`, `weekly`, `weekly-opus`, `monthly`, `credits`, `budget`. Each has `used`, `limit` (absolute or %), `resets_at`. |
| **Harness** | The tool that ran the agent: Claude Code, Codex CLI, opencode, LM Studio (local models), Gemini CLI, Cline/Roo, Cursor, claude.ai/ChatGPT web… |
| **Usage event** | One model call, read from local logs: timestamp, harness, model, tokens (in/out/cache-read/cache-write/reasoning), session, project, cost estimate, category. |
| **Snapshot** | One reading of a remote window at a point in time. The history of snapshots feeds the forecasts. |
| **Category** | What the work was: Build, Bug fix, Discovery, Refactor, Test, Review, Docs, Ops, Planning, Chat. |

The two data planes are deliberately separate:
- **Allowance plane (remote):** what the provider says is left. It's authoritative for limits, resets
  and requests to admins.
- **Activity plane (local):** what the harnesses on this machine did. It's authoritative for models,
  categories, harnesses, projects and API-equivalent cost.

They meet on the account: a local event is tied to an account when the harness's credentials say
which one it used (§7.4).

## 5. Data sources

Confidence: **Official** = documented API. **Observed** = what the provider's own CLI/web app calls;
it can change without notice, so it sits behind a versioned adapter with contract tests and fails
softly. **Local** = files on disk.

### 5.1 Allowance plane

| Provider / plan | Source | Data | Auth | Confidence |
|---|---|---|---|---|
| Claude (Pro/Max/Team/Enterprise) via Claude Code login | `GET api.anthropic.com/api/oauth/usage` (what Claude Code's `/usage` calls) | 5-hour & weekly utilisation %, per-model weekly, `resets_at`, extra-usage/spend cap | Claude Code OAuth token (read from Keychain / `~/.claude/.credentials.json`, read-only, with consent) **or** our own login | Observed |
| Claude (any plan) via claude.ai | claude.ai web API (`/api/organizations/{org}/usage` and friends) | Same, plus org name, plan, and the request-more-usage flow for Enterprise | Session cookie from an in-app login window (§8) | Observed |
| ChatGPT / Codex (Plus/Pro/Business/Enterprise) | Codex rollout files: `rate_limits` on `token_count` events (primary/secondary windows, `used_percent`, `resets_at`, credits) | Window %, reset, credits | None (local) | Local |
| ChatGPT / Codex | `chatgpt.com/backend-api/…/usage` (what the Codex CLI/app calls) | Same, without needing a recent session | Token from `~/.codex/auth.json` **or** our own login | Observed |
| AWS Bedrock | CloudWatch `AWS/Bedrock` metrics (`InputTokenCount`, `OutputTokenCount`, `Invocations` by `ModelId`); Cost Explorer `GetCostAndUsage` (service = Amazon Bedrock); AWS Budgets as the "allowance"; Service Quotas for TPM/RPM | Tokens per model per hour, spend per day (Cost Explorer lags ~24 h and costs $0.01 per call, so we cache it), budget vs actual | AWS profile / SSO from `~/.aws` (read-only IAM policy in docs) | Official |
| Anthropic API / OpenAI API orgs | Admin API usage & cost reports | Tokens & cost per model/day, spend limits | Admin key (user-provided, kept in the keychain) | Official |
| OpenRouter | `GET /api/v1/key` (any key); `/api/v1/credits` (management keys only, skipped on 403) | The key's limit or most restrictive budget with its daily/weekly/monthly reset; spend today/week/month (UTC); free-model requests on the free tier; account credit balance | API key | Official |
| GitHub Copilot | `GET api.github.com/copilot_internal/user` (`Authorization: token …`, `X-GitHub-Api-Version: 2025-04-01`) | `quota_snapshots.premium_interactions` (and chat/completions when limited), `quota_reset_date`; Free: `limited_user_quotas` / `monthly_quotas`; unlimited → a note, no bar; `access_type_sku: subscription_ended` → "subscription has ended" (Broken, cause "setup") | `gh auth token`, else the Copilot editor login (`~/.config/github-copilot/apps.json`/`hosts.json`), else `GH_TOKEN`/`GITHUB_TOKEN` (method cli); or a pasted token (key) | Observed — verified Oct 2026 for an ended subscription only; quota shapes as documented, unverified |
| Cursor | `GET cursor.com/api/usage-summary` (plan `totalPercentUsed`, `used`/`limit` assumed cents, on-demand), `POST /api/dashboard/get-plan-info` (plan name), `POST /api/dashboard/get-current-period-usage` (fallback) | "Monthly plan" % and $ to the cycle end; "On-demand" spend | `WorkosCursorSessionToken` cookie: in-app login window (login), the Cursor app's `state.vscdb` `cursorAuth/accessToken` as `<user>%3A%3A<token>` (cli), or pasted (key) | Observed — verified Oct 2026 (shapes, zero usage); app login unverified |
| xAI / Grok | SuperGrok: `POST grok.com/rest/rate-limits` per `requestKind` (DEFAULT, REASONING, DEEPSEARCH). API: `GET api.x.ai/v1/api-key` (key status only; no documented spend endpoint yet) | Queries left per rolling window (reset known only when used up); API: key active/blocked, "spend not available yet" note | grok.com `sso`/`sso-rw` cookies (login window); xAI API key (key) | Observed — unverified (no account to test) |
| Google Gemini (CLI) | `POST cloudcode-pa.googleapis.com/v1internal:loadCodeAssist` (tier, project) then `:retrieveUserQuota` (per-model `remainingFraction`, `resetTime`) | "Gemini 2.5 Pro · daily" etc. | Gemini CLI login `~/.gemini/oauth_creds.json`, read-only, never refreshed | Observed — unverified (no CLI login to test) |
| Google Gemini (API projects) | Cloud Billing / quotas | API spend | gcloud ADC | Official (not built yet) |

**Bedrock in practice.** A Bedrock "allowance" is usually a budget (AWS Budgets) or a soft cap your
platform team sets, not a provider window. Tokenrunway shows month-to-date spend against the budget,
tokens per model from CloudWatch (near-realtime, ~1–5 min), and the TPM quota. Claude Code running on
Bedrock (`CLAUDE_CODE_USE_BEDROCK=1`) still writes local JSONL, so the activity plane covers it
whether or not AWS access is set up. If your laptop has no AWS console/API access, Bedrock works
**local-only**: you get spend from the logs at Bedrock prices, but no budget.

### 5.2 Activity plane (local harness logs)

| Harness | Location (defaults; overridable) | Notes |
|---|---|---|
| Claude Code | `~/.claude/projects/**/*.jsonl` (+ `CLAUDE_CONFIG_DIR`) | `message.model`, `message.usage` (input, cache_creation incl. 5m/1h split, cache_read, output, thinking), `cwd`, `gitBranch`, `sessionId`, `isSidechain` (subagents), `requestId` (dedupe) |
| Codex CLI / app | `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` (+ `CODEX_HOME`) | `token_count` events (`total_token_usage`, `last_token_usage`, cached, reasoning), `rate_limits`, `turn_context` (model, cwd) |
| opencode | `~/.local/share/opencode/opencode.db` (SQLite; older: `storage/message/**.json`) | Tokens and the cost opencode itself works out, per message; provider/model id |
| LM Studio | `~/.lmstudio/server-logs/YYYY-MM/YYYY-MM-DD.N.log` (+ `LMSTUDIO_HOME`; 0.2: `~/.cache/lm-studio/server-logs`) | llama.cpp's per-request `print_timing` (prompt eval = new input tokens, eval = output) and `release` (`n_tokens` − both = the reused prefix, a cache read); the model from `[INFO][<model id>]` tags or the `load_model` path, carried across rotated parts and days. Local, so always free (`lmstudio/<id>`, never priced) and filed under the project "LM Studio". The MLX engine logs no token counts for streamed requests, so those aren't counted. Prompt text in DEBUG lines is never kept |
| Gemini CLI | `~/.gemini/tmp/*/` chats/telemetry | Tokens per turn when telemetry is on |
| Cline / Roo Code / Kilo | VS Code `globalStorage/<ext>/tasks/*/ui_messages.json` | `api_req_started` entries carry tokens + cost |
| Cursor | none locally → allowance plane only | |
| Others (Goose, Amp, Aider, Continue…) | community parsers (§10) | |

The ingesters are **incremental and idempotent**. Each one keeps a cursor per file (path, inode,
size, mtime, byte offset or last rowid) and dedupes on a stable event key
(e.g. `requestId+messageId`). File-system watchers (`notify`) trigger re-reads, and a 60 s sweep
catches anything the watchers miss.

## 6. User experience

### 6.1 Menubar / tray
- **Icon**: a runway glyph whose fill shows the most constrained window (template image on macOS,
  so it follows light/dark). It turns amber at the warning threshold and red at critical or when
  you're projected to run out.
- **Label** (optional; macOS and Linux appindicator. Windows can't put text in the tray, so it
  goes in the tooltip): choose between *none*, *% left*, *runway* (time to run out, or to the
  reset), for the most urgent window or one pinned in Settings (`menubarWindow`), or *$ today*: API-equivalent spend since local midnight from the
  agents' logs (`$12.40`, `$318`, `$12.3k`), refreshed as the logs are read.
- **Tooltip**: `Tokenrunway · Claude work 5h: 38% left, resets 14:20 · iokig.com`.
- **Left click** opens the popover. **Right click** (or the menu on Linux) opens the native menu:
  `Tokenrunway v0.1.0 on iokig.com`, Open dashboard, Refresh now, Pause notifications ▸,
  Check for updates…, Settings…, Quit.

### 6.2 Popover (≈ 380×560, anchored to the tray icon)
Three tabs, with a footer that never goes away:

1. **Now**: one card per account, sorted by urgency.
   - Account name, plan badge, provider logo, last updated ("12 s ago" or "stale").
   - One bar per window: used %, absolute where known (`412k / 1.2M tokens`, `$184 / $500`), reset
     countdown and local time (`resets in 2h 41m · 14:20`), and forecast chip: *On track* /
     *Out at 13:05 — 1h 15m before reset* / *Out in 2 days*.
   - Actions: **Request more** (§9), Open provider page, Refresh.
2. **Stats**: today / 7 d / 30 d / lifetime switcher; spend (API-equivalent and actual where
   known), tokens, top models, top harnesses, top projects, category split; sparkline per day.
3. **Harness**: the same stats, pivoted by Claude Code / Codex / opencode / … with each harness's
   model mix and categories.

Footer (always shown, can't be turned off): `Tokenrunway v0.1.0 · iokig.com · ⚙︎`. When an update
is available it becomes `Update available: v0.2.0 →` (§11).

- **LM Studio card** (`local.rs`, polled every 15 s while `~/.lmstudio` exists): after the accounts,
  while LM Studio's server answers on 127.0.0.1 (port from `.internal/http-server-config.json`,
  default 1234; never through a proxy, 1.5 s timeout), or when it served requests today. It shows
  running or not, the loaded models from `GET /api/v0/models` (quantisation, loaded context; older
  servers answer only `/v1/models`, ids without load state), and today's requests and tokens from
  its logs, marked Free. No account or setup.

### 6.3 Dashboard window (resizable)
For deeper analysis: stacked area of tokens/cost per day by model, harness or category; a calendar
heatmap; a table of sessions (date, harness, project, model, tokens, cost, category, duration);
burn-down per window with the forecast cone; and a reset calendar showing upcoming resets across
accounts. Filters: account, harness, model, project, category, date range. Export as CSV/JSON.

### 6.4 Settings
Accounts (add/remove/re-login/rename/colour), Harness sources (detected paths, enable/disable,
custom paths), Notifications (§8.3), Rattleboard (§8.4), Menubar label, Pricing (currency, overrides), Categories
(rules, LLM classification opt-in), Updates (auto-check), Start at login, Data (retention, export,
wipe), About (version, licence, iokig.com, Ko-fi).

### 6.5 The iokig.com credit
"Visible at all times" means three places that can't be turned off: the popover footer, the first
item of the tray menu (`Tokenrunway vX on iokig.com`, which opens https://www.iokig.com/tokenrunway)
and the tray tooltip. In other words, it's in whatever opens when you click the icon, alongside the
details. The menubar itself shows only the icon (and the optional usage label in §6.1), never the
credit.

### 6.6 Command line
The app's own executable doubles as a command line: with a subcommand it prints and exits before
any window, tray icon or single-instance check starts (src-tauri/src/cli.rs; formatting in
`tokenrunway_core::cli`).
- `tokenrunway status [--json]`: the shortest runway, every account's windows (% used, reset,
  forecast) and today's spend.
- `tokenrunway statusline`: one line for a shell prompt or Claude Code's `statusLine` command,
  e.g. `▲ 1h 26m · Claude 5-hour 78% · $12.40 today`. It doesn't read stdin, so it never waits.
- `tokenrunway today [--json]`: today's spend by harness, model and project.
- Allowances come from `status.json`, which the running app writes to its data folder on every
  state broadcast (accounts, windows, summary and today's spend; no settings, keys or cookies).
  Older than 10 minutes counts as stale and says so. Spend comes from `activity.db` (WAL, so safe
  beside the app); when the app isn't keeping it current, the command line reads the new log
  lines itself first (inserts are idempotent).
- Where it lives: Homebrew links it (`binary` in the cask); the `.deb` puts it in `/usr/bin`; on
  Windows it's `tokenrunway.exe` in the install folder (it attaches to the calling console); the
  AppImage takes the same arguments.

## 7. Architecture

```
┌────────────────────────────── Tauri app (src-tauri) ──────────────────────────────┐
│ tray + popover + dashboard windows (vanilla JS, no bundler, strict CSP)          │
│ commands/events ◄──────────────► AppState                                         │
│ notifications · login windows · keychain · updater · autostart · single-instance │
└──────────────┬───────────────────────────────────────────────────────────────────┘
               │
┌──────────────▼──────────── tokenrunway-core (pure Rust, no Tauri) ──────────────┐
│ model.rs      Account, Window, Snapshot, UsageEvent, Category                    │
│ store.rs      SQLite (rusqlite, bundled): events, snapshots, cursors, rollups    │
│ ingest/       harness parsers: claude_code, codex, opencode, lmstudio, gemini…   │
│ providers/    allowance adapters: anthropic_oauth, claude_web, chatgpt, bedrock, │
│               openai_admin, anthropic_admin, openrouter, copilot, cursor, xai    │
│ pricing.rs    model price table (bundled snapshot + refresh), currency           │
│ forecast.rs   burn rate, run-out time, month-end projection                      │
│ categorise.rs rule engine (+ optional LLM pass)                                  │
│ alerts.rs     threshold / projection / reset rules, dedupe                       │
│ update.rs     GitHub release check + notes (from Rattleboard)                    │
└──────────────────────────────────────────────────────────────────────────────────┘
```

Why this split: like rattleboard-core, the core has no Tauri dependency, so it can be tested on every
OS in CI and could later back a headless `tokenrunway` CLI (`tokenrunway status --json`), and a
Rattleboard source plugin could show your runway on a split-flap board.

### 7.1 Key traits

```rust
/// An allowance source for one account.
#[async_trait]
pub trait Provider: Send + Sync {
    fn id(&self) -> &'static str;                       // "anthropic-oauth"
    fn capabilities(&self) -> Caps;                     // windows, spend, request_more, models
    async fn fetch(&self, acct: &Account, auth: &Auth) -> Result<Vec<WindowReading>, ProviderError>;
    async fn request_more(&self, acct: &Account, auth: &Auth, req: &AllowanceRequest)
        -> Result<RequestOutcome, ProviderError> { Err(ProviderError::Unsupported) }
}

/// A local log source.
pub trait Ingester: Send + Sync {
    fn harness(&self) -> Harness;
    fn discover(&self) -> Vec<SourcePath>;              // default + configured paths
    fn read(&self, src: &SourcePath, cursor: &Cursor) -> Result<(Vec<UsageEvent>, Cursor), IngestError>;
}
```

`ProviderError` separates `AuthExpired` (prompt to log in again), `RateLimited(retry_after)`,
`SchemaChanged` (mark the adapter degraded, keep the last good reading, report it) and `Network`.

### 7.2 Storage
SQLite in the app data dir (`io.github.slastrina.tokenrunway`), WAL mode.
- `accounts(id, provider, label, plan, colour, created_at, …)`, with secrets in the OS keychain only
- `snapshots(account_id, window, ts, used, limit, unit, resets_at, raw_json?)`
- `events(key PK, ts, harness, account_id?, provider, model, session_id, project, branch,
  input, output, cache_read, cache_write_5m, cache_write_1h, reasoning, cost_usd, cost_actual, turn_id)`
- `turns(id, signals, category, confidence)`: a turn's category signals as counts (never prompt text)
- `cursors(source, path, inode, offset, mtime)`
- `rollups_daily(day, harness, account_id, model, category, tokens…, cost)`, kept in step with
  `events` for fast charts
- Retention: raw events kept forever by default (they're small); snapshots thinned to hourly after
  30 days.

### 7.3 Polling
Remote polling is adaptive per account: every 60 s while a window is above 70% or a harness is
active (a local event in the last 5 min), 5 min otherwise, and 15 min when idle or on battery saver.
Claude's usage endpoint rate-limits hard per login (Claude Code on every signed-in computer uses
it too), so Claude is read every 5 min at most. On 429 we back off exponentially and honour
`Retry-After`; while the last good reading is under 30 min old the card carries on with it instead
of turning stale. "Refresh now" polls straight away (debounced to once per 15 s), but not during a
rate-limit wait. Codex readings found in local logs update the window without a remote
call.

### 7.4 Matching local events to accounts
- Claude Code: the account comes from the credentials file / `oauthAccount` in `~/.claude.json`;
  Bedrock/Vertex from env in the session; API key from its hash prefix.
- Codex: `auth.json` `account_id` / plan.
- When unknown: we attribute the event to "Unassigned (harness)", and a settings rule can map
  `project path → account`.

## 8. Accounts, auth and security

### 8.1 Adding an account
"Add account" → pick a provider → pick a method:
1. **Use my CLI login** (Claude Code, Codex, gcloud, aws): Tokenrunway reads the token the CLI
   stores, read-only, on each poll. It never writes to the file and never refreshes tokens itself,
   so it can't log the CLI out. On macOS, reading Claude Code's Keychain item shows a system prompt
   once; we explain why beforehand.
2. **Log in inside Tokenrunway**: a login window (a separate Tauri webview with its own data
   directory per account, so work and personal sessions never mix) loads the provider's real login
   page. We keep only the session cookie or token the adapter needs, in the OS keychain
   (`keyring`: Keychain, Credential Manager, Secret Service). SSO and SAML (Enterprise) work
   because it's the provider's own page. Built for Cursor and Grok (`src-tauri/src/login.rs`,
   `allowance/login.rs`): window `login-<account id>`, not in any capability; its own store per
   account (`data_store_identifier` on macOS 14+, a non-persistent store on older macOS,
   `data_directory` on Windows/Linux); the cookies are polled every second until the session
   cookie appears; closing the window or 10 minutes cancels; removing the account deletes the
   store. A re-login waits for a session different from the refused one.
3. **API / admin key**: pasted in and stored in the keychain.

Each account shows its auth state (OK / expiring / needs login). An expired login sends one quiet
notification, never a loop of prompts.

### 8.2 Security and privacy rules
- No Tokenrunway backend and no telemetry. The only outbound calls go to the providers you add and to
  GitHub (update checks, price-table refresh), and both can be turned off. A Rattleboard you
  connect (§8.4) gets window names, percentages and times, nothing else.
- Prompts and transcripts are read only to classify them locally. Only counts, model ids, paths and
  categories are stored, never prompt text (unless you turn on "keep session titles").
- Strict CSP, no remote scripts, and login webviews isolated from app IPC: they get no Tauri
  capabilities.
- Read-only IAM policy for Bedrock in the docs (`cloudwatch:GetMetricData`, `ce:GetCostAndUsage`,
  `budgets:ViewBudget`, `servicequotas:GetServiceQuota`).
- gitleaks in CI; the `.gitignore` blocks key/cert files.
- Observed endpoints are called at a human rate with Tokenrunway's honest User-Agent, and only for the
  logged-in user's own data. The README says plainly that they're unofficial and can break.

### 8.3 Notifications
Native notifications through `tauri-plugin-notification` (Windows toast, macOS UserNotifications,
libnotify). Rules, per window and editable:
- **Thresholds**: 50% (off by default), 75%, 90%, 100%.
- **Projection**: "At this pace you'll run out at 13:05, 1h 15m before it resets", sent when the
  projected run-out is earlier than the reset and within the lead time (default 60 min).
- **Reset**: "Claude work weekly just reset" (off by default; useful after running out).
- **Auth**: login expired.
- **Spend goals** (Settings → Spend goals): a daily and a monthly API-equivalent goal from the
  agents' logs; notified at 80% and 100%, once per day or month. The Now tab shows a ring for
  each, and with the *$ today* label the tray icon turns amber at 80% of the daily goal and red
  at 100%.
- **Fast sessions** (`live.rs`): a session whose last 20 minutes spent at least $5 and at least 3×
  the median 20-minute slice of the last 14 days' sessions, while it's still writing (within
  5 minutes). Without 10 slices of history, only $15 in 20 minutes counts. At most every 2 hours
  per session; also shown as a banner on the Now tab.
- **Switch to save** (`allowance/tips.rs`): a tight window gets a tip. A model's own window
  (*Weekly Opus*) tight while the account's *Weekly* isn't: use another model for routine work.
  Otherwise another seat (Claude, ChatGPT, Copilot, Cursor, Gemini, Grok; not API orgs or cloud
  budgets) with every window under 60%: move the next task there. The projection notification
  carries it, and the Now tab shows the most urgent one.
- **In-app toasts**: every notification is also sent to the open windows (`tr://toast`) and shown
  for 8 seconds, so it isn't missed when system notifications are off. Settings → Notifications →
  *Send a test*.
- Each rule fires once per window instance (the window id plus `resets_at`). Quiet hours, and
  *Pause for 1 h / 2 h / until resumed* as in Rattleboard.
- Actions where the OS supports them: **Request more**, **Open**, **Snooze**.

### 8.4 Rattleboard
Tokenrunway can put its alerts and an "AI runway" card on a [Rattleboard](../rattleboard) split-flap
board (`crates/tokenrunway-core/src/rattleboard.rs`, wired in `src-tauri/src/board.rs`; Settings →
Rattleboard).

- **Connecting.** *This computer*: the Rattleboard app's own `rattleboard --notify` CLI, which uses
  the app's key, so nothing to set up (found at the usual install paths or on PATH; detected with
  the binary plus an unauthenticated `GET /rattleboard/info` on the port in Rattleboard's
  `settings.json`, 7070 by default, answering 401). *A board on my network*: its pair link
  (`rattleboard://pair?v=2&url=…&key=…`, Rattleboard → Settings → API server → Copy link) or an
  address and key, checked with `GET /rattleboard/info` before anything is saved. The key goes only
  in the OS keychain (`rattleboard-key`), never in settings.json or the log. A real Vestaboard (404
  or 401 on `/rattleboard/info`, a layout from `/local-api/message`) gets alerts as a plain layout.
- **Alerts.** Whatever the §8.3 rules raise (once per window instance, held while paused) goes to
  the board too, by its own level, independent of the desktop switches: *Alerts* (critical,
  projected run-out, login expired), *+ Warnings*, *Everything* (also resets). Short upper-case text
  with a chip: "CLAUDE 5H OUT AT 13:05 / 1H 15M BEFORE RESET". Red for critical (sent with priority
  `alert`, so it gets through a Focus), orange for warnings, green for resets, violet for logins.
  Shown for 30 s, linked to the provider's usage page. API: `POST /rattleboard/notify`, or
  `/local-api/message` where the board lacks `notify`; CLI: `--notify --color --seconds --source
  Tokenrunway [--target]`.
- **The card** (needs the key: network mode, or this computer with its pair link): a `text` plugin
  `com.iokig.tokenrunway` installed with `POST /rattleboard/sources/plugins` and added to the
  rotation once per session if it isn't there (`GET /rattleboard/sources`, then `PUT
  …/rotation` with every other source exactly as it was and nothing else). Re-posted when its text
  changes, at most every 5 minutes, at once when the runway turns critical; a failure waits the 5
  minutes too. Turning it off (or disconnecting) deletes the plugin. The text fits the board's
  rows × cols: "AI RUNWAY 1H 26M", then one line per window, most urgent first: "{63} CLAUDE 5H 78%
  14:20" (chip, account, window, % used, reset).
- **Privacy.** Only account short names, window names, percentages and times reach the board: no
  prompts, no costs, no keys. Failures are a status line in Settings and the debug log, never a
  pop-up.

## 9. Asking for more allowance

The aim is to raise the request *before* you run out, with the evidence attached.

Three tiers, tried in order and set per account:
1. **Native**: when the provider has an in-product request flow (Claude Enterprise's "request more
   usage", ChatGPT Business/Enterprise credit requests), the adapter calls the same endpoint the web
   app uses, through the account's logged-in session. Spike S3 captures and documents the flow; it
   ships only if it works without scraping UI.
2. **Deep link**: open the provider's page that holds the request button (claude.ai settings →
   usage), already signed in through the login webview.
3. **Message**: write a message to the admin contact set for that account (email via `mailto:`,
   or a Slack/Teams deep link), filled in with:
   > I'm at 86% of my Claude Enterprise weekly allowance (resets Mon 09:00). At my current pace I'll
   > run out Thu ~15:00. Could you raise my limit by ~40% for this cycle? Current use: 61% Claude
   > Code (Build 48%, Bug fix 30%), …

The prompt to ask appears at a set threshold (default 85%) or when you're projected to run out
within the lead time. Every request is logged locally (when, which tier, what outcome) and shown on
the account card.

## 10. Analytics

### 10.1 Cost
- **API-equivalent cost** for every event: tokens × the model's price, including cache-write 5m/1h and
  cache-read multipliers, long-context tiers, and Bedrock/Vertex regional prices where they differ.
- **Actual spend** where the provider reports it (Bedrock Cost Explorer, Admin APIs, opencode's own
  cost, Cline cost).
- **Subscription value**: API-equivalent cost this cycle compared with the plan price ("your $200 Max
  plan did $1,940 of API work this month").
- Price table: a bundled JSON snapshot generated in CI from public price lists (LiteLLM's
  `model_prices_and_context_window.json` as the base, plus our overrides), refreshed weekly from the
  repo's latest release asset. Users can override it. Currency conversion is optional and uses a
  daily rate.

### 10.2 Periods
Day / week (ISO, or a configurable start day) / month / lifetime, in local time; plus "this cycle"
per window (from the previous reset to the next).

### 10.3 Forecasts (`forecast.rs`)
- **Window run-out**: for a window with use `u` at time `t` and reset `R`, the burn rate `r` is a
  blend of
  (a) the EWMA of the slope over the last 30–90 min of snapshots and local tokens, and
  (b) an hour-of-week profile learnt from history (median use per hour of week, so a quiet night
      isn't extrapolated across a workday).
  Then `t_out = t + (1-u)/r`. We show *On track* if `t_out ≥ R`, otherwise the run-out time and the
  margin. A cone (P25–P75) comes from the profile's spread. Until there are 2 weeks of history,
  only (a) is used, and the chip says "rough".
- **Spend projection**: month-to-date + Σ over the remaining days of (weekday-weighted daily
  median), shown as month-end, next-30-days and year-to-date at that run rate.
- Backtests: unit tests replay recorded snapshot series and assert the error stays within bounds.

### 10.4 Work categories (`categorise.rs`)
The unit is a **turn** (a user prompt and the agent actions that follow it until the next prompt).
Turns roll up into sessions. Each turn is scored against the categories, and its tokens and cost go
to the winner (or are split when two scores are within 15%).

| Category | Strong signals |
|---|---|
| Build | Edit/Write/apply_patch on source files; prompt verbs: add, implement, create, build; branch `feat/` |
| Bug fix | Prompt: fix, bug, error, broken, failing, crash, stack traces in the prompt; branch `fix/`; edit after a failing test run |
| Discovery | Mostly Read/Grep/Glob/LS/WebSearch/WebFetch, few or no edits; prompt: how, why, where, explain, find |
| Refactor | Edits spread over many files with near-zero net lines; prompt: refactor, rename, clean up, simplify |
| Test | Writing `*test*`/`spec` files; running test commands (`cargo test`, `pytest`, `npm test`, `go test`) |
| Review | Prompt: review, PR, diff; `gh pr` commands; read-only diff inspection |
| Docs | Edits mainly to `*.md`, docs/, comments |
| Ops | Bash: docker, kubectl, terraform, aws, gh workflow, deploy; CI files |
| Planning | Plan mode, TodoWrite-heavy turns, spec/ADR files |
| Chat | No tools at all |

- Rules are data (`categories.json`), shipped with defaults and editable in Settings.
- Optional **LLM pass** (off by default, opt-in per account): it sends a short, local, redacted turn
  summary (tool names, file extensions, the first 200 characters of the prompt) through a model you
  choose (e.g. a cheap model via your own CLI or key). The UI shows that this uses your allowance.
- Every turn keeps its confidence score; the UI shows "≈" where confidence is low.

### 10.5 Views and pivots
Any metric (tokens, cost, events, sessions, active time) by any dimension (account, provider,
harness, model, project, branch, category, hour of day/week). The harness view (Claude Code,
Codex, opencode, …) is a first-class tab, not just a filter.

The popover's Top models and Top projects rows open the dashboard narrowed to that model or
project (`open_dashboard { target }`; an open dashboard switches via `tr://dash-target`). The
dashboard's filter chips clear it. Today maps to the dashboard's 7 d.

The dashboard's account menu narrows everything, not only the allowance tiles: a subscription
login's activity is its own agent's (Claude → Claude Code, ChatGPT → Codex, Gemini → Gemini
CLI); accounts with no agent of their own (API keys, Bedrock, Copilot, Cursor…) show no activity.
A cloud model the price table doesn't know is listed under Stats as counted at $0, rather than
passing silently.

Which login a call ran under (`events.account`, an account id, never a token):
- Codex writes it into each session's `session_meta` (`creator_account_id`, the ChatGPT account
  id that `auth.json` holds), so Codex sessions are told apart all the way back; sessions read
  before 0.2.4 are filled in once from their files' first line.
- Claude Code's logs don't say (only rare `bridge-session` lines carry an account uuid), so each
  ingest pass first notes which login the source's `.claude.json` names
  (`oauthAccount.accountUuid`; `CLAUDE_CONFIG_DIR` gives each config dir its own) in a `logins`
  table of stretches (`from_ts`, `seen_ts`). A call goes to the stretch it falls in, or to the
  latest one if it's within a minute of the last check and no other login has been seen since.
  Calls before the first check, or in a gap between two logins, stay unattributed.
- Each CLI account notes the login ids it reads (`AccountRecord.logins`, learned on each good
  poll). The only account for a CLI collects every login it reads; with several, a new account
  takes the login it was added under and the others keep theirs.

The filter then: the only account for a CLI gets all of that agent's work, as before. With
several (two Claude logins), each gets the calls made under its own logins; the spend tile says
"Claude Code · since 29 Oct" when this login's first known call is inside the period, since
earlier sessions can't be told apart.

### 10.6 Insights (`insights.rs`, `get_insights { period, project }`)
- **Projects and branches**: cost, tokens, sessions and cache hit rate per project, with its top
  branches (all of them in the drill-down, which also filters the dashboard's chart, heatmap and
  sessions). The branch comes from the harness (Claude Code `gitBranch`, Codex `git.branch`);
  `HEAD` and none read as "No branch".
- **Cache tips**: hit rate = cache read / (input + cache read + cache write), counting only
  harnesses that report caching. A project with ≥ $5 or ≥ 1M tokens in the period whose rate is
  under half the usual and ≥ 20 points below gets a tip (top 3 by cost). The cause is the biggest
  share of uncached tokens among: calls after an idle gap longer than the cache lifetime (5 min, or
  1 h when the session writes 1-hour cache), calls that switched model mid-session, and sessions of
  3 calls or fewer. Under 25% for all of them: no cause is claimed.
- **Weekly recap**: last Monday–Sunday (local): spend, tokens, sessions, change from the week
  before, top project and model, biggest session. Notified once a week from Monday 09:00 when
  that week had work (`recap.rs`, remembered in recap.json); Settings → Notifications → Weekly
  recap, held by Pause.
- **Plan value**: API-equivalent spend this calendar month from the harnesses that run on a
  subscription (Claude Code → Claude, Codex → ChatGPT, Gemini CLI → Gemini; opencode and others
  can use either a subscription or a key, so they don't count), against the plan's monthly price.
  List prices are built in (Claude Pro $20, Max 5x $100, Max 20x $200; ChatGPT Plus $20, Pro $200;
  Google AI Pro/Ultra); Team, Enterprise and Business seats need one in Settings (account →
  Plan price). Several accounts of one provider are added together, since the logs don't say
  which account a session used.

## 11. Updates and changelog (same as Rattleboard)
- `tokenrunway-core::update` checks `api.github.com/repos/slastrina/tokenrunway/releases/latest` about
  30 s after launch and then daily (at most every 12 h across restarts). When there's a newer
  version, the tray menu gets **Update available: vX…** and the popover footer changes.
- **What's new** window: the notes of every release since the installed version, newest first,
  from the GitHub release bodies (the part before "Download the installer for your platform
  below.").
- **Update and restart** uses `tauri-plugin-updater` against `latest.json` (signed with the
  minisign updater key, verified before installing). Homebrew installs are pointed to `brew upgrade`,
  and `.deb` installs to the download page.
- `CHANGELOG.md` holds the notes. The release workflow copies the tag's section into the GitHub
  release body, so changelog, release page and in-app notes always match.
- ⚠️ While the repo is **private**, unauthenticated update checks and `latest.json` downloads get
  404, so updates only start working once the repo is public (or you'd need a public
  release-mirror repo).

## 12. Build, CI and release (same as Rattleboard)
- `ci.yml`: gitleaks; `cargo test --workspace` on macOS, Windows, Ubuntu 22.04 (x64 and arm64);
  `cargo clippy -D warnings`; `cargo fmt --check`.
- `release.yml` (on a `v*` tag or by hand): Tauri builds for macOS universal (signed with Developer
  ID and notarized when the secrets are set), Windows x64 (NSIS + MSI) and Linux x64/arm64 (deb +
  AppImage); then the updater-manifest job writes `latest.json`, and the Homebrew job bumps
  `slastrina/homebrew-tap` `Casks/tokenrunway.rb`.
- Secrets (same names as the other slastrina apps): `DEVELOPER_ID_CERT_P12`,
  `DEVELOPER_ID_CERT_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_APP_PASSWORD`,
  `APPLE_ORG_NAME`, `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`,
  `HOMEBREW_TAP_PAT`.
- **Homebrew**: `brew install --cask slastrina/tap/tokenrunway`. The release job fills
  `packaging/homebrew/tokenrunway.rb.template` with the version and DMG checksum and pushes it to the
  tap, and Homebrew installs see `brew upgrade` in place of the in-app installer. Like the updater,
  this needs public release assets, so it starts working once the repo is public.
- Versioning: semver, one version for the workspace, `package.json` and `tauri.conf.json`
  (`scripts/bump.sh X.Y.Z`).

## 13. Platform notes
- **macOS 11+**: `ActivationPolicy::Accessory` (no Dock icon), template tray icon, popover positioned
  under the icon, notifications need a signed app with its bundle id.
- **Windows 10/11**: tray icon with no text label (tooltip instead), popover anchored to the tray's
  corner, toasts with actions need an AUMID (the NSIS installer registers it).
- **Linux**: libayatana-appindicator. Left-click isn't reliable on every desktop, so the native menu
  has "Open Tokenrunway"; GNOME needs the AppIndicator extension; Secret Service needed for the
  keychain (we fall back to an encrypted file with a warning).

## 14. Milestones

| Milestone | Scope | Done when |
|---|---|---|
| **M0 Skeleton** (this commit) | Repo, CI, release pipeline, tray + popover shell, iokig.com credit, update check + notes, spec | CI green on all 4 runners; a hand-run release produces installers |
| **M1 Local stats** | SQLite store, Claude Code + Codex + opencode ingesters, pricing, Stats + Harness tabs, dashboard v1 | Lifetime numbers match `ccusage` within 1% on the dev machine |
| **M2 Allowances** | Accounts UI, keychain, Claude (OAuth + web), ChatGPT/Codex (local + remote), Now tab, tray label/colour | Work Claude Enterprise + ChatGPT windows within 1 point of the providers' own UI |
| **M3 Alerts + forecasts** | Snapshots history, forecast.rs, notifications, pause/quiet hours | Projection alert arrives ≥30 min before a real run-out in testing |
| **M4 Ask for more** | Tier 3 message for all accounts, tier 2 deep links, tier 1 where spike S3 succeeds | A request can be raised from a notification in ≤2 clicks |
| **M5 Bedrock + more providers** | Bedrock (CloudWatch/CE/Budgets), Anthropic/OpenAI admin APIs, OpenRouter, Copilot, Cursor, xAI API, Gemini | Each adapter has contract tests from recorded fixtures |
| **M6 Categories** | Rule engine, defaults, editor, optional LLM pass | ≥80% agreement with hand-labelled sample of 200 turns |
| **v1.0** | Docs site page on iokig.com, Homebrew, public repo | |

### Spikes (before M2/M4)
- **S1**: Claude usage endpoints with an Enterprise seat: OAuth usage vs claude.ai org usage. Which
  fields exist for Enterprise (seat-level spend caps)?
- **S2**: ChatGPT/Codex usage endpoint with a Business/Enterprise workspace; how often do local
  `rate_limits` arrive?
- **S3**: The request-more-usage flows (Claude Enterprise, ChatGPT): record the requests in devtools
  and decide between tier 1 and tier 2.
- **S4**: Bedrock at work: what read access does the laptop's AWS profile actually have?

## 15. Open questions
1. Store prompt-derived session titles (more useful session lists) or never (more private)?
2. LLM categorisation: allow it to run through the user's Claude Code/Codex CLI (`claude -p`), or
   only through an API key?
3. Later: an E2E-encrypted export/import to merge stats from several machines?
4. ~~A `tokenrunway` CLI and a Rattleboard source plugin?~~ Both done: the command line is §6.6,
   the board card is §8.4.
