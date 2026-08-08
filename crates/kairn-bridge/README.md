# kairn-bridge

Headless bridge mode from the sync spec (§3 Phase B, §15.6). Runs the shared
sync engine against a notes folder and mirrors it to a Kairn sync server on a
short interval — pulling remote changes and pushing local ones — with **no app
running**. One always-on machine (the Mac Mini) runs exactly one bridge per
vault, which is what puts a folder of notes onto the server so the phone can
join.

It shares the same engine, transport, and conflict rules as the desktop and iOS
builds, so there is no protocol drift. A genuine same-line collision keeps the
server's version as the file and preserves the local one as a
`…sync-conflict-…` copy: **never clobber, never lose typed text.**

> **Only ever run one bridge against a given folder.** Two engines writing one
> folder is the conflict-hell the spec rejects. Coexisting with Syncthing on the
> same folder is the deliberate Phase B design (exactly one bridge, writes are
> idempotent by hash), and is safe; two *bridges* are not.

## Enrolling and running

1. On the server host, mint a device token (printed once):

   ```
   ~/kairn-sync/kairn-sync-server --data-dir ~/kairn-sync enroll --name mini-bridge
   ```

2. Put the token in a file the bridge can read (keeps it out of the process
   list), then run:

   ```
   echo '<token>' > ~/.kairn-bridge/token
   chmod 600 ~/.kairn-bridge/token
   kairn-bridge --notes ~/Notes --token-file ~/.kairn-bridge/token
   ```

   Defaults: server `http://100.121.119.52:8787`, vault `default`, state DB
   `~/.kairn-bridge/<vault>.db` (kept outside the notes folder so it never
   syncs), interval 3s, device label `MINI`. `--once` runs a single cycle and
   exits (handy for a first push or a smoke test).

## Phased rollout (the safe path)

- **Phase A — prove it.** Point `--notes` at a *throwaway* test folder, not your
  real notes, and enroll the phone against the server. Edit on both sides,
  confirm zero lost lines.
- **Phase B — go live.** Point `--notes` at the real notes folder (Syncthing may
  keep managing it; the bridge is the single bridge device). Install the launchd
  agent below so it runs always.

## launchd agent (macOS, Phase B)

Template — fill in the absolute paths (`launchd` does not expand `~`), drop it at
`~/Library/LaunchAgents/com.kairn.bridge.plist`, then
`launchctl load` it:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.kairn.bridge</string>
    <key>ProgramArguments</key>
    <array>
        <string>/ABSOLUTE/PATH/TO/kairn-bridge</string>
        <string>--notes</string>
        <string>/ABSOLUTE/PATH/TO/Notes</string>
        <string>--token-file</string>
        <string>/ABSOLUTE/PATH/TO/.kairn-bridge/token</string>
    </array>
    <key>RunAtLoad</key><true/>
    <key>KeepAlive</key><true/>
    <key>StandardOutPath</key><string>/ABSOLUTE/PATH/TO/.kairn-bridge/bridge.log</string>
    <key>StandardErrorPath</key><string>/ABSOLUTE/PATH/TO/.kairn-bridge/bridge.err.log</string>
</dict>
</plist>
```
