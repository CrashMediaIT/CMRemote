#!/bin/sh
# Source: CMRemote, clean-room implementation.
# Pre-removal script shared by the cargo-deb maintainer-scripts entry.
# Stops + disables the systemd unit before the binary is removed so we
# don't leave a half-uninstalled service in a Restart=on-failure spin.

set -e

if [ -d /run/systemd/system ]; then
    systemctl stop cmremote-agent.service || true
    systemctl disable cmremote-agent.service || true
fi

exit 0
