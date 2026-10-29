# Quarry

Quarry is a fast, lightweight desktop database client written in Rust. It delivers the core developer workflow of DataGrip—writing queries, navigating schemas, editing rows with atomic review, and importing/exporting data—without IDE bloat, slow startup, or heavy background resource consumption.

Built with [GPUI](https://www.gpui.rs) for GPU-accelerated 60fps rendering and powered by an asynchronous multi-driver database engine.

It's currently in early beta. Feedback, bug reports, and contributions are welcome!

![Quarry: SQL editor, results grid with cell aggregates, schema browser](assets/screenshot.png)

---

## Highlights

- **SQL Editor:** Schema-aware autocompletion (keywords, tables, columns, CTEs, subqueries, and foreign-key join hints), query formatting, syntax highlighting, explain plans, query parameter placeholders (`:id`, `$1`), and an optional modal **Vim mode**.
- **Interactive Data Grid:** Zero-copy column-oriented row buffer rendering up to 100,000 rows smoothly. Live cell editing with optimistic concurrency verification, cell range selection, aggregate summaries (sum, avg, count, min, max), transposed vertical view, and foreign-key navigation (`F4`).
- **Review Before Commit:** Visual review dialog displaying generated `UPDATE`, `INSERT`, and `DELETE` SQL statements before applying modifications to the database.
- **Visual Filter Builder:** Construct composite filters visually with comparison operators (`=`, `!=`, `>`, `<`, `LIKE`, `IN`, `IS NULL`) without hand-crafting `WHERE` clauses.
- **ER Diagram & Schema Explorer:** Interactive foreign-key relationship diagrams with multi-hop neighbors, table structure inspection (columns, keys, indexes, triggers, table stats), DDL generation, and fast table peeking on hover.
- **Import & Export:** Streaming import wizard with CSV / JSON auto-detection, schema inference, duplicate handling (`ON CONFLICT DO NOTHING`), and streaming export to CSV, JSON, or SQL `INSERT` statements.
- **Security & Safety:** Encrypted SSH tunneling, OS keychain password storage, and environment tagging (**Local**, **Staging**, **Production**). Production environments safeguard against accidental mutations with explicit confirmation prompts.
- **Command Bar (`⌘K` / `Ctrl+K`):** Unified keyboard-first launcher to quickly switch connections, open tables, run recent queries, and trigger commands.

---

## Supported Databases

Quarry abstracts database engines behind a modular driver interface in `crates/quarry-core`. Adding new engines requires implementing the driver traits without touching the UI layer (see [docs/Adding-a-Database.md](docs/Adding-a-Database.md)).

| Engine | Driver Crate / Protocol | Features | Notes |
|---|---|---|---|
| **PostgreSQL** | `tokio-postgres` (Rustls TLS) | Full introspection, streaming cursors, `EXPLAIN (FORMAT JSON)`, `pg_dump`, server activity & locks | SSH tunneling, per-tab `search_path` schema switching |
| **SQLite** | `rusqlite` (bundled C engine) | Schema inspection, in-memory & file DBs, built-in SQL dump | `:memory:` connections are ephemeral |
| **MySQL / MariaDB** | `mysql_async` (Rustls TLS) | Processlist, statements monitor, mysqldump, table DDL | Supports `#` comments and `\'` escape sequences |
| **SQL Server** | `tiberius` (TDS 7.3) | Active requests, query stats, lock inspector | Supports `GO` batch statement separation |
| **Oracle** | `rust-oracle` (ODPI-C) | Sessions, SQL stats, lock monitors, PL/SQL blocks | Dynamic Instant Client loading; PL/SQL blocks end at `/` |

---

## Screenshots

| | |
|---|---|
| ![ER diagram of a table and its foreign-key neighbours](assets/screenshot-diagram.png) | ![Table structure: columns, keys, indexes and references](assets/screenshot-structure.png) |
| **ER Diagram:** Visual graph of tables and foreign-key relationships | **Structure View:** Columns, keys, indexes, foreign keys, and comments |
| ![Review dialog showing the generated UPDATE and DELETE statements](assets/screenshot-review.png) | ![Command bar listing actions with their shortcuts](assets/screenshot-command-bar.png) |
| **Review Before Commit:** Inspect generated SQL statements before executing edits | **Command Bar (`⌘K`):** Fast launcher for actions, tables, and query history |

---

## Architecture

Quarry is organized as a Cargo workspace with three focused crates:

```
quarry/
├── crates/
│   ├── quarry-core/    # Engine-agnostic database driver traits, connection registry,
│   │                   # column-oriented RowBuffer, DbValue, schema catalog, streaming I/O
│   ├── quarry-sql/     # Statement splitting, AST-assisted autocomplete, CTE extraction,
│   │                   # parameter binding (:id, $1), and query rewrites
│   └── quarry-app/     # GPU-accelerated desktop UI (GPUI), background Tokio runtime,
│                       # data grids, tab/split layout, and dialog workflows
└── docs/               # The GitHub wiki: guides, architecture, and
                        # Adding-a-Database.md for new engines
```

For an in-depth architectural breakdown—including memory layout, asynchronous runtime isolation, and data-flow diagrams—see the [Architecture Documentation (GitHub Wiki)](docs/ARCHITECTURE.md).

---

## Keyboard Shortcuts

| Shortcut (macOS) | Shortcut (Win / Linux) | Action |
|---|---|---|
| `⌘K` | `Ctrl+K` | Open Command Bar (actions, tables, queries) |
| `⌘↵` | `Ctrl+Enter` | Run statement at cursor (or selected text) |
| `⇧⌘↵` | `Ctrl+Shift+Enter` | Run all statements in editor |
| `⌘T` | `Ctrl+T` | Open new query tab |
| `⌘W` | `Ctrl+W` | Close active tab |
| `⇧⌘T` | `Ctrl+Shift+T` | Reopen last closed tab |
| `⌘P` | `Ctrl+P` | Quick switch tab |
| `⌘B` | `Ctrl+B` | Toggle left sidebar |
| `⌘J` | `Ctrl+J` | Toggle bottom panel (history, logs, server monitor) |
| `⌘\` | `Ctrl+\` | Split active tab horizontally / vertically |
| `⌥↵` | `Alt+Enter` | Show query code actions (expand `*`, add `LIMIT`, wrap in transaction) |
| `F4` | `F4` | Follow foreign key from selected grid cell |
| `⌘Z` / `⇧⌘Z` | `Ctrl+Z` / `Ctrl+Y` | Undo / redo staged data grid edits |

---

## Installation

### Linux & Windows
Download a pre-built `.deb`, `.AppImage`, or `.rpm` for Linux (x86_64 or ARM64), or the Windows installer, from [GitHub Releases](https://github.com/thekodeking/quarry/releases). Fedora users can install the matching RPM with `sudo dnf install ./quarry-*.rpm`.

### macOS
Download `Quarry-<version>.dmg` from [GitHub Releases](https://github.com/thekodeking/quarry/releases) and drag Quarry to Applications. The app is ad-hoc signed rather than notarized by Apple, so macOS blocks the first launch. Clear the quarantine flag once:

```sh
xattr -dr com.apple.quarantine /Applications/Quarry.app
```

Or open it, then click **Open Anyway** in System Settings → Privacy & Security.

To build it yourself instead (no Gatekeeper warning):

```sh
# Prerequisites: Xcode command line tools (xcode-select --install) and Rust (https://rustup.rs)
git clone https://github.com/thekodeking/quarry && cd quarry
cargo install cargo-packager --locked
cd crates/quarry-app && cargo packager --release -f app
cp -R ../../dist/Quarry.app /Applications/
```

---

## Development

Install the native prerequisites for your platform before compiling. Rust is also required; install it with [rustup](https://rustup.rs).

### Debian / Ubuntu

```sh
sudo apt install build-essential pkg-config clang mold \
  libasound2-dev libdbus-1-dev libfontconfig1-dev libfreetype6-dev \
  libvulkan-dev libwayland-dev libx11-xcb-dev libxcb1-dev \
  libxkbcommon-dev libxkbcommon-x11-dev libfuse2
```

### Arch Linux

```sh
sudo pacman -S --needed base-devel pkgconf clang mold \
  alsa-lib dbus fontconfig freetype2 vulkan-headers vulkan-icd-loader \
  wayland libx11 libxcb libxkbcommon libxkbcommon-x11 fuse2
```

### Fedora

```sh
sudo dnf install gcc-c++ pkgconf-pkg-config clang mold \
  alsa-lib-devel dbus-devel fontconfig-devel freetype-devel \
  vulkan-headers vulkan-loader-devel wayland-devel libX11-devel \
  libxcb-devel libxkbcommon-devel libxkbcommon-x11-devel fuse-libs
```

### macOS

```sh
xcode-select --install
```

The FUSE package is only needed for AppImage packaging; it is not required to compile Quarry.

```sh
make            # List available commands
make dev        # Run a debug build (fast compile)
make run        # Run a release build (smooth UI)
make test       # Run unit and integration tests across workspace
make lint       # Check rustfmt formatting and Clippy warnings
make fmt        # Format all code with rustfmt
make bench      # Run Criterion benchmarks for quarry-core and quarry-sql
```

### Packaging & Release Builds

Packages are generated using [cargo-packager](https://github.com/crabnebula-dev/cargo-packager) and output to `dist/`:

```sh
cd crates/quarry-app
cargo build --release -p quarry-app
cargo packager --release -f app       # macOS .app
# or: -f deb, -f appimage, -f nsis
```

`cargo-packager` creates the DEB and AppImage packages; RPMs are assembled with [nfpm](https://github.com/goreleaser/nfpm) in the release workflow. Linux packages are built in parallel for x86_64 and ARM64 and published together. `make package` packages for the current OS (`OS=macos|linux|windows` to choose). On macOS that's the ad-hoc signed DMG from `scripts/package-macos.sh`.

To build an RPM locally from the repository root after compiling, install nfpm with `go install github.com/goreleaser/nfpm/v2/cmd/nfpm@latest`, then run:

```sh
version=$(cargo pkgid -p quarry-app | sed 's/.*[#@]//')
mkdir -p dist
VERSION="$version" GOARCH="$(go env GOARCH)" nfpm package \
  --packager rpm --config nfpm.yaml \
  --target "dist/quarry-${version}-$(uname -m).rpm"
```

To create a new release, push a git tag matching the workspace version (e.g. `v0.1.0`).

---

## Documentation

- **[Architecture Guide & Wiki](docs/ARCHITECTURE.md):** Deep-dive into design decisions, threading models, and engine subsystems.
- **[Driver Implementation Guide](docs/Adding-a-Database.md):** Step-by-step instructions for implementing and testing new database engines.

---

## License

[GPL-3.0-or-later](LICENSE)
