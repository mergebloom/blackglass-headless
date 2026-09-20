# Native Blackglass Headless client

Planning baseline: 2026-09-19. Implementation is in progress under `native/`.
The Rust crypto/Sync/CLI/local stdio MCP vertical slice, persistent service,
private SQLite applied-state journal, and negotiated conditional push extension
have passed disposable interoperability tests. Source-bound Linux amd64 and
arm64 candidates passed packaged-client smoke tests, including service recovery
after forced termination. See the [current candidate evidence](../native-service-candidate-2026-09-20.md)
for exact hashes and remaining gaps. The staged gates below remain release
criteria; these candidates are not a release qualification.

## Outcome and boundaries

Build an independent Rust client for Linux amd64 and arm64. Hermes uses local
stdio MCP; people and scripts use `bgh`. Both share one engine and local state.
Blackglass Server receives encrypted Sync payloads and never receives custom
vault passwords or derived decryption keys. No Obsidian application, upstream
JavaScript bundle, Node, Bun, or GUI is required by the released native client.

Initial enrollment supports custom-password encryption version 3 only. Reject
managed-encryption vaults explicitly: the current server stores their password,
which does not meet the selected confidentiality boundary. Do not convert or
migrate an existing vault automatically. Preserve existing server behavior for
other clients. Public-key enrollment and new encryption formats are deferred.

Local notes, attachment files, indexes, and any retained conflict versions are
plaintext on the trusted client machine. Hermes and its configured inference
provider can see content returned by tools. Vault content is data, never trusted
agent instructions. The inherited protocol protects content confidentiality but
does not authenticate all revision/path metadata against a malicious server;
do not claim rollback protection or complete malicious-server integrity.

## Inspected baseline

This audit inspected local working trees, not a fresh remote checkout:

| Project | Local HEAD | Relevant state |
| --- | --- | --- |
| Headless | `43d7113` | Clean before this planning document; adapter 0.1.0, upstream 0.0.14 |
| Server | `b8cd5b1` | Rust service 0.6.1; existing uncommitted Publish work |
| Bridge | `ff22bc9` | Existing uncommitted Publish work; behind the previously published 0.5.0 release |

Keep existing edits intact. Before implementation qualification, bind exact
clean source revisions and artifacts in isolated checkouts if necessary.
No new branch, commit, push, or publication was needed for this work.

Evidence paths, relative to the three sibling repositories:

- Headless `blackglass-headless.cjs`, `lib/adapter.cjs`, `lib/baseline.cjs`:
  verification and in-memory patching of the upstream bundle. There is no
  independent client crypto or sync implementation to port.
- Headless `lib/config.cjs`: origin validation, private configuration, and
  explicit server selection are reusable behavioral requirements.
- Headless `tests/blackglass-server.e2e.cjs`: disposable-account custom-E2EE
  upload/recovery, continuous two-way sync/deletion, restart, and backup verify.
  It is opt-in and skipped without a server binary. It does not by itself test
  an independent crypto implementation, desktop interoperability, conflicts,
  restore recovery, multi-user lifecycle, or Hermes.
- Server `packages/protocol/src/{control,sync}.ts`: useful wire type definitions.
- Server `apps/server-rust/src/server.rs`: snapshot/resume boundary, ordered
  notifications, upload state machine, heartbeats, origin checks, revocation.
- Server `apps/server-rust/src/{db,model}.rs`: transactional revisions and
  authorization; no expected-base revision or idempotency key in uploads.
- Server `tests/{client-contract,tenant-isolation,collaboration}.integration.test.ts`:
  reusable protocol and authorization scenarios, often with opaque test bytes.
- Bridge `tools/e2e-scenario.ts`, `compatibility/matrix.json`, release tooling:
  desktop conformance requirements and source/artifact evidence patterns.

Historical research notes in the earlier Sync workspace describe v3 scrypt,
HKDF-SHA-256, AES-GCM bodies, and deterministic AES-SIV paths/hashes. Treat these
as leads to validate with independent vectors, not a complete wire specification.
The exact encodings, normalization, HKDF labels, verifier, associated data,
nonce/tag layouts, and deterministic behavior must be documented and tested.

## Architecture and ownership

Use the existing Headless repository for development, initially under `native/`.
Keep the current adapter usable as a private interoperability reference until
the native implementation passes qualification. Do not copy its bundled
implementation into Rust or include it in native release assets. Before public
source distribution, resolve independent-code licensing and package only the
native tree and approved fixtures; a full source archive of this fork would
still contain the upstream bundle and its history.

Start with one Rust library and executable, with modules:

- `protocol`: typed control requests and ordered WebSocket state machine.
- `crypto`: exact v3-compatible codec using maintained crypto libraries.
- `credentials`: local account sessions, key storage/unlock, redaction.
- `store`: SQLite migrations, file inventory, operation journal and conflicts.
- `sync`: bootstrap, catch-up, reconciliation, upload/download, resume.
- `vault`: atomic file operations, Markdown edits, attachments, path safety.
- `index`: rebuildable local full-text search; links/tags/properties follow.
- `service`: per-profile process ownership and per-vault serialized operations.
- `cli` and `mcp`: thin adapters to the same application operations.

