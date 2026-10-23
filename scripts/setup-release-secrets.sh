#!/usr/bin/env bash
# Sets the GitHub Actions secrets the release workflow signs and notarizes with: the same names as
# the other slastrina apps (Rattleboard), so the macOS build is a signed, notarized Developer ID app
# and the Homebrew cask is bumped on every tag.
#
#   scripts/setup-release-secrets.sh            (run it yourself: it asks for passwords)
#   scripts/setup-release-secrets.sh --cert     (only re-export the certificate)
#
# - Exports the "Developer ID Application" identity from your login keychain as a .p12 (macOS asks
#   you to allow it), protected by a fresh random password, and stores both as secrets.
# - The Apple ID and app-specific password for notarization come from the keychain item
#   "apple-notarization" when it exists; otherwise it asks (account.apple.com → Sign-In and
#   Security → App-Specific Passwords). Typed input is hidden and goes straight to `gh`.
# - Optionally a HOMEBREW_TAP_PAT: a fine-grained token with Contents read/write on
#   slastrina/homebrew-tap.
# Nothing is kept on disk: the exported files live in a private temp folder deleted at the end.
set -euo pipefail
REPO="${REPO:-slastrina/tokenrunway}"
TEAM_ID="LN23P43535"

command -v gh >/dev/null || { echo "Needs the GitHub CLI (brew install gh)"; exit 1; }
gh auth status >/dev/null 2>&1 || { echo "Run gh auth login first"; exit 1; }

identity=$(security find-identity -v -p codesigning | sed -n 's/.*"\(Developer ID Application: .*\)"/\1/p' | head -1)
[ -n "$identity" ] || { echo "No Developer ID Application identity in your keychain"; exit 1; }
echo "Signing identity: $identity"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
p12pw=$(openssl rand -base64 24)
echo "Exporting the identity (macOS will ask you to allow access to the key)…"
# `security export` can only export every identity at once, and Tauri refuses a .p12 holding more
# than the one it signs with. So: export all, then repack just the Developer ID certificate with
# its own key (matched by localKeyID), with 3DES/SHA1 so `security import` on the runner reads it.
security export -k login.keychain-db -t identities -f pkcs12 -P "$p12pw" -o "$tmp/all.p12"
/usr/bin/openssl pkcs12 -in "$tmp/all.p12" -passin "pass:$p12pw" -nodes -out "$tmp/all.pem" 2>/dev/null \
  || openssl pkcs12 -legacy -in "$tmp/all.p12" -passin "pass:$p12pw" -nodes -out "$tmp/all.pem"
python3 - "$tmp/all.pem" "$tmp/cert.pem" "$tmp/key.pem" <<'PY'
import re, sys
text = open(sys.argv[1]).read()
blocks = re.findall(r"(Bag Attributes.*?-----END [A-Z ]+-----)", text, re.S)
def kid(b):
    m = re.search(r"localKeyID: ([0-9A-F ]+)", b)
    return m and m.group(1).strip()
cert = next(b for b in blocks if "BEGIN CERTIFICATE" in b and "Developer ID Application" in b)
key = next(b for b in blocks if "PRIVATE KEY" in b and kid(b) == kid(cert))
pem = lambda b: b[b.index("-----BEGIN"):] + "\n"
open(sys.argv[2], "w").write(pem(cert))
open(sys.argv[3], "w").write(pem(key))
PY
/usr/bin/openssl pkcs12 -export -in "$tmp/cert.pem" -inkey "$tmp/key.pem" -passout "pass:$p12pw" \
  -name "$identity" -certpbe PBE-SHA1-3DES -keypbe PBE-SHA1-3DES -macalg sha1 -out "$tmp/cert.p12"

set_secret() { printf '%s' "$2" | gh secret set "$1" -R "$REPO" >/dev/null && echo "  set $1"; }

base64 < "$tmp/cert.p12" | tr -d '\n' | gh secret set DEVELOPER_ID_CERT_P12 -R "$REPO" >/dev/null && echo "  set DEVELOPER_ID_CERT_P12"
set_secret DEVELOPER_ID_CERT_PASSWORD "$p12pw"
set_secret APPLE_SIGNING_IDENTITY "$identity"
set_secret APPLE_TEAM_ID "$TEAM_ID"
set_secret APPLE_ORG_NAME "$(sed -n 's/Developer ID Application: \(.*\) (.*/\1/p' <<<"$identity")"
if [ "${1:-}" = "--cert" ]; then echo "Certificate updated."; exit 0; fi

# The notarization login, from the keychain item "apple-notarization" if it's there (account = the
# Apple ID, password = the app-specific password); otherwise asked for.
if security find-generic-password -s apple-notarization >/dev/null 2>&1; then
  apple_id=$(security find-generic-password -s apple-notarization | sed -n 's/.*"acct"<blob>="\(.*\)"/\1/p')
  app_pw=$(security find-generic-password -s apple-notarization -w)
  echo "Notarization login from the keychain (apple-notarization): $apple_id"
else
  read -r -p "Apple ID (email) for notarization: " apple_id
  read -r -s -p "App-specific password: " app_pw; echo
fi
[ -n "$apple_id" ] && set_secret APPLE_ID "$apple_id"
[ -n "$app_pw" ] && set_secret APPLE_APP_PASSWORD "$app_pw"

read -r -s -p "HOMEBREW_TAP_PAT (Enter to skip): " tap; echo
[ -n "$tap" ] && set_secret HOMEBREW_TAP_PAT "$tap"

echo "Done. Secrets now on $REPO:"
gh secret list -R "$REPO" | cut -f1
