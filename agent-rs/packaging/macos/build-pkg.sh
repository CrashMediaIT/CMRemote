#!/bin/sh
# Source: CMRemote, clean-room implementation.
#
# macOS .pkg builder for the CMRemote Rust agent (slice R8). Run on a
# macOS runner with Xcode command-line tools installed and a Developer ID
# Installer certificate available in the keychain. Produces a notarized
# universal2 .pkg that installs cmremote-agent under /usr/local/bin and
# registers a LaunchDaemon under /Library/LaunchDaemons.
#
# Inputs (env):
#   CMREMOTE_VERSION   - SemVer string for the build (e.g. "0.1.0").
#   CMREMOTE_BIN_X64   - Path to a release-built x86_64 cmremote-agent.
#   CMREMOTE_BIN_ARM64 - Path to a release-built aarch64 cmremote-agent.
#   CMREMOTE_SIGN_ID   - Developer ID Installer common name (optional —
#                        omit for an unsigned local build).
#
# Outputs:
#   target/macos/cmremote-agent-${CMREMOTE_VERSION}-universal.pkg
#
# Notarization is the caller's responsibility (typically `xcrun
# notarytool submit ... --wait` followed by `xcrun stapler staple`); the
# release workflow under .github/workflows/release.yml drives that step
# when the macOS notary credentials are configured as repo secrets.

set -eu

: "${CMREMOTE_VERSION:?CMREMOTE_VERSION must be set}"
: "${CMREMOTE_BIN_X64:?CMREMOTE_BIN_X64 must be set}"
: "${CMREMOTE_BIN_ARM64:?CMREMOTE_BIN_ARM64 must be set}"

SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
WORKSPACE_ROOT=$(cd "$SCRIPT_DIR/../.." && pwd)
OUT_DIR="$WORKSPACE_ROOT/target/macos"
PAYLOAD_DIR="$OUT_DIR/payload"
SCRIPTS_DIR="$OUT_DIR/scripts"

rm -rf "$OUT_DIR"
mkdir -p "$PAYLOAD_DIR/usr/local/bin" \
         "$PAYLOAD_DIR/Library/LaunchDaemons" \
         "$PAYLOAD_DIR/etc/cmremote" \
         "$SCRIPTS_DIR"

# Stitch the per-arch binaries into a universal2 binary so one .pkg
# covers Intel and Apple Silicon endpoints.
lipo -create -output "$PAYLOAD_DIR/usr/local/bin/cmremote-agent" \
    "$CMREMOTE_BIN_X64" "$CMREMOTE_BIN_ARM64"
chmod 0755 "$PAYLOAD_DIR/usr/local/bin/cmremote-agent"

cp "$SCRIPT_DIR/ca.crashmedia.cmremote.agent.plist" \
   "$PAYLOAD_DIR/Library/LaunchDaemons/ca.crashmedia.cmremote.agent.plist"
chmod 0644 "$PAYLOAD_DIR/Library/LaunchDaemons/ca.crashmedia.cmremote.agent.plist"

cp "$SCRIPT_DIR/../systemd/agent.env.sample" \
   "$PAYLOAD_DIR/etc/cmremote/agent.env.sample"
chmod 0640 "$PAYLOAD_DIR/etc/cmremote/agent.env.sample"

cat > "$SCRIPTS_DIR/postinstall" <<'POSTINSTALL'
#!/bin/sh
set -e
# Create the unprivileged service account if it does not already exist.
if ! dscl . -read /Groups/_cmremote >/dev/null 2>&1; then
    GID=$(dscl . -list /Groups PrimaryGroupID | awk '$2 < 500 { id=$2 } END { print id+1 }')
    dscl . -create /Groups/_cmremote
    dscl . -create /Groups/_cmremote PrimaryGroupID "$GID"
fi
if ! dscl . -read /Users/_cmremote >/dev/null 2>&1; then
    UID_NEW=$(dscl . -list /Users UniqueID | awk '$2 < 500 { id=$2 } END { print id+1 }')
    GID=$(dscl . -read /Groups/_cmremote PrimaryGroupID | awk '{print $2}')
    dscl . -create /Users/_cmremote UserShell /usr/bin/false
    dscl . -create /Users/_cmremote UniqueID "$UID_NEW"
    dscl . -create /Users/_cmremote PrimaryGroupID "$GID"
    dscl . -create /Users/_cmremote NFSHomeDirectory /var/lib/cmremote
    dscl . -create /Users/_cmremote IsHidden 1
fi
mkdir -p /var/lib/cmremote /var/log/cmremote
chown -R _cmremote:_cmremote /var/lib/cmremote /var/log/cmremote /etc/cmremote
chmod 0750 /var/lib/cmremote /etc/cmremote
launchctl load -w /Library/LaunchDaemons/ca.crashmedia.cmremote.agent.plist || true
exit 0
POSTINSTALL
chmod 0755 "$SCRIPTS_DIR/postinstall"

PKG_OUT="$OUT_DIR/cmremote-agent-${CMREMOTE_VERSION}-universal.pkg"

PKGBUILD_ARGS=
if [ -n "${CMREMOTE_SIGN_ID:-}" ]; then
    PKGBUILD_ARGS="--sign \"$CMREMOTE_SIGN_ID\""
fi

# shellcheck disable=SC2086
pkgbuild --root "$PAYLOAD_DIR" \
         --identifier "ca.crashmedia.cmremote.agent" \
         --version "$CMREMOTE_VERSION" \
         --install-location "/" \
         --scripts "$SCRIPTS_DIR" \
         $PKGBUILD_ARGS \
         "$PKG_OUT"

echo "Built $PKG_OUT"
