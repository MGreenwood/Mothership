# Mothership Final Stretch

A checklist of the features discussed, ordered from highest to lowest impact. Each item lists the decisions made in the design rounds.

**Current priorities:** ease of adoption comes first. Git export is out of scope for now.

**Status:** Tasks 1 (event log) and 2 (GUI timeline page) are done. Task 3 (scoped checkpoints) is next.

## 1. Rift event log (foundation)
- [x] Append-only, sequenced event log per rift that every other feature reads from (`rift_events` table, global `id` as the timeline cursor)
- [x] Record: file saved, checkpoint created, chat message, join/leave, restore, wormhole create/resolve/converge/toggle/dissolve/delete, access granted/removed
- [ ] Record: verification pass/fail (added with item 9)
- [x] Log file changes on every save (not debounced)
- [x] Retention set by the server owner (`collaboration.max_rift_events`); `-1` keeps everything
- [x] Read endpoint: `GET /api/rifts/:id/events` with `after`, `before`, `latest`, `limit`, and `wait` (long-poll), collaborators only
- [x] Tests: `mothership-server/src/events_api_tests.rs` (HTTP, long-poll, socket hooks) and `mothership-common/tests/e2e_rift_events.rs` (client against the mock)
- [x] GUI timeline page reads the endpoint (task 2)
- [ ] Gap: concurrent events can be stored out of order. Writes are fire-and-forget, so two events written at the same moment can get ids in either order. Deferred until all tasks are complete.

## 2. GUI timeline page
- [x] A dedicated Timeline view in the GUI that shows the event log as it happens
- [x] Default view is live follow. The GUI long-polls, so the server holds the request until a new event arrives (one request per 25 s when idle). It makes no requests while the view is hidden.
- [x] History view: "Load older" pages back through the log, and filters hide event types
- [x] Clicking a file event opens that file in the editor
- [ ] Clicking a checkpoint event jumps to that checkpoint
- [ ] Clicking a chat event with a line anchor jumps to that line (needs item 6)
- [ ] Gap: push events over the rift WebSocket instead of long-polling. Deferred until all tasks are complete (see the end of this file).

## 3. Scoped checkpoints
- [ ] `checkpoint` with no paths captures the whole rift (current behavior)
- [ ] `checkpoint <paths...>` captures only the named files or folders
- [ ] Available in both the CLI and the GUI
- [ ] Gap: file deletions are captured by checkpoints and sent over live sync. Deletes are not in the sync protocol yet, so the event log can't record them either.

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
- [ ] Note: chat is currently written to both `chat_messages` (for chat history) and `rift_events`. Merge into one store before this item is done.

## 9. Optional verification on checkpoints
- [ ] Config file: `cp-config.toml` in the project root (shared with the team, decided)
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

## Added during work
- [x] Interactive shell: running `mothership` with no command opens a prompt where commands are typed without the `mothership` prefix. Ctrl+Z then Enter (or `exit`) leaves. Tokenizer tests live in `mothership-cli/src/shell.rs`.

## Not planned
- [ ] Git export (decided: none for now)
- [ ] AI-agent attribution on checkpoints (left to the company)
- [ ] Hunk-level and line-level checkpoints (per-file scoping is the starting point)

## Deferred until all tasks are complete
- [ ] Push events over the rift WebSocket instead of long-polling (item 2). Lower latency, but it needs a protocol change, and the CLI and daemon would have to ignore the new message.
- [ ] Fix the pre-existing `mothership-common` test `e2e_projects_rifts`, which imports `start_oauth_login` and `verify_oauth_token`. Neither exists in `client.rs`, on `HEAD` or now.
- [ ] Fix the pre-existing startup panic in `create_combined_router` (`mothership-server/src/main.rs`). It registers `/auth/finalize` and also merges the web UI routes, which register it again, so single-port mode panics. The tests use `create_api_router` instead.
- [ ] GUI tests. The GUI has no JavaScript test runner (`package.json` has only dev, build, and preview). Decide whether to add one, such as Vitest, for `Timeline.jsx` and the Tauri command wrappers.
- [ ] Retention task test. The prune query is tested, but the timer that runs it is not.
