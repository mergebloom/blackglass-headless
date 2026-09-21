# Native service candidate evidence — 2026-09-20

This record applies to the Rust client and Server changes promoted as
Blackglass Headless 0.2.0. It is not a desktop compatibility claim. The
previous [candidate record](native-candidate-readiness.md) describes different
binaries and remains historical evidence.

## Implemented in this candidate

- An owner-only SQLite applied-cursor/file-inventory journal checkpoints Sync
  transitions and rejects corrupt state, symlinked state, and backwards cursor
  updates. The JSON profile is no longer authoritative after journal creation.
- An owner-only Unix-socket service keeps Sync running independently of an MCP
  process. CLI and stdio MCP calls proxy through its single engine; bounded
  retries cover transient failures. Linux peers must have the service UID.
- Server advertises `conditional_push_v1`. Native pushes compare the expected
  vault version inside the commit transaction and receive a committed UID;
  legacy push responses remain unchanged. The client refuses a Server without
  the capability.
- Replay E2E rewinds the journal after a downloaded file, a propagated
  deletion, and a committed upload with a simulated lost receipt. Retry
  converges without duplicating the committed upload. Restore E2E verifies a
  stale profile is rejected without losing local bytes, while a fresh profile
  connects to the rotated vault and recovers backed-up files.

## Validation completed

- Native: 19 Rust tests on macOS and 23 Linux-specific/native tests on each
  Ubuntu architecture, plus formatting and Clippy with warnings denied.
- Reference-client interoperability: custom-E2EE bidirectional Markdown and
  multi-piece files, deletion and conflict preservation, wrong-password
  rejection, MCP and service lifecycle, backup/restart, and real restore with
  clean-profile recovery.
- Server: 106 Rust tests and 184 Bun tests; release metadata, distribution,
  third-party notices, TypeScript, formatting, and Clippy gates passed.
- Reference adapter: 10 tests and exact-upstream verification passed.
- Hermes Agent 0.21.3 discovered all seven stdio MCP tools through its real
  `mcp add` and `mcp test` commands in an isolated temporary home. No model
  response or live agent task was requested.
- Ubuntu 24.04 arm64 and amd64: separate source-bound Linux packages pass
  relocation checks and a disposable two-profile packaged-client smoke test
  against current Server source. It covers bidirectional notes, deletion,
  restart, verified backup, ciphertext absence, persistent service background
  Sync, stale-socket recovery after a forced service kill, and clean socket
  shutdown. Same-builder rebuilds yield byte-identical
  executables and binary archives. This is repeatability on those builders,
  not cross-environment reproducibility.
- Idle service RSS during the release smoke runs: 7,084 KiB (arm64) and 14,540 KiB
  (amd64). These are observations, not resource budgets or peak measurements.

| Asset | SHA-256 |
| --- | --- |
| Source archive, both architectures | `e1c0018809a10b2aa3934858daf68af76f34ae858bd616ad6e9da65924aa210d` |
| `native/dist/dev-current-arm64/bgh-0.2.0-linux-arm64` | `a45906f66b8e6169586b22df32ab97960dd6869b2874004d5abdf30c20f86f3e` |
| `native/dist/dev-current-arm64/bgh-0.2.0-linux-arm64.tar.gz` | `406da2044413e5f998d546fda1839b6153e73f55c9df57b34588a71c68199c26` |
| `native/dist/dev-current-amd64/bgh-0.2.0-linux-amd64` | `e7083940c898b8b2547e08039c7939ed55ddbb17bf7ad36530bf9a4f385e44ac` |
| `native/dist/dev-current-amd64/bgh-0.2.0-linux-amd64.tar.gz` | `0609ccd0e43e6c4ca6d77495aea01cfa4d12781037dfba63f5a5bddc2ce301a4` |

## Known limitations

No durable server-side idempotency key or general lost-ack protocol exists;
the tested same-content replay case does not establish exactly-once behavior
for every interleaving.
Legacy desktop writes are still unconditional. The journal has not passed a
kill-at-every-transition fault matrix or large-vault resource tests. Native
collaboration/revocation and mixed packaged-desktop E2E are not qualified.
The current service has not passed a live Hermes agent-task matrix. CLI/MCP operations remain a
small note/search/Sync subset; attachments, move/delete tools, history restore,
and structured conflict handling are not complete.
