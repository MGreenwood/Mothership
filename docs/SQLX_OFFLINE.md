SQLx offline workflow

Overview

This project uses SQLx for compile-time SQL validation. To allow developers to build quickly without a running database, we use SQLx's offline mode with prepared metadata.

What this repo provides

- `scripts/sqlx_prepare.ps1` — PowerShell helper to start a temporary Postgres container, run migrations, and generate `.sqlx` metadata for crates that use SQLx.
- GitHub Actions workflow `.github/workflows/sqlx-prepare.yml` — runs `cargo sqlx prepare` on push to `master` or manually, and uploads `.sqlx` metadata as an artifact.

Developer quick start (Windows PowerShell)

1. Prepare metadata locally (optional)

```powershell
# Ensure Docker is running
.\scripts\sqlx_prepare.ps1
```

2. Build offline

```powershell
$env:SQLX_OFFLINE = 'true'
cargo build
```

CI notes

- CI workflow `sqlx-prepare.yml` generates `.sqlx` metadata using a temporary Postgres service and uploads it as an artifact.
- You can download the artifact and place the `.sqlx` folder at the repo root to enable offline builds that still validate queries.

Keeping metadata up-to-date

- Run `scripts/sqlx_prepare.ps1` after changing migrations or SQL used by `query!` macros.
- The CI job will also refresh metadata on pushes to `master`.
