#!/bin/sh
# Source: CMRemote, clean-room implementation.
# Postinst script shared by the cargo-deb maintainer-scripts entry. Creates
# the dedicated `cmremote` system user, takes ownership of the state and
# config directories, and enables + starts the systemd unit.
#
# Idempotent: re-running on an upgrade does NOT recreate the user or
# overwrite operator-modified ConnectionInfo.json / agent.env files.

set -e

CMREMOTE_USER=cmremote
CMREMOTE_GROUP=cmremote
CONFIG_DIR=/etc/cmremote
STATE_DIR=/var/lib/cmremote

# Create the system group and user if they don't already exist.
if ! getent group "$CMREMOTE_GROUP" >/dev/null 2>&1; then
    groupadd --system "$CMREMOTE_GROUP"
fi

if ! getent passwd "$CMREMOTE_USER" >/dev/null 2>&1; then
    useradd --system --gid "$CMREMOTE_GROUP" \
        --home-dir "$STATE_DIR" --no-create-home \
        --shell /usr/sbin/nologin \
        --comment "CMRemote endpoint agent" \
        "$CMREMOTE_USER"
fi

# Take ownership but keep the existing mode if the directory was already
# created (so an operator-customised mode survives a package upgrade).
mkdir -p "$CONFIG_DIR" "$STATE_DIR"
chown -R "$CMREMOTE_USER:$CMREMOTE_GROUP" "$CONFIG_DIR" "$STATE_DIR"
# ConnectionInfo.json contains the per-device verification token; keep
# it readable only to the agent and root (the systemd unit's
# ConfigurationDirectoryMode reinforces this).
chmod 0750 "$CONFIG_DIR" "$STATE_DIR"

if [ -d /run/systemd/system ]; then
    systemctl daemon-reload || true
    # Enable but do not start on a fresh install if the operator hasn't
    # yet placed ConnectionInfo.json — the agent will exit immediately
    # without it. Log a hint instead of churning the unit.
    systemctl enable cmremote-agent.service || true
    if [ -f "$CONFIG_DIR/ConnectionInfo.json" ]; then
        systemctl restart cmremote-agent.service || true
    else
        echo "cmremote-agent: install $CONFIG_DIR/ConnectionInfo.json then run 'systemctl start cmremote-agent'." >&2
    fi
fi

exit 0
