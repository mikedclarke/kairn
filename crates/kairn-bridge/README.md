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
   kairn-bridge --notes ~/Notes --server http://<server-host>:8787 \
       --token-file ~/.kairn-bridge/token
   ```

   `--server` is required (or `KAIRN_BRIDGE_SERVER`): there is no default, so a
   vault can never end up pointed at whichever machine an old address now
   belongs to. Other defaults: vault `default`, state DB
   `~/.kairn-bridge/<vault>.db` (kept outside the notes folder so it never
   syncs), interval 3s, device label `MINI`. `--once` runs a single cycle and
   exits (handy for a first push or a smoke test).

   Persistent errors (a revoked token, a server that has moved) back off
   exponentially to one retry every five minutes, so a broken bridge doesn't
   fill the log or hammer the server.

## Safety valves

- **The state DB is bound to its vault.** On first run it records the notes
  folder, server, and vault id; on later runs a mismatch is a hard failure
  rather than a silent re-sync. Sync state read against the wrong folder looks
  exactly like every note having been deleted.
- **One bridge per folder, enforced.** The engine takes an exclusive lock beside
  the state DB (`<vault>.db.lock`) and refuses to start if another process holds
  it — which is what happens when launchd respawns the bridge while the old one
  is still stuck in a request.
- **Mass deletes are refused.** If a cycle would push deletes for more than
  `max(10, 20%)` of the tracked files, it pushes none of them, logs loudly, and
  leaves the vault alone. Pulls and content pushes keep working. Once you have
  checked the folder really is as you left it, re-run with
  `--allow-bulk-delete` to let those deletions through.

## Phased rollout (the safe path)

- **Phase A — prove it.** Point `--notes` at a *throwaway* test folder, not your
  real notes, and enroll the phone against the server. Edit on both sides,
  confirm zero lost lines.
- **Phase B — go live.** Point `--notes` at the real notes folder (Syncthing may
  keep managing it; the bridge is the single bridge device). Install the launchd
  agent below so it runs always.

### Sharing the folder with Syncthing

Add the engine's atomic-write temp files to the folder's `.stignore` so
Syncthing never picks one up mid-rename and shuttles a half-written note to
another machine:

```
.*.kairn-tmp.*
```

(The engine already ignores those names, and `.stignore`, `.stfolder` and
`.stversions`, in the other direction.)

## launchd agent (macOS, Phase B)

> **macOS privacy permission: read this before you redeploy.** If the notes
> folder lives somewhere macOS protects (`~/Documents`, `~/Desktop`, iCloud
> Drive), the bridge needs **Full Disk Access**. A background agent cannot show
> the approval prompt, so without the grant `opendir` on the vault **blocks
> forever**: the process stays alive, logs no error, and silently syncs nothing.
>
> The grant follows the binary's code signature, so an ad-hoc (unsigned) build
> loses it on every rebuild. Sign the deployed binary with a stable identity
> once, and the grant survives future rebuilds:
>
> ```sh
> codesign --force -s "Developer ID Application: <your identity>" \
>   --identifier com.example.kairn-bridge --options runtime --timestamp \
>   ~/kairn-bridge/kairn-bridge
> ```
>
> Then add that binary under System Settings → Privacy & Security → Full Disk
> Access and restart the agent. The bridge warns in its log if a first cycle has
> not finished within 90 seconds, which is what this looks like.


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
