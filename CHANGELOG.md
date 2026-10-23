# Changelog

Each release's section becomes its GitHub release notes and the in-app "What's new" (see
`.github/workflows/release.yml`). Write for people using the app: what they can do now, in a
sentence or two each.

## [Unreleased]

- **Rattleboard.** Send Tokenrunway's alerts to a Rattleboard split-flap board, and keep an "AI
  runway" card in its rotation with your shortest runway and every window. Connect the Rattleboard
  on this computer in one click, or a board on your network with its pair link, in Settings →
  Rattleboard.

- **GitHub Copilot, Cursor, Gemini and Grok.** Add them from Add account: Copilot from your GitHub
  CLI login or a token (premium requests and Free-tier quotas), Cursor by signing in inside
  Tokenrunway, from the Cursor app or with a pasted session cookie (monthly plan and on-demand
  spend), Gemini from the Gemini CLI login (daily quota per model), and Grok by signing in to
  grok.com (query limits) or with an xAI API key. Gemini and Grok are early: they were built
  without a test account.
- **Sign in inside Tokenrunway.** Cursor and Grok open their own sign-in page in a separate window
  with its own private storage per account; Tokenrunway keeps only the session cookie, in your
  keychain.
- An account whose subscription has ended now says so, with a link to the provider, instead of
  asking you to update Tokenrunway.

## [0.1.0] - 2026-10-23

- **First build.** Tokenrunway sits in your menubar or system tray with the iokig.com credit, and
  checks for updates and shows what's new in each release. Accounts, allowances and stats come in
  the next releases; see the roadmap in SPEC.md.
