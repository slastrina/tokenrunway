# Changelog

Each release's section becomes its GitHub release notes and the in-app "What's new" (see
`.github/workflows/release.yml`). Write for people using the app: what they can do now, in a
sentence or two each.

## [Unreleased]

### Added
- **$ today in the menubar**: Settings → Menubar label → *$ today* shows what your agents have
  spent since midnight (API-equivalent, from their local logs), updated as they work. On Windows
  it's in the tray tooltip.

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
