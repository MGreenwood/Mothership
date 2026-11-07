# Mothership Final Stretch

A checklist of the features discussed, ordered from highest to lowest impact. Each item lists the decisions made in the design rounds.

**Current priorities:** ease of adoption comes first. Git export is out of scope for now.

**Status:** Tasks 1 through 7 are done. Task 8 (chat stored once, derived into checkpoints) is next.

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
- [x] Clicking a chat event with a line anchor jumps to that line
- [ ] Gap: push events over the rift WebSocket instead of long-polling. Deferred until all tasks are complete (see the end of this file).

## 3. Scoped checkpoints
- [x] `checkpoint` with no paths captures the whole rift (current behavior)
- [x] `checkpoint <paths...>` captures only the named files or folders (folders end in `/`)
- [x] Available in both the CLI and the GUI (the GUI takes one path per line)
- [x] Partial checkpoints link to the rift's previous checkpoint and store only the selection. Restoring one returns the full rift state, folded from the nearest whole-rift checkpoint.
- [x] The first checkpoint of a rift must be whole-rift. A partial one needs a checkpoint to build on.
- [x] A checkpoint that a partial checkpoint builds on cannot be deleted (409). Delete the later partial checkpoints first.
- [x] Checkpoint history is loaded from disk at startup, so parent links survive a server restart
- [ ] Gap: file deletions are captured by checkpoints and sent over live sync. Deletes are not in the sync protocol yet, so the event log can't record them either. Deferred until all tasks are complete.

## 4. Restore
- [x] Restore a full checkpoint (whole rift), from the CLI and the GUI
- [x] Restore only selected files or folders from a checkpoint (`mothership restore <id> src/ README.md`, or one path per line in the GUI). Nothing outside the selection is touched.
- [x] Restoring any checkpoint returns the full rift state at that point, including partial checkpoints
- [ ] Gap: a whole restore behaves differently in each client. The GUI removes local files that aren't in the checkpoint, but the CLI only overwrites. Pick one behavior and make both match. Deferred until all tasks are complete.

## 5. Diffs
- [x] Compare the current rift against a checkpoint (`mothership rift diff checkpoint:<id>` shows what changed since it; the GUI Compare panel does the same)
- [x] Compare the current rift against another rift, with real line counts (the old placeholder count is gone)
- [x] Show a full diff (`--full`, or "Show all changes" in the GUI) and a per-file view (`--file <path>`, or click a file in the GUI)
- [ ] Gap: a file deleted locally isn't shown as removed until deletions sync (same gap as item 3). Deferred until all tasks are complete.
- [ ] Gap: a rift name that exists in more than one project is refused as ambiguous (409). Pick a project-scoped form. Deferred until all tasks are complete.

## 6. Chat anchored to code
- [x] Anchor syntax: `path:line` for one line, `path/` for a folder. A plain file name with a dot (`README.md`) also works. Words without a dot are never anchors, so `and/or` is ignored.
- [x] Anchors are read from the message text every time, so they point at the live file, not a snapshot
- [x] CLI shows each anchor under its message with the line's text from the local folder, or says the file isn't here. The CLI accepts anchors as ordinary chat text.
- [x] Clicking an anchor in the GUI opens the file at that line in the GUI editor (timeline chat rows). An already-open file moves its cursor instead of reopening.
- [ ] Gap: a line anchor keeps its line number from when it was posted, so it doesn't follow lines inserted above it. Deferred until all tasks are complete.
- [ ] Gap: the GUI has no chat panel yet. Anchors are clickable only in the timeline. Deferred until all tasks are complete.

## 7. Secret scanning before sync
- [x] Scan files before they leave the machine. The daemon checks every outgoing change, including the first sync and streamed large files.
- [x] When a secret is found, hold back only that file and tell the user. The CLI asks y/N with `mothership secrets`. The GUI Secrets panel has an "Allow syncing" button.
- [x] Remember an allowance per exact secret (a hash, stored in `.mothership/allowed-secrets.json`). The file is asked about again only if the secret changes.
- [x] Built-in patterns: AWS access key IDs, GitHub and Slack tokens, private key headers, and secret values in assignments. Obvious placeholders such as `changeme` and `${TOKEN}` are ignored.
- [x] Removing the secret releases the file without an allowance
- [ ] Gap: binary files are read as text, so a secret in a binary file is not detected. Deferred until all tasks are complete.
- [ ] Gap: the daemon's gate has no automated end-to-end test. The gate logic is tested in mothership-common. Deferred until all tasks are complete.

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
