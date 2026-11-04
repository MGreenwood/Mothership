# Shortcomings Checklist

Source: consolidated findings from `analysis/reports/code_analysis_report.md` and `analysis/reports/functional_testing_report.md`
Date: 2025-08-16

Notes: this checklist captures prioritized shortcomings, current status, acceptance criteria, and next actionable steps so we can begin fixing items and track progress.

- [ ] 1) WebSocket real-time features (bidirectional comm + broadcasting)
  - Files/area: `mothership-server/src/sync.rs`, `mothership-cli/src/sync.rs`
  - Priority: High
  - Status: Open
  - Acceptance: server and CLI support bidirectional websocket messages and broadcast updates to rift channels; functional realtime tests pass.
  - Next: implement missing features and add unit/integration tests.

- [ ] 2) Daemon & CLI packaging (binaries present and startable)
  - Files/area: `mothership-daemon`, `mothership-cli`
  - Priority: High
  - Status: Open
  - Acceptance: `mothership-daemon` and `mothership-cli` build and the functional test harness can invoke them.
  - Next: run `cargo build --release` for both crates and ensure artifacts exist in `target/release`.

- [ ] 3) Database connectivity & migrations
  - Area: DB config & `DATABASE_URL`, migrations in `migrations/`
  - Priority: High
  - Status: Open
  - Acceptance: Server starts with DB connection and migrations applied; DB-dependent tests pass.
  - Next: verify `DATABASE_URL`, run migrations, add helpful startup error messages when DB unavailable.

- [ ] 4) Authentication (OAuth endpoints + JWT handling)
  - Files/area: `mothership-server/src/auth.rs`, `oauth.rs`, config
  - Priority: High
  - Status: Open
  - Acceptance: OAuth endpoints reachable, JWTs issued/validated, oauth/jwt tests pass.
  - Next: validate env vars/config, enable dev OAuth redirect config, add integration tests.

- [ ] 5) Server-side rate limiting / loop protection
  - Files/area: `mothership-server/src/sync.rs`
  - Priority: Medium
  - Status: Open
  - Acceptance: server drops or throttles repeated identical updates from clients, and logs incidents.
  - Next: implement per-rift/file recent-change tracking + thresholds as a second safety layer.

- [ ] 6) Triage and fix `missing_auth_feature` occurrences (223 items)
  - Area: codebase-wide (see analysis report)
  - Priority: Medium
  - Status: Open
  - Acceptance: top 10 most impactful missing auth features addressed, CI shows reduced issue count.
  - Next: produce a short list of the top 10 files/locations and fix iteratively.

- [ ] 7) Improve error handling & resolve TODOs
  - Area: codebase-wide
  - Priority: Medium
  - Status: Open
  - Acceptance: critical paths return actionable errors and fewer TODOs remain in core modules.
  - Next: fix poor error handling instances (6 reported) and tackle high-impact TODOs.

- [x] 8) File-watching robustness (ignore lists + flood detection)
  - Files/area: `mothership-daemon/src/file_watcher.rs`
  - Priority: Medium
  - Status: Partially done
  - Acceptance: daemon ignores common noisy dirs (e.g., `.venv`) and suppresses flood events; functional file_watching test passes.
  - Notes: `.venv` ignore and flood detection (8 events / 5s) implemented.

- [ ] 9) CLI packaging & tests
  - Area: `mothership-cli` crate and distribution
  - Priority: Medium
  - Status: Open
  - Acceptance: CLI binary is produced and `cli_help` and `cli_binary` tests pass.
  - Next: build CLI, add basic smoke tests for `--help`.

- [ ] 10) Re-run functional tests and iterate
  - Area: analysis/functional testing
  - Priority: High
  - Status: Open
  - Acceptance: functional test suite passes or failing tests produce reproducible logs for triage.
  - Next: after items above, run the functional tests and capture logs.

---

Quick commands (optional) to run locally:

```powershell
# Build daemon and cli
cd mothership-daemon
cargo build --release
cd ..\mothership-cli
cargo build --release

# Build server
cd ..\mothership-server
cargo build

# Re-run functional tests (example harness)
# (Adjust to your test runner)
# ./scripts/run-functional-tests.sh
```

If you want, I can now:
- (A) Start implementing server-side rate limiting in `mothership-server/src/sync.rs` (small, safe change), or
- (B) Verify/attempt local builds for `mothership-daemon` and `mothership-cli` and report back.

