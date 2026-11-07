# Mothership Final Stretch

A checklist of the features discussed, ordered from highest to lowest impact. Each item lists the decisions made in the design rounds.

## 1. Rift event log (foundation)
- [ ] Append-only, sequenced event log per rift that every other feature reads from
- [ ] Record: file saved, checkpoint created, chat message, join/leave, restore, wormhole created/merged/dissolved, access granted/removed, verification pass/fail
- [ ] Log file changes on every save (not debounced)
- [ ] Retention set by the server owner; `-1` means keep forever

## 2. GUI timeline page
- [ ] A dedicated page in the GUI that shows the event log as it happens
- [ ] Default view is live follow (auto-scrolls as events arrive)
- [ ] History view for scrolling back and filtering
- [ ] Clicking an event jumps to its file, line, or checkpoint

## 3. Scoped checkpoints
- [ ] `checkpoint` with no paths captures the whole rift (current behavior)
- [ ] `checkpoint <paths...>` captures only the named files or folders
- [ ] Available in both the CLI and the GUI

## 4. Restore
- [ ] Restore a full checkpoint (whole rift)
- [ ] Restore only selected files or folders from a checkpoint

## 5. Diffs
- [ ] Compare the current rift against a checkpoint
- [ ] Compare the current rift against another rift
- [ ] Show a full diff and a per-file view

## 6. Chat anchored to code
- [ ] Anchor syntax: `path:line` for a line, `path/` (or `path`) for a folder
- [ ] Anchors point to the live file, so they follow edits
- [ ] CLI prints and accepts the same anchors
- [ ] Clicking an anchor in the GUI opens the file at that line in the GUI editor

## 7. Secret scanning before sync
- [ ] Scan files before they leave the machine (pre-sync)
- [ ] When a secret is found, block sync and show a y/n prompt so the user can fix it or allow it
- [ ] Remember an "allow" decision per exact secret in a local file; ask again only if the secret changes

## 8. Chat stored once, derived into checkpoints
- [ ] Chat is stored once in the rift's event log
- [ ] A checkpoint shows the chat since the previous checkpoint by reference, not by copy

## 9. Optional verification on checkpoints
- [ ] Config file: `.mothership/` project file in TOML, listing commands to run
- [ ] Optional personal config as well, on the user's machine
- [ ] Runs on the local daemon, using the user's own tools
- [ ] Runs on checkpoint and can also be started manually
- [ ] Failure is labeled on the checkpoint; it never blocks saving

## 10. Git import
- [ ] `mothership import git` converts a Git repo into a Mothership rift
- [ ] Full history: each Git commit becomes a checkpoint

## 11. Chat history cap (owner setting)
- [ ] Server owner sets the maximum chat messages per rift
- [ ] `-1` means unlimited (replaces the current default of 1000 in `mothership-server/src/config.rs`)

## 12. VS Code extension
- [ ] Reuses the existing GUI pages (timeline, diffs, chat, anchors) inside VS Code
- [ ] Planned after the GUI timeline page is working

## Not planned
- [ ] Git export (decided: none for now)
- [ ] AI-agent attribution on checkpoints (left to the company)
- [ ] Hunk-level and line-level checkpoints (per-file scoping is the starting point)
