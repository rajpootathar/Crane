#!/bin/bash
# Crane is ad-hoc signed (no paid Apple Developer ID), so macOS
# Gatekeeper flags it on first run and asks you to approve from
# System Settings → Privacy. Running this script once strips the
# download-quarantine attribute from /Applications/Crane.app so
# Crane opens normally from then on.
#
# Usage:
#   1. Drag Crane.app from this window into the Applications folder.
#   2. Right-click this file → Open (first time only; macOS warns
#      because *this* file is also flagged). Click "Open" at the
#      prompt.
#   3. The Terminal window will run the fix and print "Done".
#   4. Launch Crane from Applications.

set -e

APP="/Applications/Crane.app"

if [ ! -d "$APP" ]; then
    echo "Crane.app isn't in /Applications yet."
    echo "Drag Crane.app from the disk image into the Applications"
    echo "folder first, then run this script again."
    echo
    read -n 1 -s -r -p "Press any key to close."
    exit 1
fi

echo "Removing quarantine from $APP …"
xattr -cr "$APP"
echo "Done. You can now launch Crane from Applications."
echo
read -n 1 -s -r -p "Press any key to close."
