Crane — First Run
=================

Crane is ad-hoc signed (no paid Apple Developer ID yet). macOS
Gatekeeper therefore flags it as "from an unidentified developer"
on the first launch.

Quick install (30 seconds):

  1. Drag Crane.app into the Applications folder on the right.
  2. Right-click "Fix Gatekeeper.command" → Open. Click "Open" at
     the prompt. It removes the download-quarantine flag from
     Crane.app — no Privacy-settings trip needed.
  3. Launch Crane from /Applications.

Subsequent in-app updates (the toast in the top-right) don't hit
Gatekeeper at all — the updater strips quarantine for you.

If you'd rather skip the script:
  - Right-click Crane.app → Open the first time, then click Open
    at the warning. Or
  - Open a terminal and run:  xattr -cr /Applications/Crane.app

Questions: https://github.com/rajpootathar/Crane/issues
