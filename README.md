<p align="center">
  <img src="docs/banner.png" alt="Tokenrunway: every AI allowance in your menubar, with resets, forecasts and alerts before you run out" width="100%">
</p>

<p align="center">
  <a href="https://github.com/slastrina/tokenrunway/releases/latest"><img alt="Latest release" src="https://img.shields.io/github/v/release/slastrina/tokenrunway?color=f5a524&label=release"></a>
  <a href="https://github.com/slastrina/tokenrunway/actions/workflows/ci.yml"><img alt="CI" src="https://img.shields.io/github/actions/workflow/status/slastrina/tokenrunway/ci.yml?branch=main&label=ci"></a>
  <img alt="Platforms" src="https://img.shields.io/badge/macOS%20·%20Windows%20·%20Linux-0d1321">
  <a href="LICENSE"><img alt="MIT licence" src="https://img.shields.io/badge/licence-MIT-7cc4fa"></a>
  <a href="https://www.iokig.com/tokenrunway"><img alt="iokig.com/tokenrunway" src="https://img.shields.io/badge/site-iokig.com-f5a524"></a>
</p>

<p align="center">
  <b>How much AI runway do you have left?</b><br>
  A free, open-source menubar and system-tray app that puts all your AI subscriptions and API
  accounts in one place: how much of each allowance is left, when it resets, whether you'll run
  out first, and what your agents spend their tokens on.
</p>

<p align="center">
  <a href="#install"><b>Install</b></a> ·
  <a href="#screenshots"><b>Screenshots</b></a> ·
  <a href="#rattleboard"><b>Rattleboard</b></a> ·
  <a href="#build-from-source"><b>Build</b></a> ·
  <a href="SPEC.md"><b>Spec</b></a>
</p>

<br>

<table>
  <tr>
    <td width="50%" valign="top">
      <img src="docs/screenshots/popover-now-dark.png" alt="The Now view: shortest runway, each account's windows with reset times and forecasts">
    </td>
    <td width="50%" valign="top">
      <h3>Allowances at a glance</h3>
      Claude (Pro, Max, Team, Enterprise), ChatGPT/Codex (Plus, Pro, Business, Enterprise),
      GitHub Copilot, Cursor, Gemini (CLI), Grok/xAI, AWS Bedrock budgets and the
      Anthropic/OpenAI API orgs and OpenRouter keys. Several accounts per provider.
      <h3>Resets and forecasts</h3>
      Every 5-hour, weekly and monthly window with its reset time, a pace marker, and a
      projection of when you'll run out at your current rate. The menubar shows your shortest
      runway.
      <h3>Alerts before you run out</h3>
      Notifications at your thresholds and when a forecast says you'll hit the wall before the
      reset, plus a one-click request to your administrator for more.
    </td>
  </tr>
</table>

### Features

- **Local stats from your agents' logs**: Claude Code, Codex, opencode and others. Spend per day,
  week, month and lifetime; your most-used models and projects; and what the work was (building,
  discovery, bug fixing…). Compare harnesses side by side.
- **A full dashboard** with daily spend by model, harness or category, a burn-down for any window,
  every session, upcoming resets and a contribution-style heatmap.
- **Sign in where there's no CLI**: Cursor and Grok can be signed in to inside Tokenrunway. Only
  the session cookie is kept, in your keychain.
- **Rattleboard**: alerts and an "AI runway" card on a split-flap board.
- **Updates itself** with release notes, and installs with Homebrew.
- **Private**: no account, no server, no telemetry. Your logins stay in your OS keychain, and your
  stats stay on your computer.

## Screenshots

<p align="center">
  <img src="docs/screenshots/dashboard-dark.png" alt="The dashboard in dark mode: spend tiles, daily spend by model, burn-down, sessions, upcoming resets and a tokens-per-day heatmap">
  <br><sub>The dashboard: daily spend by model, burn-down to the next reset, every session and what's resetting next.</sub>
</p>

