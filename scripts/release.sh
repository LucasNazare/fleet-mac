#!/usr/bin/env bash
# Build fleet.app + .dmg.
#   Ad-hoc signed by default (runs on this Mac).
#   Developer ID + notarization when these are set:
#     APPLE_SIGNING_IDENTITY="Developer ID Application: Name (TEAMID)"
#     APPLE_ID, APPLE_PASSWORD (app-specific), APPLE_TEAM_ID
#     — or APPLE_API_KEY, APPLE_API_ISSUER, APPLE_API_KEY_PATH
set -euo pipefail
cd "$(dirname "$0")/.."
source "$HOME/.cargo/env" 2>/dev/null || true
if [[ -n "${APPLE_SIGNING_IDENTITY:-}" ]]; then
  echo "Signing with: $APPLE_SIGNING_IDENTITY"
  export APPLE_SIGNING_IDENTITY
  if [[ -z "${APPLE_ID:-}${APPLE_API_KEY:-}" ]]; then
    echo "Note: no notarization credentials set; the app will be signed but not notarized."
  fi
else
  echo "No APPLE_SIGNING_IDENTITY: building an ad-hoc signed app (fine for this Mac)."
fi
npm run tauri build -- --bundles app,dmg
echo
ls -1 src-tauri/target/release/bundle/macos/*.app src-tauri/target/release/bundle/dmg/*.dmg
