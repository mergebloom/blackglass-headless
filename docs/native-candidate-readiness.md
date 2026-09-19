# Native headless candidate readiness (2026-09-19)

This record describes the independent `bgh` 0.1.0 **candidate**, not a
qualified general-availability release. The source tree is uncommitted; the
binary embeds the SHA-256 of its native-only source archive, not a Git commit.
Do not publish a release or claim full desktop/headless parity from this record.

## Built and tested

- macOS development host: `cargo fmt --check`, 15 Rust tests, `cargo clippy`
  with warnings denied, and a disposable Blackglass Server/reference-client
  custom-E2EE interoperability test passed. The latter covers bidirectional
  Markdown, multi-piece PNG/PDF, edits, deletion, wrong password, conflict
  preservation, restart, backup verify, rollback rejection, server plaintext
  canary absence, and MCP manual/automatic Sync. The MCP test waits for clean
  process exit before attempting profile recovery.
- Hermes Agent 0.21.3 discovered the initial six stdio tools in an isolated temporary
  home. This proves registration, not broad agent-task quality.
  `sync_cancel` was added afterward and has MCP contract coverage but was not
  separately retested through Hermes discovery.
- Ubuntu 24.04 ARM64 and AMD64: native binary plus verified Blackglass Server
  0.6.1 Linux binary passed two-profile note exchange, deletion, restart,
  backup verification, and ciphertext-at-rest smoke tests. Binary checksum
  manifests and embedded source identities passed on both architectures.
  All 19 Linux-specific/native Rust tests passed on each architecture; both
  relocated package checks passed.
  The source-bound packages were rebuilt after the independent review fixes;
  their relocatable archives include the project license and dependency texts.
- Source archive SHA-256 (same on both):
  `f34a19adc2d2b29a4ccbe66e47f2081e989020c7ab9cd43d32a6f8271df306b7`.
- A second isolated source extraction on each architecture produced identical
  executable and binary-archive SHA-256 values to the verified package. This
  proves repeatability on the same pinned builders; cross-environment
  reproducibility has not been established.

An independent GPT-6 Astra read-only review and follow-up review cleared its
identified correctness defects: nonblocking special-file rejection,
same-filesystem history preflight (including nested mounts), durable MCP Sync
progress across cancellation/EOF, and truthful completed-before-cancel results.
The review was focused code review, not a release qualification audit.

| Artifact | SHA-256 |
| --- | --- |
| `native/dist/verified-arm64/bgh-0.1.0-linux-arm64` | `f4a79de788d0ad047599c4c2ca9d2f4ff7da6b051183cbb277236b5af8ca3a73` |
| `native/dist/verified-arm64/bgh-0.1.0-linux-arm64.tar.gz` | `ffa110439cbcf13d7a8ae7446dbe412999a5f07bcfe9aae4065b8659ee082a6b` |
| `native/dist/verified-amd64/bgh-0.1.0-linux-amd64` | `9d36edec278c61371616c4229614c6f418204ae0fe125343c2102f36887fc5bd` |
| `native/dist/verified-amd64/bgh-0.1.0-linux-amd64.tar.gz` | `3d1334f765a4244f6a7dec66955a27bccb23c8fb24949dfed6a2e6eccff85c56` |

The source archive contains only the native implementation, tests, build tools,
README, and license. It excludes the upstream application and bundled JavaScript.
`dependency-licenses.json` and `THIRD-PARTY-LICENSES.txt` are included with
each architecture's binary and checksum manifest. Linux binaries link to Ubuntu 24.04 glibc; older systems
are untested. Nothing was pushed or published.

## Security boundary and release blockers

The storage server receives ciphertext for custom-E2EE vault contents and no
vault password or derived content key. The trusted client profile stores the
account token and derived key with owner-only permissions; the local vault holds
plaintext. Agent tool output may also disclose plaintext to its model provider.
Only custom-E2EE v3 is accepted; managed encryption is deliberately rejected.
Local plaintext history lives beside the vault and must share its filesystem;
enrollment rejects a separately mounted vault root, while Sync rejects nested
mounts before replacing or deleting files there.

This is not yet safe as the sole copy of important data. Before a supported
release, implement and test a durable local operation journal, crash/retry
reconciliation for indeterminate uploads, a persistent background service,
server-restore and source-loss recovery, robust conflict/version handling,
remote-write race protection or an explicitly bounded protocol extension,
collaboration/revocation lifecycle, mixed native/packaged-desktop E2E, and a
full secret-redacted qualification record. The current server protocol has no
conditional write or idempotency key; client-only logic cannot promise
exactly-once uploads. Metadata visibility and malicious-server integrity
limitations are detailed in [the implementation plan](plans/native-client.md).

No Server or Bridge source was changed. Existing dirty worktrees in those
repositories were preserved.