Reuse the established Rust/SQLite release practices, not server database schema
or internal storage access. The client connects only through existing APIs.
Select and pin dependency versions after checking crypto vectors and MCP SDK
compatibility; language/framework selection is not evidence of correctness.

`bgh` avoids overwriting the existing desktop `blackglass` command. Use separate
native configuration/state directories and explicit profiles; do not silently
import credentials or SQLite state from the Node adapter. A profile maps each
remote vault to one local root. A second local copy needs an explicit profile.

## Process and credential lifecycle

One engine owns a profile's mutable state, with a per-vault work queue. A local
service holds the lock, SQLite handles and Sync connections. CLI/MCP frontends
connect over an owner-only Unix socket; concurrent starts are serialized and
stale process recovery checks ownership. The MCP lifecycle must not determine
the background service lifetime. Provide a systemd user-service installation
and an explicit foreground mode; do not silently leave unmanaged processes.

One-shot CLI operations may own the engine directly when no service is running;
otherwise they delegate. Never run two independent reconciliation loops against
the same local root. Filesystem watchers are hints; periodic and startup scans
recover missed events. Do not assume external editors respect our lock.

Account password and vault password are distinct inputs. Read secrets through
interactive input or a protected file descriptor/credential file, not command
arguments or MCP tool schemas. The service can remain locked until explicitly
unlocked. For unattended use, support deliberate provisioning of a derived key
through protected local credentials; do not claim a 0600 file is encrypted or
safe from its owning user. Avoid storing the original vault password. Exclude
key material from indexes, logs, receipts and routine evidence. No network key
escrow or server-side key packages are needed for the first release.

A dedicated Blackglass collaborator account limits server-side membership.
Local MCP vault/path allowlists narrow the tool surface but are not an OS
security boundary against an agent with shell access as the same user. Use a
separate service identity/container when that stronger isolation is required.

## Protocol constraints and guarantees

1. Control and WebSocket requests require an allowed Origin; retain the existing
   compatibility origin initially. Pin the configured data host and validate
   TLS independently; do not trust arbitrary returned hosts or redirects with
   credentials. CORS is not authentication.
2. WebSocket replies have no general request IDs. Serialize operations and
   demultiplex unsolicited push notices, replies, pong and binary pieces.
   Uploads use 2 MiB pieces, next/ok acknowledgements, and a commit notification
   before final acknowledgement. Honor configured file limits and backpressure.
3. Bootstrap uses `initial:true`; resume uses a durable applied revision cursor.
   Received notices and applied filesystem state need distinct cursors. Advance
   the applied cursor only after recoverable file/journal persistence.
4. `ready.version` provides a catch-up boundary. An active socket is not proof
   that all content has been applied. A fresh read can request a bounded catch-up
   cycle and return the applied boundary. It cannot include unsent offline edits.
5. Peer idle timeout, lagged events and session revocation require deliberate
   handling. Reconnect with bounded backoff and resume; do not busy-loop on
   rejected credentials or missing membership.
6. After a server restore, a cursor ahead of the server is rejected. Preserve
   local files and pending operations; require a reviewed re-bootstrap/merge
   rather than treating everything local as new or deleting it.
7. The current development Server has a negotiated, atomic vault-version
   conditional push extension. It rejects native pushes based on stale server
   state. Local expected-hash checks protect against stale local tool edits.
   Legacy desktop uploads remain unconditional, so this is not universal CAS.
8. There is no server request deduplication. Persist operation IDs and reconcile
   uncertain acknowledgements against observed revisions before retrying. If
   outcome remains ambiguous, return `indeterminate`; do not promise exactly-once.
9. Preserve base/local/remote versions for known conflicts. Prefer explicit
   conflict resolution over automatic merging in v1. Retain pending local bytes
   through crashes and uploads, and never report a remote race as impossible.

Return structured status distinguishing local persistence, pending upload,
server commit, conflict, locked, revoked and indeterminate outcome. Include
local content identity and observed/applied server revision where available.
Server commit does not mean every desktop has received the change.

## Initial CLI and MCP surface

Proposed CLI groups: `bgh configure/login/logout`, `vault list/create/connect`,
`note list/read/create/edit/move/delete`, `attachment import/export`, `search`,
`sync once/watch/status`, `history list/restore`, `service`, and `mcp`.
Provide JSON output, stdin content, bounded results and stable exit codes.

Initial MCP tools: `vault_list`, `note_list`, `note_read`, `note_create`,
`note_edit`, `note_move`, `note_delete`, `search`, `sync_run`, `sync_status`,
`history_list`, `history_restore`. Attachments can follow once transport and
size behavior are qualified. Use the same operation contracts as the CLI.
Edits accept a local expected content hash and structured edit operations.
Destructive history purge, encryption migration and account administration
are excluded from the initial agent surface. Explicitly bound tool output and
file traversal; reject symlink escapes, special files and ambiguous paths.