<table>
  <tr>
    <td align="center" width="25%"><img src="docs/screenshots/popover-stats-dark.png" alt="Stats view"><br><sub><b>Stats</b>: spend, tokens, top models and projects, kind of work</sub></td>
    <td align="center" width="25%"><img src="docs/screenshots/popover-harness-dark.png" alt="Harness view"><br><sub><b>Harness</b>: Claude Code vs Codex vs opencode vs Gemini CLI</sub></td>
    <td align="center" width="25%"><img src="docs/screenshots/popover-now-light.png" alt="Now view in light mode"><br><sub><b>Light mode</b>: follows your system theme</sub></td>
    <td align="center" width="25%"><img src="docs/screenshots/popover-request-light.png" alt="Request more view"><br><sub><b>Request more</b>: a prefilled ask to your admin</sub></td>
  </tr>
</table>

<p align="center">
  <img src="docs/screenshots/dashboard-light.png" alt="The dashboard in light mode, grouped by kind of work">
  <br><sub>Grouped by kind of work: build, bug fix, discovery and more, classified locally from your sessions.</sub>
</p>

<sub>Screenshots use sample data.</sub>

## Install

| Platform | How |
|---|---|
| macOS 11+ (Apple Silicon and Intel) | `brew install --cask slastrina/tap/tokenrunway`, or the `.dmg` from [Releases](https://github.com/slastrina/tokenrunway/releases/latest) |
| Windows 10/11 | the `-setup.exe` or `.msi` from Releases |
| Linux x86_64 / arm64 | the `.deb` or `.AppImage` from Releases (needs libayatana-appindicator; on GNOME the AppIndicator extension) |

Tokenrunway checks GitHub for new versions once a day, shows what's new, and updates in place when
you say so. Homebrew installs update with `brew upgrade --cask tokenrunway`.

## Rattleboard

Tokenrunway can show its alerts, and an "AI runway" card, on a [Rattleboard](https://github.com/slastrina/rattleboard)
split-flap board. In **Settings → Rattleboard**:

1. **Rattleboard on this computer:** click **Connect** next to *Found on this computer*. Turn on
   Rattleboard's **Settings → API server** first. Alerts go through the Rattleboard app, with its
   own key.
2. **A board on your network:** in that Rattleboard, **Settings → API server → Copy link** (turn on
   **Other devices**), paste the `rattleboard://pair?…` link and click **Connect**. Or open *or enter
   address and key*.
3. **Test** shows "TOKENRUNWAY CONNECTED" for 15 seconds. Pick which alerts go to the board, and turn
   on **Show an AI runway card in the rotation**. The card needs the board's key: on this computer,
   paste its pair link under *Add the key for the card*.

The key is kept in your OS keychain. The board only gets window names, percentages and reset
times. It never gets prompts or costs. **Disconnect** removes the card from the board and the key
from the keychain.

## Build from source

```sh
npm ci
npm run dev        # run it
npm run build      # installers in target/release/bundle
cargo test --workspace
```

Linux needs `libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libdbus-1-dev libxdo-dev`.

Layout: `crates/tokenrunway-core` is the engine (providers, log ingest, pricing, forecasts, update
checks) with no Tauri dependency; `src-tauri` is the desktop app; `src` is its UI (plain HTML/JS).

## Releasing

1. `scripts/bump.sh 0.2.0`
2. Add `## [0.2.0] - YYYY-MM-DD` to [CHANGELOG.md](CHANGELOG.md). That section becomes the release
   notes and the in-app "What's new".
3. Commit, then `git tag v0.2.0 && git push --tags`. GitHub Actions builds and signs every
   platform, publishes the release and `latest.json` for the updater, and bumps the Homebrew cask.

## Unofficial endpoints

Some providers have no public usage API for subscriptions. For those, Tokenrunway reads what their
own CLIs already keep on your computer, or calls the endpoints their own apps call, for your own
account only. Those can change without notice. When one does, Tokenrunway keeps the last good
reading and says the source needs an update. The Gemini, Grok and xAI readers were built from
the providers' observed calls without a test account, so treat them as early.

## Licence

MIT. Not affiliated with Anthropic, OpenAI, Amazon, xAI, Google, GitHub or Cursor.

<p align="center"><sub>Made by <a href="https://www.iokig.com">iokig.com</a> · <a href="https://www.iokig.com/tokenrunway">Project page</a> · <a href="CHANGELOG.md">Changelog</a></sub></p>
