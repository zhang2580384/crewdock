#!/bin/sh
set -eu

if [ "$(id -u)" -ne 0 ]; then
  echo "Run this installer as root." >&2
  exit 1
fi

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
UPDATE_ROOT=/opt/crewdock/update

install -d -m 0750 "$UPDATE_ROOT" "$UPDATE_ROOT/control" "$UPDATE_ROOT/logs" "$UPDATE_ROOT/docker-config" /opt/crewdock/releases
install -m 0750 "$SCRIPT_DIR/crewdock_updater.py" "$UPDATE_ROOT/crewdock_updater.py"

install -m 0644 "$SCRIPT_DIR/crewdock-update.service" /etc/systemd/system/crewdock-update.service
install -m 0644 "$SCRIPT_DIR/crewdock-update.path" /etc/systemd/system/crewdock-update.path
install -m 0644 "$SCRIPT_DIR/crewdock-update-check.service" /etc/systemd/system/crewdock-update-check.service
install -m 0644 "$SCRIPT_DIR/crewdock-update-check.timer" /etc/systemd/system/crewdock-update-check.timer

/usr/bin/python3 "$UPDATE_ROOT/crewdock_updater.py" --initialize
systemctl daemon-reload
systemctl enable --now crewdock-update.path crewdock-update-check.timer

echo "Update service installed. Releases are read from the public GitHub repository over HTTPS."
