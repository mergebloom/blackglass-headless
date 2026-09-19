# Native Blackglass Headless

`bgh` is an independent Rust CLI and local stdio MCP client for Blackglass
Server. It is intended for a trusted Linux host running Hermes or other
automation without a GUI. The storage host never receives the custom vault
password or derived content keys. The local host stores plaintext notes and a
0600 profile containing its account token and derived vault key; secure that
host and back up the profile privately. The server still sees account and
membership metadata, file sizes, extensions, timestamps, and traffic patterns.
Hermes and its configured model provider see note content returned by tools.
The client also creates an owner-only vault-ownership marker beside the vault
directory so a second profile cannot silently enroll the same local root.

This tree does not use the bundled Obsidian Headless code at runtime. The
adapter in the parent repository is an interoperability test oracle, not part
of native artifacts. Do not publish a source archive of the whole parent
repository as a native-only release.

## Build and configure

```sh
cargo build --locked --release --manifest-path native/Cargo.toml
bgh configure --server https://sync.example.com
bgh login --email you@example.com
bgh vault list
bgh vault create --name Notes
mkdir -p /srv/notes
bgh vault connect --id VAULT_ID --path /srv/notes --data-host data.example.com
bgh sync once
```

The password prompts are interactive; `--password-stdin` is available for a
protected local credential pipe. Never put passwords in command arguments or
MCP tool inputs. `--data-host` pins a data host different from the control
origin and must exactly match the host returned by Blackglass Server. HTTP is
accepted only for loopback tests. Only custom-E2EE version 3 vaults are
accepted; server-managed encryption is rejected.

`bgh note list`, `bgh note search QUERY`, `bgh note read PATH`, and
`bgh note write PATH` operate on local Markdown files. `note write` reads
content from stdin. Editing an
existing note requires `--expected-sha256 HASH` from `note read`. Writes are
local until `bgh sync once` succeeds. The MCP equivalent is `bgh mcp`, exposing
`note_list`, `note_search`, `note_read`, `note_write`, `sync_run`,
`sync_status`, and `sync_cancel` over local stdio. Sync runs in a serialized
worker so status and ping remain responsive while it is in progress. A cancelled
upload may have reached the server; run Sync again to reconcile. For Hermes,
configure the executable path and
arguments `--profile /private/path/profile.json mcp --auto-sync-seconds 30`
for automatic polling while Hermes keeps the MCP process open; omit the option
to sync only on `sync_run`. Do not expose it as a network MCP server. A
dedicated Unix user and vault directory are recommended for an agent.

## Current boundaries

The engine performs one-shot cycles; `bgh sync watch --interval-seconds 30`
repeats them for a long-lived CLI process. Only one process can own a profile,
so `sync watch` and `bgh mcp` cannot use the same profile concurrently.
MCP automatic polling stops when Hermes closes the MCP process. There is no
independent durable background service, SQLite journal, or remote conditional
write support.
The server's existing protocol lacks upload idempotency and compare-and-swap;
an interrupted upload can have an indeterminate outcome. Local/remote edits
to the same known file fail closed and retain the local bytes, but a race
arriving after catch-up may still be overwritten. There is no automatic
conflict merge. Empty folders, rename metadata, history, revocation lifecycle,
offline queues, and server-restore recovery are not qualified. Do not use it
as the sole copy of important data yet.
Previous local versions and remote-deleted files are retained in an owner-only
history directory beside the vault, outside the synchronized namespace. Back
up and protect it as plaintext; it currently has no automatic retention limit.
An incomplete download or interrupted note replacement is retained as a
`.blackglass-partial` or `.blackglass-tmp` file and blocks further Sync until
inspected. Never delete such a file without checking whether it contains the
only copy of an edit.

## Linux candidate packaging

On each native Ubuntu build host, set an architecture-specific
`CARGO_TARGET_DIR`, then run `sh native/tools/package-linux.sh arm64 OUT` or
`sh native/tools/package-linux.sh amd64 OUT`. The script refuses to overwrite
old outputs and writes separate source and binary archives, a standalone
executable, a dependency-license inventory, and SHA-256 checksums.
`sha256sums.txt` uses relocatable basenames. Binary archives also include
the project license and third-party license texts. Packaging compiles from an
isolated extraction of the exact source archive whose hash is embedded in the
binary.
`bgh build-info` reports the source archive SHA-256 embedded in the binary.
Ubuntu 24.04 glibc and Rust 1.92 were used for
the initial candidates; older Linux distributions have not been tested.

## Validation

```sh
cargo fmt --check --manifest-path native/Cargo.toml
cargo test --locked --manifest-path native/Cargo.toml
cargo build --locked --manifest-path native/Cargo.toml
BLACKGLASS_SERVER_BINARY=/path/to/blackglass-server \
  BLACKGLASS_NATIVE_BINARY=/path/to/bgh \
  node --test native/tests/interoperability.cjs
```

The interoperability test uses a disposable loopback Server and the pinned
reference client. It covers custom-E2EE recovery, bidirectional Markdown and
multi-piece binary files, edits, deletion, wrong-password rejection, conflict
preservation, server restart, backup verification, rollback rejection, server
plaintext absence, MCP note-write-to-Sync, and MCP automatic polling. These tests
are not a replacement for the full release matrix in the
[development plan](../docs/plans/native-client.md).

On Linux, `sh native/tests/package-relocation.sh PACKAGE_DIRECTORY` verifies
relocated checksums, source identity, and notices. Then
`sh native/tests/linux-packaged-smoke.sh /path/to/blackglass-server
/path/to/bgh` exercises two native profiles, deletion, restart, backup, and
ciphertext storage against a disposable loopback server. It uses only synthetic
test data.

Blackglass is independent of and not endorsed by Obsidian. Users must supply
their own legitimate Obsidian installation for Bridge; this native client
does not require one at runtime.
