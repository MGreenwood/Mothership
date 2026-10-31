# scripts/sqlx_prepare.ps1
# Prepare sqlx offline metadata for the repo by creating a temporary Postgres container,
# running migrations, and running `cargo sqlx prepare` in relevant crates.
param(
    [string]$DbUser = 'ms',
    [string]$DbPass = 'ms_pass',
    [string]$DbName = 'mothership_test',
    [int]$HostPort = 5433
)

function Ensure-SqlxCli {
    if (-not (Get-Command sqlx -ErrorAction SilentlyContinue)) {
        Write-Host "Installing sqlx-cli (postgres, migrate features)..."
        cargo install sqlx-cli --no-default-features --features postgres,migrate
    } else {
        Write-Host "sqlx found: $(sqlx --version)"
    }
}

function Start-Postgres {
    $name = 'mothership-sqlx'
    # Remove any existing container with same name
    if ((docker ps -a --filter "name=$name" --format '{{.Names}}') -match $name) {
        docker rm -f $name | Out-Null
    }
    Write-Host "Starting Postgres container $name on port $HostPort..."
    docker run -d --name $name -e POSTGRES_USER=$DbUser -e POSTGRES_PASSWORD=$DbPass -e POSTGRES_DB=$DbName -p ${HostPort}:5433 postgres:15 | Out-Null

    Write-Host "Waiting for Postgres to become ready..."
    while ($true) {
        try {
            docker exec $name pg_isready -U $DbUser > $null 2>&1
            if ($LASTEXITCODE -eq 0) { break }
        } catch {
            # ignore and retry
        }
        Start-Sleep -Seconds 1
    }
    Write-Host "Postgres is ready."
    return $name
}

# Main
try {
    Ensure-SqlxCli
    $container = Start-Postgres

    $env:DATABASE_URL = "postgres://${DbUser}:${DbPass}@localhost:${HostPort}/${DbName}"
    Write-Host "Using DATABASE_URL=$env:DATABASE_URL"

    # Prepare mothership-server
    if (Test-Path "mothership-server") {
        Push-Location mothership-server
        Write-Host "Running migrations and sqlx prepare in mothership-server..."
    cargo sqlx migrate run
    cargo sqlx prepare
        Pop-Location
    } else {
        Write-Host "mothership-server directory not found; skipping."
    }

    # Prepare mothership-common
    if (Test-Path "mothership-common") {
        Push-Location mothership-common
        Write-Host "Running sqlx prepare in mothership-common..."
    cargo sqlx prepare
        Pop-Location
    } else {
        Write-Host "mothership-common directory not found; skipping."
    }

    Write-Host "Preparation finished."
} finally {
    if ($container) {
        Write-Host "Stopping and removing Postgres container $container..."
        docker rm -f $container | Out-Null
    }
}
