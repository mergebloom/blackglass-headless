# Upstream baseline

Blackglass Headless 0.1.0 is pinned to Obsidian Headless 0.0.14 at commit
`0d0ec4364bfde6c715c539cf3555ff8272bb7a58`. The unmodified upstream `cli.js`
has SHA-256 `c6307dc72c00bcf6f22093fb3e0eb91fdc417fc9dd05884ff2c36e5a19cd0196`.

The launcher verifies the complete upstream file and every reviewed semantic
anchor before adapting it in memory. It refuses unknown or changed upstream
code. The tracked upstream bundle is not rewritten.

To qualify a future upstream release:

1. Fetch it from `upstream` and record its release, commit, byte length, and
   SHA-256.
2. Review the endpoint, login Origin, profile, token, user-agent, command-name,
   description, and Publish-disable anchors against the previous baseline.
3. Update only the reviewed offsets, lengths, and hashes in `lib/baseline.cjs`.
4. Run `pnpm check`, then `BLACKGLASS_SERVER_BINARY=/path/to/blackglass-server
   pnpm test:e2e`.
5. Publish only the exact commit and artifacts that passed those checks.

Obsidian Headless and its bundled code remain subject to the terms in the
upstream repository. Blackglass is independent and is not endorsed by Obsidian.
