# Mothership Security Review

Scope: `mothership-server` (auth, OAuth, GhostID/QR auth, routes, rifts, wormholes, websocket, storage), `mothership-cli` (beam, wormhole, update, credentials), `mothership-daemon` (IPC server, sync), `mothership-common`, `mothership-gui` (Tauri config), Docker and CI config.

Method: static code review only. No exploits were run. Confidence is the reviewer's estimate that the issue is real and reachable in the default configuration.

## Findings

| # | Severity | File | Lines | Vulnerability | Confidence |
|---|----------|------|-------|---------------|------------|
| 1 | 🔴 CRITICAL | mothership-server/src/web_ui.rs | ~960–1050, ~866–930 | GhostID callback never verifies `signature`. Any caller can mark a challenge verified and receive a session cookie. | 8/10 |
| 2 | 🔴 CRITICAL (conditional) | mothership-server/src/main.rs | ~1356–1359, ~233, ~361 | Default admin secret `mothership-admin-secret-2025` and a public `/admin/create` route. | 8/10 |
| 3 | 🟠 HIGH (conditional) | mothership-server/src/main.rs | ~151–152 | Default JWT secret `mothership_dev_secret` if `JWT_SECRET` is unset. Tokens can be forged. | 8/10 |
| 4 | 🟠 HIGH | mothership-server/src/main.rs | ~1917–1975 | `restore_checkpoint` checks access to `project_id` but not that the checkpoint belongs to that project (IDOR). | 7/10 |
| 5 | 🟠 HIGH | mothership-server/src/web_ui.rs | ~727–730 (also main.rs ~697–700, web_ui.rs ~650) | Full JWT placed in a query string (`/download/authenticated?...&token=`) and temp codes logged at INFO. Tokens leak to logs, history, and proxies. | 9/10 |
| 6 | 🟠 HIGH | mothership-server/src/main.rs | ~ GET `/projects/:id`, `/projects/name/:name` | Unauthenticated reads ignore `is_private`. | 7/10 |
| 7 | 🟠 HIGH | mothership-cli/src/beam.rs | 279–280 | Beam download writes `project_path.join(&file.path)` with no `safe_relative_path` check. A malicious server can write outside the project. | 7/10 |
| 8 | 🟠 HIGH | mothership-daemon/src/daemon.rs | ~1773–1777 (also 1518, 1545) | Sync `FileChanged` handler joins peer-supplied `path` directly. A peer can write arbitrary files. The wormhole and beam paths already sanitize, so this is an inconsistency. | 8/10 |
| 9 | 🟡 MEDIUM | mothership-daemon/src/ipc_server.rs | 133–141 | Unauthenticated localhost control API with no CORS layer. Body-less POST routes (`/shutdown`, `/projects/:id/remove`, `/projects/:id/pause`) can be triggered by any web page via a simple cross-origin POST. | 7/10 |
| 10 | 🟡 MEDIUM | mothership-cli/src/update.rs | ~331, ~370–486 | Self-update downloads a binary and writes it with no checksum or signature check. The restart script runs via `cmd /C`. The temp path is predictable (`%TEMP%\mothership-downloaded.exe`), so a local attacker can race it. | 6/10 |
| 11 | 🟡 MEDIUM | mothership-server/src/main.rs | ~2045–2110 | `delete_project` has no owner check. Any member can delete (TODO at ~2081). | 6/10 |
| 12 | 🟡 MEDIUM | mothership-server/src/qr_auth.rs | 116–230 | `subject` is read from the unverified payload after the external verifier returns success. The challenge is not consumed before the external call, so a race can reuse a payload. In-memory maps are unbounded. Verifier URL defaults to plaintext HTTP on 127.0.0.1:23457. | 6/10 |
| 13 | 🟡 MEDIUM | mothership-server/src/auth.rs | JWT encode/verify | 30-day JWTs, `validate_aud = false`, no revocation. | 7/10 |
| 14 | 🟡 MEDIUM | mothership-server/src/web_ui.rs | ~541–546 | CLI install hint embeds `MOTHERSHIP_TOKEN` in `curl \| bash` and `irm \| iex` command lines. The install script has no checksum verification. | 6/10 |
| 15 | 🟡 MEDIUM | mothership-cli/src/auth.rs, config.rs | auth.rs:180, config.rs:148 | Credentials are written with `fs::write` and no restrictive file mode on Unix. | 6/10 |
| 16 | 🟡 MEDIUM | mothership-gui/tauri.conf.json | 14 | `"csp": null` disables the webview Content Security Policy. | 6/10 |
| 17 | ⚪ LOW | mothership-server/src/oauth.rs | 38–39 | Prints the first 10 characters of the Google client ID and secret at startup. | 9/10 |
| 18 | ⚪ LOW | docker-compose.dev.yml | 48 | Dev database password is `postgres`. CI uses `ms_pass` (`.github/workflows/sqlx-prepare.yml:17`). | 9/10 |
| 19 | ⚪ LOW | mothership-server/src/oauth.rs, web_ui.rs | state/temp token maps | OAuth state and temp token maps never evict expired entries (unbounded memory growth). | 7/10 |

## Checked and not flagged

- `storage.rs` `safe_relative_path` rejects `..` and absolute paths.
- `wormhole.rs` (CLI) `safe_relative` (line 371) requires all components to be `Normal`.
- `create_wormhole` checks membership and rift ownership. Base hashes are server-generated.
- The websocket handler validates the token, the user, and rift collaborator membership.
- No string-concatenated SQL in `database.rs`.
- The OAuth callback does not redirect to the client-supplied `callback_url`, so there is no open redirect.

## Recommended fixes (summary)

1. Verify the GhostID signature against the claimed public key before marking a challenge verified, and bind it to the challenge nonce.
2. Refuse to start when `JWT_SECRET` or `ADMIN_SECRET` is unset or equal to the default. Remove or gate `/admin/create`.
3. Check checkpoint-to-project ownership in `restore_checkpoint`.
4. Move tokens out of query strings and stop logging temp codes.
5. Enforce `is_private` on the unauthenticated project read routes.
6. Apply `safe_relative_path` to beam file writes (`mothership-cli/src/beam.rs:279`) and to daemon sync writes (`mothership-daemon/src/daemon.rs:1773`).
7. Require a per-session token on the daemon IPC server and reject requests that carry a non-localhost `Origin` header. Require JSON bodies on mutating routes.
8. Verify a published SHA-256 or signature before applying a self-update. Use a unique temp file name.
9. Add an owner or admin check to `delete_project`.
10. Consume QR challenges before calling the verifier. Bound the in-memory maps and evict expired entries.
11. Add expiry and audience validation to JWTs, and consider a revocation list.
12. Stop embedding tokens in install commands. Use a short-lived device code.
13. Create credential files with mode `0600` on Unix.
14. Enable a restrictive CSP in the Tauri config.
15. Remove secret-prefix logging, and rotate the dev DB password for any shared environment.

## Notes and open questions

- Whether findings 2 and 3 are live depends on whether production sets `JWT_SECRET` and `ADMIN_SECRET`.
- Whether finding 12 is exploitable depends on the blockchain verifier, which is outside this repo.
- The Dockerfile, `app.js`, `Hub.jsx`, and Tauri capabilities were not fully reviewed.
- No dynamic testing was performed. Verify findings 1, 7, and 8 with a proof-of-concept before fixing.
