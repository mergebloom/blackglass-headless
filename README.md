# Blackglass Headless

An independent Rust CLI and local stdio MCP client is available as a production
under [native/](native/README.md). It uses the same Blackglass Sync service but
does not run or ship the upstream JavaScript bundle. The existing Node-based
adapter described below remains the reference client while native release
qualification grows. Both executables use the `bgh` name; install only the one
whose instructions you intend to follow.
The [current native candidate evidence](docs/native-service-candidate-2026-09-20.md)
records the new service and Linux package tests separately from the existing
adapter release.

Blackglass Headless is a command-line Sync client for a self-hosted
[Blackglass Server](https://github.com/mergebloom/blackglass-server). It runs
without a desktop session or GUI, so it can keep a Markdown vault synchronized
on a Linux server, NAS, CI worker, or macOS terminal.

This fork preserves the official Obsidian Headless bundle unchanged. A small
launcher verifies the exact reviewed upstream release, applies the Blackglass
compatibility incisions in memory, uses an isolated profile, and fails closed
when upstream code differs. Sync data hosts continue to come from the selected
Blackglass Server. Obsidian Publish commands are disabled.

## Supported baseline

- Blackglass Headless 0.2.0
- Obsidian Headless 0.0.14, exact upstream commit and SHA-256 documented in
  [UPSTREAM.md](UPSTREAM.md)
- Node.js 22 or later
- Runtime and Sync validation on Linux amd64, Linux arm64, and Apple Silicon
  macOS against Blackglass Server 0.6.1
- Blackglass Sync only; no GUI and no Publish support

Compatibility is claimed only for the exact upstream identity that passes the
Blackglass conformance checks.

## Install

Install Node.js 22 or later, then install directly from the fork:

```bash
npm install --global github:mergebloom/blackglass-headless
```

For development:

```bash
git clone https://github.com/mergebloom/blackglass-headless.git
cd blackglass-headless
corepack pnpm install --frozen-lockfile
pnpm check
npm link
```

## Configure and use

Point the isolated Blackglass profile at your own control origin:

```bash
bgh configure --server https://sync.example.com
bgh configure --show
bgh login
bgh sync-list-remote
```

Create or connect a vault and start continuous background synchronization:

```bash
mkdir -p ~/vaults/notes
cd ~/vaults/notes
bgh sync-setup --vault "Notes" --device-name "headless-server"
bgh sync --continuous
```

For a one-off invocation, `--server` overrides the saved origin:

```bash
bgh --server https://sync.example.com sync-list-remote --json
```

`BLACKGLASS_CONTROL_ORIGIN` is the non-persistent server override and
`BLACKGLASS_AUTH_TOKEN` is the optional non-persistent authentication token.
HTTPS is required except for loopback development. Credentials, paths, query
parameters, and fragments are rejected in server origins.

Authentication and server selection are stored under
`~/.config/blackglass-headless` on Linux (or `$XDG_CONFIG_HOME`) and
`~/.blackglass-headless` elsewhere. Blackglass does not reuse the upstream
Obsidian Headless profile.

One profile maps a remote vault to one local folder. To test or run a second
local copy of the same remote vault on one host, give it a separate operating
system home/profile (for example, a separate container or service account).
Reusing one profile for two folders would reuse the first folder's Sync state.

The pinned upstream package does not include a Linux native module for file
birth times. Linux Sync content is byte-identical, but creation-time metadata
may not be preserved.

Run `bgh --help` for the upstream-compatible Sync command reference.

## Compatibility validation

The fast suite verifies the exact upstream file and every incision, config-file
permissions, origin validation, Publish blocking, and launcher execution:

```bash
corepack pnpm check
```

The opt-in server E2E creates a disposable account and custom-E2EE vault,
uploads Markdown and binary data, recovers it into a clean profile, proves
continuous bidirectional Sync and deletion, restarts the server, and verifies a
backup:

```bash
BLACKGLASS_SERVER_BINARY=/path/to/blackglass-server \
  corepack pnpm test:e2e
```

## Project boundaries

- [Blackglass Server](https://github.com/mergebloom/blackglass-server) owns the
  Rust/SQLite service, deployment, migrations, backups, and Linux server
  artifacts.
- [Blackglass Bridge](https://github.com/mergebloom/blackglass-bridge) owns the
  adapted graphical desktop client and the broader conformance tooling.
- This repository owns the no-GUI command-line adapter and its exact upstream
  baseline.

Blackglass is an independent research project exploring frontier LLM
capabilities in software analysis, compatibility engineering, implementation,
and end-to-end validation. Obsidian is a third-party product; Blackglass is not
endorsed by Obsidian. This fork retains the upstream `UNLICENSED` designation
and provenance. Review the upstream terms before use; this note is not legal
advice.
