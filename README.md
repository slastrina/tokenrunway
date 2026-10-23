# Tokenrunway

**How much AI runway do you have left?** Tokenrunway is a free, open-source menubar and system-tray
app for macOS, Windows and Linux. It shows all your AI subscriptions and API accounts in one place:
how much of each allowance is left, when it resets, whether you'll run out first, and what your
agents spend their tokens on.

- **Allowances at a glance**: Claude (Pro, Max, Team, Enterprise), ChatGPT/Codex (Plus, Pro,
  Business, Enterprise), GitHub Copilot, Cursor, Gemini (CLI), Grok/xAI, AWS Bedrock budgets and
  the Anthropic/OpenAI API orgs, with OpenRouter to come. Several accounts per provider. Cursor and
  Grok can be signed in to inside Tokenrunway; only the session cookie is kept, in your keychain.
- **Resets and forecasts**: every 5-hour, weekly and monthly window with its reset time, and a
  projection of when you'll run out at your current pace.
- **Notifications before you run out**, and a one-click request to your administrator for more.
- **Local stats from your agents' logs**: Claude Code, Codex, opencode and others. Spend per day,
  week, month and lifetime; your most-used models; and what the work was (building, discovery,
  bug fixing…). View it by harness.
- **Private**: no account, no server, no telemetry. Your logins stay in your OS keychain, and your
  stats stay on your computer.

Project page: [iokig.com/tokenrunway](https://www.iokig.com/tokenrunway) · Status: early
development, see [SPEC.md](SPEC.md) for the design and roadmap.

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
