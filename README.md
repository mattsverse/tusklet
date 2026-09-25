# HandyPOS

A native desktop workspace for local PostgreSQL Docker containers. Built with **Rust, GPUI, and SQLite**, following [the original mockup](handypos-mockup.png).

## Features

- Named projects containing multiple independently managed databases.
- Any official `postgres` Docker Hub image tag, including Alpine variants and prereleases.
- Local images are always used first. Offline mode prevents image downloads.
- Start/stop controls, health status, and PostgreSQL logs with pause/resume and copy.
- Custom PostgreSQL settings, such as `-c wal_level=logical -c max_replication_slots=10`.
- Automatic available ports or explicit reserved ports, bound only to `127.0.0.1`.
- Persistent Docker volumes and SQLite configuration across app and container restarts.
- Custom-format `pg_dump` exports and custom-format/plain SQL restores, using the container's PostgreSQL tools.
- Copy a connection URL with a randomly generated password for each database.

## Run

Install Rust (edition 2024; the current stable toolchain is recommended) and Docker Desktop, or Docker Engine with the `docker` CLI. Start Docker and use a **local Docker context**. Windows requires Docker's Linux-container mode.

```sh
cargo run --locked
```

Create a project, add a database, then click **Start database**. The first start downloads the chosen image if needed. A cached image starts without internet access. If Docker is unavailable, saved projects remain visible and can still be created or renamed.

For an isolated workspace, useful during development:

```sh
cargo run --locked -- --data-dir /tmp/handypos-dev
```

macOS needs the Xcode command-line tools. Windows needs the MSVC build tools and Windows SDK. Linux needs a working GPU/Vulkan driver, X11 or Wayland, and the following build packages on Ubuntu 24.04:

```sh
sudo apt-get install clang cmake pkg-config libasound2-dev libfontconfig1-dev \
  libfreetype6-dev libssl-dev libvulkan-dev libwayland-dev libx11-xcb-dev \
  libxcb1-dev libxcb-composite0-dev libxcb-damage0-dev libxcb-randr0-dev \
  libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev \
  libxkbcommon-x11-dev libzstd-dev
```

## Storage and behavior

Each database gets a container named `handypos-<uuid>` and a volume named `handypos-<uuid>-data`. PostgreSQL data lives in `/var/lib/postgresql/handypos` within the mounted volume. Explicit `PGDATA` supports the different volume defaults in PostgreSQL 17 and 18. [Official image documentation](https://hub.docker.com/_/postgres)

- **Stop** preserves data. Closing HandyPOS also leaves databases running.
- **Settings** can change the display name, port, and PostgreSQL arguments while stopped. HandyPOS recreates the container with the same volume at the next start.
- Image tag, initial database, and user are immutable after creation. To change versions, create a database and migrate with backup/restore.
- Assigned ports stay reserved within HandyPOS, including automatically chosen ports and stopped databases. Other applications can still take a stopped container's port; start reports a conflict instead of silently changing it.
- **Remove database** requires confirmation and a stopped container. It removes the app entry and container but **retains the named data volume**. The confirmation shows the volume name so it can be recovered or removed manually in Docker. Empty projects can be removed separately.
- Log following refreshes a bounded 400-line tail every two seconds. Docker rotates log files at 10 MB, retaining three files. Operations run on a worker thread.

Configuration and generated passwords are saved in `handypos.sqlite3` in the operating system's local application-data directory (`~/Library/Application Support/dev.handypos.HandyPOS` on macOS, `$XDG_DATA_HOME/handypos` or `~/.local/share/handypos` on Linux, `%LOCALAPPDATA%\handypos\HandyPOS\data` on Windows). `--data-dir` overrides this location. Passwords are stored locally, not encrypted; the SQLite file is restricted to the current user on Unix. Docker also retains the container environment. This is a **local development tool**, not a production secret manager. Back up both the SQLite file and database dumps if migrating machines.

## Backups

**Export backup** writes a PostgreSQL custom-format `.dump` through a temporary file, publishing it only after success. Existing files are never overwritten. No host PostgreSQL installation is needed.

**Restore backup** accepts `pg_dump` custom-format files or plain SQL. The target must be running; an empty database is recommended. Restores run in a single transaction with stop-on-error, and do not drop existing objects. Conflicts roll back ordinary transactional SQL. Plain scripts that manage their own transactions or require commands outside a transaction are not supported. Only restore trusted backups: PostgreSQL dumps can contain executable SQL. Cluster-wide `pg_dumpall` archives, directory archives, and gzip files are not supported.

## Development and checks

```sh
cargo fmt --all --check
cargo clippy --locked --all-targets --features ui-tests -- -D warnings -D clippy::pedantic -D clippy::perf -D clippy::suspicious
cargo test --locked --no-default-features
cargo test --locked --features ui-tests --bin handypos
cargo build --locked
```

The UI tests render the database detail screen at multiple window sizes without opening a desktop window or connecting to Docker. The backend can be built and tested without desktop dependencies. Opt-in Docker integration tests create uniquely named containers and volumes and clean them up afterward. They exercise offline start, health, custom WAL settings, logs, custom/plain backups, transaction rollback, stop/restart, container recreation, and data retention on PostgreSQL 17 and 18:

```sh
docker pull postgres:17-alpine
docker pull postgres:18-alpine
cargo test --locked --no-default-features --test docker_integration -- --ignored
```

Source layout: `model.rs` validates configuration; `store.rs` owns SQLite persistence; `docker.rs` runs bounded Docker commands without a shell; `service.rs` coordinates operations and snapshots; `ui.rs` renders the GPUI interface.

## Packaging and releases

```sh
cargo install cargo-packager --locked --version 0.11.8

# Run the command for the OS you are building on:
cargo packager --release --formats app,dmg  # macOS
cargo packager --release --formats nsis     # Windows
cargo packager --release --formats deb      # Ubuntu 24.04
```

[cargo-packager](https://github.com/crabnebula-dev/cargo-packager) reads `[package.metadata.packager]` in `Cargo.toml`, builds the release binary with the lockfile, and writes packages to `dist/`. The app version and description come from the Cargo package metadata. `mise install` also installs the pinned packager version.

macOS produces `HandyPOS.app` and a DMG; Windows produces an NSIS installer; Linux produces a DEB with a desktop entry and runtime dependencies. Install the DEB with `sudo apt install ./dist/*.deb`. Linux packages target Ubuntu 24.04 or compatible systems and require a Vulkan-capable graphics driver and access to a Docker daemon. The macOS bundle is ad-hoc signed; distribution signing/notarization and Windows code signing are not configured.

CI builds and checks all three desktop platforms and runs the Docker tests on Linux. The release workflow packages each platform when a `v*` tag is pushed, generates a SHA-256 file for each installer, then creates a **draft** GitHub release. The macOS release asset is the DMG containing the app. Manual workflow runs produce artifacts without creating a release. No release is published automatically.
