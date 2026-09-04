#!/bin/sh
set -e

# Ensure the state directory has restrictive permissions.
chmod 700 /var/lib/pathfinder-http

systemctl daemon-reload

if [ "$1" -ge 2 ]; then
    # Upgrade: restart if already running.
    systemctl try-restart pathfinder-http.service || true
else
    # Fresh install: enable and start.
    systemctl enable pathfinder-http.service
    systemctl start pathfinder-http.service || true
fi
