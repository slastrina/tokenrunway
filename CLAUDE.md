# Tokenrunway — notes for agents

- Design and roadmap: SPEC.md. Keep it current when a decision changes.
- Patterns come from the sibling project ../rattleboard (Tauri 2, vanilla JS UI, core crate without
  Tauri, release.yml / latest.json updater / Homebrew cask). Reuse before inventing.
- `cargo test --workspace`, `cargo fmt --all --check` and `cargo clippy --workspace --all-targets -- -D warnings`
  must pass (CI runs them on macOS, Windows, Linux x64 and arm64).
- Never store or log prompt text, tokens or cookies. Secrets go only in the OS keychain.
- The iokig.com credit stays in the popover footer, the tray menu and the tooltip (SPEC.md §6.5).
- User-facing release notes go in CHANGELOG.md under the version being released.
