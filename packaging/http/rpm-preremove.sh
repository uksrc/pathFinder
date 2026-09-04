#!/bin/sh
set -e

if [ "$1" -eq 0 ]; then
    # Full removal: stop and disable the service.
    systemctl stop pathfinder-http.service || true
    systemctl disable pathfinder-http.service || true
fi

systemctl daemon-reload