The Hermes integration is local stdio MCP with a pinned executable, explicit
profile and tool allowlist. No remote listener is necessary. Add a short skill
for search/read/edit/verify and explaining sync receipts. Automatic conversation
capture and a Hermes memory-provider plugin are separate later features.

## Delivery stages and acceptance gates

| Stage | Work | Acceptance gate |
| --- | --- | --- |
| 0: contract | Freeze source/artifact baselines; document crypto and wire behavior; collect synthetic fixtures | Exact v3 vectors and unsupported-version cases; no copied proprietary implementation or private data |
| 1: native vertical slice | Rust configuration, login, custom-E2EE enrollment, bootstrap and single-file push/pull | Native writes read correctly by reference client; reverse direction; wrong-key/tamper rejection; no secret sent to server |
| 2: durable Sync | SQLite journal, streaming, reconciliation, tombstones, conflicts, restart/reconnect and local locking | Two native clients converge; killed-process recovery at each persistence boundary; uncertain-commit behavior is explicit |
| 3: useful headless client | Note/attachment operations, FTS search, history/restore, stable CLI and local service | Offline edits survive restart; stale local edits fail; external editor changes sync; source-loss recovery and backup restore pass |
| 4: Hermes | Local MCP, shared service calls, bounded tool schemas and usage skill | Actual pinned Hermes discovers tools, reads/searches/edits, handles conflicts/errors and observes commit receipts |
| 5: desktop interoperability | Native plus packaged Bridge clients, collaboration and failure matrix | Bidirectional mixed corpus, rename/delete, revocation/reinvite/self-leave, attribution, restart and recovery pass against exact candidate |
| 6: release | Linux amd64/arm64 builds, container and systemd operation, notices, checksums and provenance | Native/emulated architecture tests identified honestly; reproducibility where supported; no upstream bundle or private data in artifacts |

Stages are dependent in order. A useful first milestone is Stage 1, before
spending effort on the full CLI or MCP surface. Real Hermes validation needs
a known Hermes version and configured model access; use deterministic MCP
contract tests first and report any unavailable live validation honestly.

Additional failure tests: empty/Unicode notes; NFC/NFD path collisions; nested
folders; malformed ciphertext; files crossing piece boundaries; truncated frames;
rename versus edit; delete versus edit; concurrent remote edits; revoke mid-upload;
quota exhaustion; full disk; corrupt local DB; expired session; server rollback;
watcher overflow; two local processes; and repeat tool calls after timeout.

Verify confidentiality with synthetic canaries, key-derivation vectors, captured
test traffic and server storage inspection. Plaintext-absence scans alone are
not proof of cryptographic security. Keys and decrypted test records must never
enter public evidence. Memory/CPU measurements should separate locked idle,
unlocked idle, initial key derivation, initial indexing and steady-state sync;
measure bounded peak memory for large files and many-note vaults before setting
release budgets. Client search resources are distinct from server budgets.

## Server scope and deferred work

The development Server now advertises `conditional_push_v1`; native clients
require that capability and pass the expected vault version at commit. This is
backward-compatible for legacy clients but does not provide exactly-once commit
behavior. Durable idempotency records, uncertain-ack reconciliation, and mixed
desktop races remain open. The server needs no decryption keys for this
extension. A client must never infer support from silently accepted JSON fields.

Deferred: managed encryption enrollment, public-key/PQ key distribution, remote
MCP, offline mobile clients, Windows, UI/plugin execution, Dataview/Bases engines,
Canvas rendering, Publish, semantic/vector search, automatic AI memory capture,
and full desktop UI parity. Markdown, attachments and compatible files remain
portable; storing a plugin's files is not implementing that plugin's behavior.

## Audit verification status

Historical initial-candidate evidence (not qualification of the current
service/journal/conditional-push work):

- `cargo test --offline` passes native crypto, path/host safety, and note
  stale-hash tests on the development Mac.
- `native/tests/interoperability.cjs` passes against local Blackglass Server
  0.6.1 and the pinned Headless reference adapter. It exercises custom-E2EE
  recovery, native-to-reference Sync, edits/deletion, multi-piece binaries,
  wrong-password rejection, conflict preservation, server restart, backup
  verification, rollback rejection, server plaintext canary absence, and a
  local MCP write followed by Sync and automatic MCP polling.
- Hermes Agent v0.21.3 discovers the initial tools through `bgh mcp` in an isolated
  temporary profile. No user Hermes configuration was changed.
- Ubuntu 24.04 ARM64 and AMD64 VMs built separate source-bound 0.1.0 artifacts.
  Both final packaged binaries passed two-profile Sync, deletion, server restart,
  backup verification, and ciphertext-at-rest smoke tests against the verified
  Blackglass Server 0.6.1 Linux release binary. Both checksum manifests pass.
  These are candidate artifacts, not a fully qualified release.
- The initial candidate stored an owner-only profile with token and derived
  key and supported one connected vault. The newer service/journal work has
  not passed the full Stage 2-6 gates or release qualification yet.
- The desktop compatibility matrix records older exact qualified combinations;
  it must not be reused as evidence for this new client. Existing user edits
  in Server and Bridge were not modified.
