# Tusklet

A native desktop workspace for local PostgreSQL Docker containers. Built with **Rust, GPUI, and SQLite**.

## Features

- Named projects containing multiple independently managed databases.
- Any official `postgres` Docker Hub image tag, including Alpine variants and prereleases.
- Cached images work automatically without internet access. Only missing images are downloaded.
- Start/stop controls, health status, and PostgreSQL logs with pause/resume and copy.
- Right-click menus and “…” action buttons on projects and databases, with removal available in their settings too.
- macOS menu bar icon with databases grouped by project, live status, and start/stop controls, even with the main window closed.
- Custom PostgreSQL settings, such as `-c wal_level=logical -c max_replication_slots=10`.
- Automatic available ports or explicit reserved ports, bound only to `127.0.0.1`.
- Persistent Docker volumes and SQLite configuration across app and container restarts.
- Custom-format `pg_dump` exports and custom-format/plain SQL restores, using the container's PostgreSQL tools.
- Copy a connection URL with a randomly generated password for each database.
- Keyboard navigation throughout the workspace, forms, action menus, and log viewer.
- Searchable command palette for creation, start/stop, settings, backups, navigation, and connection URLs.

## Run

After the first stable release is published to the [Homebrew tap](https://github.com/mattsverse/homebrew-tap), install on macOS (Apple Silicon only) or Linux (x86-64) with:

```sh
brew install --cask mattsverse/tap/tusklet
```

Linux casks require a current Homebrew with AppImage support. The AppImage targets Ubuntu 24.04 or compatible systems (glibc 2.39 or newer), requires FUSE 2 (`libfuse2t64` on Ubuntu 24.04), and can be launched with `tusklet`. macOS installs `Tusklet.app` in Applications; the app is ad-hoc signed and is not notarized, so Gatekeeper may require explicit approval to open it. Both platforms require the Docker CLI and a running local Docker daemon.

To build from source:

Install Rust (edition 2024; the current stable toolchain is recommended) and Docker Desktop, or Docker Engine with the `docker` CLI. Start Docker and use a **local Docker context**. Windows requires Docker's Linux-container mode.

```sh
cargo run --locked
```

Create a project, add a database, then click **Start database**. The database form lists locally cached tags and also accepts any official PostgreSQL tag without contacting Docker Hub. Starting uses the cached image when available and downloads it only if missing, so a Docker Hub outage does not block creating or starting a database with a cached image. If Docker is unavailable, saved projects remain visible and can still be created or renamed.

For an isolated workspace, useful during development:

```sh
cargo run --locked -- --data-dir /tmp/tusklet-dev
```

macOS requires Apple Silicon and the Xcode command-line tools. Windows needs the MSVC build tools and Windows SDK. Linux needs a working GPU/Vulkan driver, X11 or Wayland, and the following build packages on Ubuntu 24.04:

```sh
sudo apt-get install clang cmake pkg-config libasound2-dev libfontconfig1-dev \
  libfreetype6-dev libssl-dev libvulkan-dev libwayland-dev libx11-xcb-dev \
  libxcb1-dev libxcb-composite0-dev libxcb-damage0-dev libxcb-randr0-dev \
  libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev \
  libxkbcommon-x11-dev libzstd-dev
```

## Storage and behavior

Each database gets a container named `tusklet-<uuid>` and a volume named `tusklet-<uuid>-data`. PostgreSQL data lives in `/var/lib/postgresql/tusklet` within the mounted volume. Explicit `PGDATA` supports the different volume defaults in PostgreSQL 17 and 18. [Official image documentation](https://hub.docker.com/_/postgres)

- **Stop** preserves data. Quitting Tusklet also leaves databases running.
- On **macOS**, closing the window keeps Tusklet in the menu bar. Click its database icon to start or stop databases, including ones that have never been started. **Open Tusklet** or the Dock icon reopens the same workspace. **Quit Tusklet** (or Cmd+Q) exits the app. Status refreshes every two seconds; controls are disabled during another operation or while editing a form. Errors appear in the menu and can be opened in the main window for details. Log following pauses while the window is closed. If the menu bar icon cannot be created, closing the window exits as before. Windows and Linux currently exit when the last window closes.
- Right-click a project or database, or click its **…** button, to open its actions. Database actions include **Start/Stop**, **Database settings**, and **Remove database**. Project actions include **New database**, **Project settings**, and **Remove project**.
- **Settings** can change the display name, port, and PostgreSQL arguments while stopped. Settings remain viewable while running or Docker is unavailable. Tusklet recreates the container with the same volume at the next start. Project settings let you rename or remove an empty project.
- Image tag, initial database, and user are immutable after creation. To change versions, create a database and migrate with backup/restore.
- Assigned ports stay reserved within Tusklet, including automatically chosen ports and stopped databases. Other applications can still take a stopped container's port; start reports a conflict instead of silently changing it.
- **Remove database**, available from its action menu, detail screen, or settings, requires typing its saved name and a stopped container. It removes the app entry and container and **retains the named data volume by default**. Select **Also delete the data volume** in the confirmation to permanently delete the database data too. The confirmation shows the volume name and whether it will be kept or deleted. If volume deletion fails, the app entry stays available so you can retry; volumes still in use are never forcibly removed. Empty projects can be removed separately from their action menu or settings, also by confirming their name. Remove a project's databases before removing the project.
- Log following refreshes a bounded 400-line tail every two seconds. Docker rotates log files at 10 MB, retaining three files. Operations run on a worker thread.

Configuration and generated passwords are saved in `tusklet.sqlite3` in the operating system's local application-data directory (`~/Library/Application Support/com.matteogassend.tusklet` on macOS, `$XDG_DATA_HOME/tusklet` or `~/.local/share/tusklet` on Linux, `%LOCALAPPDATA%\matteogassend\tusklet\data` on Windows). `--data-dir` overrides this location. Passwords are stored locally, not encrypted; the SQLite file is restricted to the current user on Unix. Docker also retains the container environment. This is a **local development tool**, not a production secret manager. Back up both the SQLite file and database dumps if migrating machines.

## Keyboard controls

- **Cmd+K** on macOS or **Ctrl+K** elsewhere opens the command palette; the **Commands** button in the sidebar opens it too. Search by action, project, or database name (for example, `create database storefront` or `stop dev`). **Up / Down** or **Tab / Shift+Tab** select a result, **Enter** runs it, and **Escape** closes the palette and restores focus. Available actions respect Docker/container state and ongoing operations; removal and restore still use their confirmation screens. Finish or cancel an open form before opening the palette.
- **Tab / Shift+Tab** move between enabled controls. Forms and the project list scroll to reveal the focused control.
- **Enter / Space** activate buttons and open **…** menus. In a menu, use **Up / Down**, **Enter** to select, and **Escape** to return to its trigger. **Tab / Shift+Tab** close the menu and continue navigation. **Shift+F10** or the context-menu key opens the focused project or database's actions.
- **Enter** in an editable field saves an ordinary project/database form. **Cmd+Enter** on macOS or **Ctrl+Enter** elsewhere also saves. **Escape** cancels. Input-method composition is handled before form shortcuts.
- Removal and restore confirmations require typing the saved name and then activating the labeled confirmation button. **Space** toggles **Also delete the data volume** once per press. Keyboard actions respect the same busy, read-only, and container-state restrictions as mouse actions.
- Tab into **PostgreSQL logs** to scroll with **Up / Down**, **PageUp / PageDown**, or **Home / End**. **Cmd+Up / Cmd+Down** on macOS and **Ctrl+Up / Ctrl+Down** elsewhere also move to the beginning/end. Manual keyboard scrolling pauses following; activate **Resume follow** to return to live logs. Tab moves out of the viewer.

## Backups

**Export backup** writes a PostgreSQL custom-format `.dump` through a temporary file, publishing it only after success. Existing files are never overwritten. No host PostgreSQL installation is needed.

**Restore backup** accepts `pg_dump` custom-format files or plain SQL. The target must be running; an empty database is recommended. Restores run in a single transaction with stop-on-error, and do not drop existing objects. Conflicts roll back ordinary transactional SQL. Plain scripts that manage their own transactions or require commands outside a transaction are not supported. Only restore trusted backups: PostgreSQL dumps can contain executable SQL. Cluster-wide `pg_dumpall` archives, directory archives, and gzip files are not supported.

## Development and checks

```sh
cargo fmt --all --check
cargo clippy --locked --all-targets --features ui-tests -- -D warnings -D clippy::pedantic -D clippy::perf -D clippy::suspicious
cargo test --locked --no-default-features
cargo test --locked --features ui-tests --bin tusklet
cargo build --locked
```

The UI tests render the database detail screen at multiple window sizes and exercise keyboard workflows without opening a desktop window or connecting to Docker. Keyboard tests send both key-down and key-up events, covering focus recovery, menus, forms, destructive confirmations, scrolling, disabled controls, composition, and window reopening. The backend can be built and tested without desktop dependencies. Opt-in Docker integration tests create uniquely named containers and volumes and clean them up afterward. They exercise offline start, health, custom WAL settings, logs, custom/plain backups, transaction rollback, stop/restart, container recreation, and data retention on PostgreSQL 17 and 18:

```sh
docker pull postgres:17-alpine
docker pull postgres:18-alpine
cargo test --locked --no-default-features --test docker_integration -- --ignored
```

Source layout: `model.rs` validates configuration; `store.rs` owns SQLite persistence with SeaORM entities in `store/entities/`; `docker.rs` runs bounded Docker commands without a shell; `service.rs` coordinates operations and snapshots; `ui.rs` renders the GPUI interface.

### SQLite migrations

Workspace persistence uses [SeaORM](https://www.sea-ql.org/SeaORM/) entities, typed column filters, and typed active models. Saved configuration is mapped directly to `DatabaseConfig` through SeaORM's JSON support. The store runs SeaORM on a private Tokio runtime on the existing service worker thread; callers keep a synchronous API. Query logging is disabled because parameters include generated passwords.

Rust migrations live in `src/store/migrations/` and are registered in `src/store/migrations.rs`. SeaORM records applied migrations in `seaql_migrations`. Both persistent and in-memory stores apply pending migrations before workspace queries run. An explicit SQLite immediate transaction covers the history check, all pending schema changes, and migration records: failures roll back together, and concurrent startups cannot apply the same migration twice. Unknown migration names are rejected.

To change the schema, add an `mYYYYMMDD_NNNNNN_description.rs` module implementing `MigrationName` and `MigrationTrait`, write `up` and `down` with SeaQuery, and append the migration to `Migrator::migrations()`. Update the affected entities and add an upgrade test with existing data. Keep identifiers local to each migration so later entity changes cannot alter migration history. Never edit, remove, or reorder released migrations. The only SQLite-specific SQL fragment in the initial schema is `COLLATE NOCASE`, for which SeaQuery has no column builder.

Run `cargo test --locked --no-default-features` for backend tests, including persistence and migrations. Typed models catch field and filter type mistakes at compile time; database constraints, schema compatibility, and stored JSON still require runtime validation and tests.

This development change does not import the old `rusqlite`/`user_version` schema. Delete the old development `tusklet.sqlite3` or use a new `--data-dir` to start with the SeaORM schema. Subsequent schema changes use tracked migrations.

## Packaging and releases

```sh
cargo install cargo-packager --locked --version 0.11.8

# Run the command for the OS you are building on:
cargo packager --release --formats app,dmg  # macOS (Apple Silicon)
cargo packager --release --formats nsis     # Windows
cargo packager --release --formats deb,appimage  # Ubuntu 24.04
```

[cargo-packager](https://github.com/crabnebula-dev/cargo-packager) reads `[package.metadata.packager]` in `Cargo.toml`, builds the release binary with the lockfile, and writes packages to `dist/`. The app version and description come from the Cargo package metadata. `mise install` also installs the pinned packager version.

Workflows reuse the local [`package-desktop` action](.github/actions/package-desktop/README.md) for toolchain setup, backend tests, installer builds, checksums, and artifact uploads. Callers supply package formats, the artifact name, and retention days; the action exposes the uploaded artifact ID, URL, and digest. The workflow keeps responsibility for choosing platforms, updating the PR comment, and creating draft releases.

macOS produces `Tusklet.app` and a DMG; Windows produces an NSIS installer; Linux produces a DEB with a desktop entry and runtime dependencies, plus an AppImage. Install the DEB with `sudo apt install ./dist/*.deb`. Linux packages target Ubuntu 24.04 or compatible systems and require a Vulkan-capable graphics driver and access to a Docker daemon. AppImage packaging also needs `libfuse2t64` on Ubuntu 24.04. The macOS bundle is ad-hoc signed; distribution signing/notarization and Windows code signing are not configured.

CI builds and checks all three desktop platforms and runs the Docker tests on Linux. **Package applications** also builds the actual installers for every pull request, using the same packaging commands as releases. One bot comment is updated with build status, the PR commit, and download links for the macOS Apple Silicon DMG, Windows x86-64 installer, and Linux x86-64 DEB/AppImage. Sign in to GitHub and unzip the downloaded artifact to find the installers and SHA-256 checksums. PR artifacts expire after 14 days; rerun the packaging workflow to refresh them. New commits replace the links when their build starts, and superseded builds cannot overwrite a newer comment.

The comment updater runs from the default branch after packaging starts or completes, so `.github/workflows/pr-build-comment.yml` and its script must land on `main` before comments appear. It supports fork PRs without giving their build jobs write permissions and reads only GitHub's artifact metadata. Preview builds have the same signing and system requirements described above.

GitHub may require approval before running workflows for fork PRs and [PRs created by `GITHUB_TOKEN`](https://docs.github.com/en/actions/concepts/security/github_token), including the automated release PR. Approve those runs in GitHub Actions to start packaging.

When a `v*` tag is pushed, the packaging workflow generates a SHA-256 file for each installer, then creates a **draft** GitHub release. macOS releases include an Apple Silicon DMG containing the app; Intel Macs are not supported. Manual workflow runs produce artifacts without creating a release. No release is published automatically.

### Homebrew publishing

Publishing a stable GitHub release triggers `.github/workflows/homebrew.yml`. The official `Homebrew/actions/setup-homebrew` action prepares Homebrew, then `brew bump-cask-pr --write-only` updates the cask version and calculates SHA-256 checksums from the Apple Silicon macOS DMG and the Linux x86-64 AppImage. The workflow commits `Casks/tusklet.rb` directly to the default branch of `mattsverse/homebrew-tap`. Drafts and prereleases are excluded. Rerunning an already published version is a no-op; older versions cannot overwrite a newer cask. The tap must allow direct pushes by the publishing token.

Before the first publication:

1. Make `mattsverse/tusklet` public so installer URLs are accessible to Homebrew users.
2. Create a fine-grained GitHub token limited to `mattsverse/homebrew-tap`, with **Contents: Read and write**. Add it to Tusklet's Actions secrets as `HOMEBREW_TAP_TOKEN`. The default `GITHUB_TOKEN` cannot write to another repository.
3. Push these workflows to `main`, create a versioned release through the existing release process, and publish its draft after all installers have uploaded. Publish from the GitHub UI or with a personal/App token: publication using `GITHUB_TOKEN` does not trigger the Homebrew workflow.

To retry publication, run **Publish Homebrew cask** manually with the existing stable tag (for example `v1.2.3`). It fails before pushing if the repository is private, the token is missing, or Homebrew cannot download an installer. Users receive subsequent versions with `brew upgrade --cask mattsverse/tap/tusklet`.

Each new release refreshes the tap checkout from `.github/homebrew/tusklet.rb` so supported-platform changes also reach existing casks. Its bootstrap version and placeholder checksums are replaced by Homebrew before the cask is pushed. The workflow skips Homebrew's audit because the macOS app is deliberately ad-hoc signed. No Python tooling is required.

`assets/tusklet.png` is the transparent elephant/database logo used in the app sidebar and by the desktop packager. It is a square 1024-pixel PNG, also suitable for AppImage.

`assets/tray.png` is the matching monochrome macOS menu bar icon. macOS uses its transparency as a template to adapt to light and dark menu bars. The original artwork, prepared masters, and regeneration commands are documented in [assets/README.md](assets/README.md).
