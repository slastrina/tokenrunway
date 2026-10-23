#!/usr/bin/env bash
# Sets the version everywhere it lives: scripts/bump.sh 0.2.0
# Then add its section to CHANGELOG.md, commit, and push a v0.2.0 tag to release.
set -euo pipefail
v="${1:?usage: scripts/bump.sh X.Y.Z}"
cd "$(dirname "$0")/.."
perl -0pi -e "s/^version = \".*?\"/version = \"$v\"/m" Cargo.toml
for f in package.json src-tauri/tauri.conf.json; do
  node -e "const f='$f',j=require('./'+f);j.version='$v';require('fs').writeFileSync(f,JSON.stringify(j,null,2)+'\n')"
done
npm install --package-lock-only --silent
cargo update -w --quiet
grep -q "^## \[$v\]" CHANGELOG.md || echo "Now add a '## [$v] - $(date +%F)' section to CHANGELOG.md"
