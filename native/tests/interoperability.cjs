"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const net = require("node:net");
const os = require("node:os");
const path = require("node:path");
const { spawn, spawnSync } = require("node:child_process");
const readline = require("node:readline");
const test = require("node:test");

const root = path.resolve(__dirname, "../..");
const serverBinary = process.env.BLACKGLASS_SERVER_BINARY;
const nativeBinary = process.env.BLACKGLASS_NATIVE_BINARY || path.join(root, "native/target/debug/bgh");

async function port() {
  const server = net.createServer();
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const value = server.address().port;
  await new Promise((resolve) => server.close(resolve));
  return value;
}
function run(command, args, options = {}) {
  const result = spawnSync(command, args, { cwd: root, encoding: "utf8", timeout: 60000, ...options });
  assert.equal(result.status, 0, `${path.basename(command)} ${args.map((x) => x.includes("password") ? "[redacted]" : x).join(" ")} failed: ${result.stderr}`);
  return result.stdout;
}
async function ready(url) {
  for (let i = 0; i < 100; i++) {
    try { if ((await fetch(`${url}/health`)).ok) return; } catch {}
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error("server did not become healthy");
}

test("native and reference clients exchange custom-E2EE notes without server keys", { skip: !serverBinary }, async (t) => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "blackglass-native-e2e-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  const controlPort = await port();
  let dataPort = await port();
  while (dataPort === controlPort) dataPort = await port();
  const origin = `http://127.0.0.1:${controlPort}`;
  const database = path.join(dir, "server.sqlite");
  const serverEnv = { ...process.env, SELFHOST_BIND_HOST: "127.0.0.1", SELFHOST_CONTROL_PORT: String(controlPort),
    SELFHOST_DATA_PORT: String(dataPort), SELFHOST_DATA_HOST: `127.0.0.1:${dataPort}`, SELFHOST_DATABASE: database,
    SELFHOST_STAGING_DIR: path.join(dir, "uploads"), SELFHOST_ALLOWED_ORIGINS: "app://obsidian.md" };
  run(serverBinary, ["user", "create", database, "native@example.test", "Native E2E"], { input: "account-passphrase\n", env: serverEnv });
  let server = spawn(serverBinary, ["serve"], { env: serverEnv, stdio: ["ignore", "pipe", "pipe"] });
  t.after(() => server.kill("SIGINT"));
  await ready(origin);

  const referenceHome = path.join(dir, "reference-home");
  const referenceRoot = path.join(dir, "reference-vault");
  const nativeRoot = path.join(dir, "native-vault");
  fs.mkdirSync(referenceHome, { recursive: true });
  fs.mkdirSync(referenceRoot); fs.mkdirSync(nativeRoot);
  const referenceEnv = { ...process.env, HOME: referenceHome, XDG_CONFIG_HOME: path.join(referenceHome, ".config") };
  const reference = (...args) => run(process.execPath, [path.join(root, "blackglass-headless.cjs"), ...args], { env: referenceEnv });
  const nativeProfile = path.join(dir, "native-profile.json");
  const native = (args, input) => run(nativeBinary, ["--profile", nativeProfile, ...args], { input });

  reference("configure", "--server", origin);
  reference("login", "--email", "native@example.test", "--password", "account-passphrase");
  reference("sync-create-remote", "--name", "Native-E2E", "--encryption", "end-to-end", "--password", "vault-passphrase");
  fs.writeFileSync(path.join(referenceRoot, "reference.md"), "# Written by reference\n");
  const referenceImage = Buffer.alloc(2 * 1024 * 1024 + 127, 0x5a);
  fs.writeFileSync(path.join(referenceRoot, "reference.png"), referenceImage);
  reference("sync-setup", "--vault", "Native-E2E", "--path", referenceRoot, "--password", "vault-passphrase", "--json");
  reference("sync", "--path", referenceRoot);

  native(["configure", "--server", origin]);
  native(["login", "--email", "native@example.test", "--password-stdin"], "account-passphrase\n");
  const id = native(["vault", "list"]).split("\n").find((line) => line.includes("Native-E2E"))?.split("\t")[0];
  assert.ok(id, "native did not list reference-created vault");
  const wrongPassword = spawnSync(nativeBinary, ["--profile", nativeProfile, "vault", "connect", "--id", id, "--path", nativeRoot, "--password-stdin"],
    { input: "wrong-password\n", encoding: "utf8" });
  assert.notEqual(wrongPassword.status, 0);
  assert.match(wrongPassword.stderr, /wrong vault password/);
  native(["vault", "connect", "--id", id, "--path", nativeRoot, "--password-stdin"], "vault-passphrase\n");
  native(["sync", "once"]);
  assert.equal(fs.readFileSync(path.join(nativeRoot, "reference.md"), "utf8"), "# Written by reference\n");
  assert.deepEqual(fs.readFileSync(path.join(nativeRoot, "reference.png")), referenceImage);

  fs.writeFileSync(path.join(nativeRoot, "native.md"), "# Written by native\n");
  native(["sync", "once"]);
  reference("sync", "--path", referenceRoot);
  assert.equal(fs.readFileSync(path.join(referenceRoot, "native.md"), "utf8"), "# Written by native\n");

  fs.writeFileSync(path.join(nativeRoot, "native.md"), "# Edited by native\n");
  native(["sync", "once"]);
  reference("sync", "--path", referenceRoot);
  assert.equal(fs.readFileSync(path.join(referenceRoot, "native.md"), "utf8"), "# Edited by native\n");

  fs.unlinkSync(path.join(nativeRoot, "native.md"));
  native(["sync", "once"]);
  reference("sync", "--path", referenceRoot);
  assert.equal(fs.existsSync(path.join(referenceRoot, "native.md")), false);

  const nativePdf = Buffer.alloc(2 * 1024 * 1024 + 19, 0x25);
  fs.writeFileSync(path.join(nativeRoot, "native.pdf"), nativePdf);
  native(["sync", "once"]);
  reference("sync", "--path", referenceRoot);
  assert.deepEqual(fs.readFileSync(path.join(referenceRoot, "native.pdf")), nativePdf);

  fs.writeFileSync(path.join(nativeRoot, "reference.md"), "# Unsent local edit\n");
  fs.writeFileSync(path.join(referenceRoot, "reference.md"), "# Concurrent remote edit\n");
  reference("sync", "--path", referenceRoot);
  const conflict = spawnSync(nativeBinary, ["--profile", nativeProfile, "sync", "once"], { encoding: "utf8" });
  assert.notEqual(conflict.status, 0);
  assert.match(conflict.stderr, /conflicts with incoming revision/);
  assert.equal(fs.readFileSync(path.join(nativeRoot, "reference.md"), "utf8"), "# Unsent local edit\n");
  fs.writeFileSync(path.join(nativeRoot, "reference.md"), "# Concurrent remote edit\n");
  native(["sync", "once"]);

  fs.unlinkSync(path.join(nativeRoot, "reference.md"));
  fs.writeFileSync(path.join(referenceRoot, "reference.md"), "# Remote edit after local delete\n");
  reference("sync", "--path", referenceRoot);
  const deleteConflict = spawnSync(nativeBinary, ["--profile", nativeProfile, "sync", "once"], { encoding: "utf8" });
  assert.notEqual(deleteConflict.status, 0);
  assert.match(deleteConflict.stderr, /deletion conflicts with incoming revision/);
  assert.equal(fs.existsSync(path.join(nativeRoot, "reference.md")), false);
  fs.writeFileSync(path.join(nativeRoot, "reference.md"), "# Remote edit after local delete\n");
  native(["sync", "once"]);

  const backup = path.join(dir, "backup.sqlite");
  run(serverBinary, ["backup", database, backup], { env: serverEnv });
  run(serverBinary, ["verify", backup], { env: serverEnv });
  server.kill("SIGINT");
  await new Promise((resolve) => server.once("exit", resolve));
  server = spawn(serverBinary, ["serve"], { env: serverEnv, stdio: ["ignore", "pipe", "pipe"] });
  await ready(origin);
  native(["sync", "once"]);
  assert.equal(fs.readFileSync(path.join(nativeRoot, "reference.md"), "utf8"), "# Remote edit after local delete\n");

  const savedProfile = fs.readFileSync(nativeProfile);
  const aheadProfile = JSON.parse(savedProfile);
  aheadProfile.vault.applied_version += 100000;
  fs.writeFileSync(nativeProfile, JSON.stringify(aheadProfile));
  const rollback = spawnSync(nativeBinary, ["--profile", nativeProfile, "sync", "once"], { encoding: "utf8" });
  assert.notEqual(rollback.status, 0);
  assert.match(rollback.stderr, /ahead of the server/);
  assert.equal(fs.readFileSync(path.join(nativeRoot, "reference.md"), "utf8"), "# Remote edit after local delete\n");
  fs.writeFileSync(nativeProfile, savedProfile);

  const serverFiles = [database, `${database}-wal`].filter(fs.existsSync).map((file) => fs.readFileSync(file));
  for (const secret of ["vault-passphrase", "# Written by native", "# Written by reference", "# Edited by native", "# Concurrent remote edit", "# Remote edit after local delete"]) {
    assert.ok(serverFiles.every((file) => !file.includes(Buffer.from(secret))), `server stored plaintext: ${secret}`);
  }

  const mcp = spawn(nativeBinary, ["--profile", nativeProfile, "mcp", "--auto-sync-seconds", "2"], { stdio: ["pipe", "pipe", "pipe"] });
  t.after(() => mcp.kill("SIGTERM"));
  const reader = readline.createInterface({ input: mcp.stdout });
  const responses = [];
  reader.on("line", (line) => responses.push(JSON.parse(line)));
  let requestId = 0;
  async function call(method, params = {}) {
    const id = ++requestId;
    mcp.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id, method, params })}\n`);
    for (let i = 0; i < 100; i++) {
      const index = responses.findIndex((response) => response.id === id);
      if (index !== -1) return responses.splice(index, 1)[0];
      await new Promise((resolve) => setTimeout(resolve, 20));
    }
    throw new Error(`MCP ${method} timed out`);
  }
  const hello = await call("initialize", { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "test", version: "1" } });
  assert.equal(hello.result.serverInfo.name, "blackglass-headless-native");
  const listed = await call("tools/list");
  assert.ok(listed.result.tools.some((tool) => tool.name === "note_write"));
  const badArguments = await call("tools/call", { name: "note_write", arguments: { path: "invalid.md", content: 42 } });
  assert.equal(badArguments.error.code, -32602);
  assert.equal(fs.existsSync(path.join(nativeRoot, "invalid.md")), false);
  const badVersionId = ++requestId;
  mcp.stdin.write(`${JSON.stringify({ jsonrpc: "1.0", id: badVersionId, method: "tools/list" })}\n`);
  for (let i = 0; i < 100 && !responses.some((response) => response.id === badVersionId); i++) {
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  const badVersion = responses.splice(responses.findIndex((response) => response.id === badVersionId), 1)[0];
  assert.equal(badVersion.error.code, -32600);
  mcp.stdin.write("{not valid JSON}\n");
  for (let i = 0; i < 100 && !responses.some((response) => response.id === null); i++) {
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  const parseError = responses.splice(responses.findIndex((response) => response.id === null), 1)[0];
  assert.equal(parseError.error.code, -32700);
  mcp.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized" })}\n`);
  const idleCancel = await call("tools/call", { name: "sync_cancel", arguments: {} });
  assert.equal(idleCancel.result.isError, true);
  const asyncSyncId = ++requestId;
  const pingDuringSyncId = ++requestId;
  mcp.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id: asyncSyncId, method: "tools/call", params: { name: "sync_run", arguments: {} } })}\n${JSON.stringify({ jsonrpc: "2.0", id: pingDuringSyncId, method: "ping" })}\n`);
  for (let i = 0; i < 200 && !(responses.some((response) => response.id === asyncSyncId) && responses.some((response) => response.id === pingDuringSyncId)); i++) {
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  const pingIndex = responses.findIndex((response) => response.id === pingDuringSyncId);
  const syncIndex = responses.findIndex((response) => response.id === asyncSyncId);
  assert.ok(pingIndex >= 0 && syncIndex >= 0);
  assert.ok(pingIndex < syncIndex, "MCP ping was blocked behind Sync");
  assert.equal(responses.splice(syncIndex, 1)[0].result.isError, false);
  responses.splice(responses.findIndex((response) => response.id === pingDuringSyncId), 1);
  const written = await call("tools/call", { name: "note_write", arguments: { path: "agent.md", content: "# Agent note\n" } });
  assert.equal(written.result.isError, false);
  const synced = await call("tools/call", { name: "sync_run", arguments: {} });
  assert.equal(synced.result.isError, false);
  const automatic = await call("tools/call", { name: "note_write", arguments: { path: "automatic.md", content: "# Automatic background Sync\n" } });
  assert.equal(automatic.result.isError, false);
  for (let i = 0; i < 6; i++) {
    await new Promise((resolve) => setTimeout(resolve, 1000));
    reference("sync", "--path", referenceRoot);
    if (fs.existsSync(path.join(referenceRoot, "automatic.md"))) break;
  }
  assert.equal(fs.readFileSync(path.join(referenceRoot, "automatic.md"), "utf8"), "# Automatic background Sync\n");
  mcp.stdin.end();
  await new Promise((resolve, reject) => {
    if (mcp.exitCode !== null) return resolve();
    const timer = setTimeout(() => reject(new Error("MCP did not exit after stdin EOF")), 10000);
    mcp.once("exit", () => { clearTimeout(timer); resolve(); });
  });
  assert.equal(fs.readFileSync(path.join(referenceRoot, "agent.md"), "utf8"), "# Agent note\n");

  const relocatedRoot = path.join(dir, "native-vault-relocated");
  fs.renameSync(nativeRoot, relocatedRoot);
  native(["configure", "--server", origin]);
  native(["login", "--email", "native@example.test", "--password-stdin"], "account-passphrase\n");
  const replacementRoot = path.join(dir, "replacement-vault");
  fs.mkdirSync(replacementRoot);
  native(["vault", "connect", "--id", id, "--path", replacementRoot, "--password-stdin"], "vault-passphrase\n");
  native(["sync", "once"]);
  assert.equal(fs.readFileSync(path.join(replacementRoot, "agent.md"), "utf8"), "# Agent note\n");
});
