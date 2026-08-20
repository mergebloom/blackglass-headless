"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const net = require("node:net");
const os = require("node:os");
const path = require("node:path");
const { spawn, spawnSync } = require("node:child_process");
const test = require("node:test");

const root = path.join(__dirname, "..");
const launcher = path.join(root, "blackglass-headless.cjs");
const serverBinary = process.env.BLACKGLASS_SERVER_BINARY;

function availablePort() {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.unref();
    server.on("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      server.close((error) => {
        if (error) reject(error);
        else resolve(address.port);
      });
    });
  });
}

function run(command, arguments_, options = {}) {
  const result = spawnSync(command, arguments_, {
    cwd: options.cwd || root,
    encoding: "utf8",
    env: options.env || process.env,
    input: options.input,
    timeout: 30_000,
  });
  assert.equal(
    result.status,
    0,
    `${command} ${redactArguments(arguments_).join(" ")} failed\nstdout:\n${result.stdout}\nstderr:\n${result.stderr}`,
  );
  return result;
}

function redactArguments(arguments_) {
  let redactNext = false;
  return arguments_.map((argument) => {
    if (redactNext) {
      redactNext = false;
      return "[REDACTED]";
    }
    if (argument === "--password") {
      redactNext = true;
      return argument;
    }
    if (argument.startsWith("--password=")) return "--password=[REDACTED]";
    return argument;
  });
}

function clientEnvironment(home) {
  return {
    ...process.env,
    HOME: home,
    XDG_CONFIG_HOME: path.join(home, ".config"),
  };
}

function runClient(home, arguments_) {
  return run(process.execPath, [launcher, ...arguments_], { env: clientEnvironment(home) });
}

function serverEnvironment(directory, controlPort, dataPort) {
  return {
    ...process.env,
    SELFHOST_BIND_HOST: "127.0.0.1",
    SELFHOST_CONTROL_PORT: String(controlPort),
    SELFHOST_DATA_PORT: String(dataPort),
    SELFHOST_DATA_HOST: `127.0.0.1:${dataPort}`,
    SELFHOST_DATABASE: path.join(directory, "server.sqlite"),
    SELFHOST_STAGING_DIR: path.join(directory, "uploads"),
    SELFHOST_ALLOWED_ORIGINS: "app://obsidian.md",
    SELFHOST_LOG_FORMAT: "pretty",
    RUST_LOG: "info",
  };
}

async function waitFor(predicate, description, milliseconds = 10_000) {
  const deadline = Date.now() + milliseconds;
  while (Date.now() < deadline) {
    if (await predicate()) return;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error(`timed out waiting for ${description}`);
}

async function startServer(environment) {
  const child = spawn(serverBinary, ["serve"], {
    cwd: root,
    env: environment,
    stdio: ["ignore", "pipe", "pipe"],
  });
  let output = "";
  child.stdout.on("data", (chunk) => { output += chunk; });
  child.stderr.on("data", (chunk) => { output += chunk; });
  await waitFor(async () => {
    if (child.exitCode !== null) throw new Error(`server exited early:\n${output}`);
    try {
      const response = await fetch(`http://127.0.0.1:${environment.SELFHOST_CONTROL_PORT}/health`);
      return response.ok;
    } catch {
      return false;
    }
  }, "Blackglass Server health");
  return { child, output: () => output };
}

async function stopServer(server) {
  if (!server || server.child.exitCode !== null) return;
  server.child.kill("SIGINT");
  await waitFor(() => server.child.exitCode !== null, "Blackglass Server shutdown");
  assert.equal(server.child.exitCode, 0, server.output());
}

test("Blackglass Headless completes the no-GUI Sync lifecycle", { skip: !serverBinary }, async (context) => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "blackglass-headless-server-e2e-"));
  context.after(() => fs.rmSync(directory, { recursive: true, force: true }));

  const controlPort = await availablePort();
  let dataPort = await availablePort();
  while (dataPort === controlPort) dataPort = await availablePort();
  const environment = serverEnvironment(directory, controlPort, dataPort);
  const database = environment.SELFHOST_DATABASE;
  run(serverBinary, ["user", "create", database, "headless@example.test", "Headless E2E"], {
    input: "headless-account-password\n",
  });

  let server = await startServer(environment);
  let continuous = null;
  try {
    const homeA = path.join(directory, "home-a");
    const homeB = path.join(directory, "home-b");
    const vaultA = path.join(directory, "vault-a");
    const vaultB = path.join(directory, "vault-b");
    for (const item of [homeA, homeB, vaultA, vaultB]) fs.mkdirSync(item, { recursive: true });

    const markdown = Buffer.from("# Blackglass Headless E2E\n\nNo GUI required.\n", "utf8");
    const binary = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x42, 0x47]);
    fs.writeFileSync(path.join(vaultA, "note.md"), markdown);
    fs.writeFileSync(path.join(vaultA, "image.png"), binary);

    const origin = `http://127.0.0.1:${controlPort}`;
    for (const home of [homeA, homeB]) {
      runClient(home, ["configure", "--server", origin]);
      runClient(home, ["login", "--email", "headless@example.test", "--password", "headless-account-password"]);
    }

    runClient(homeA, [
      "sync-create-remote",
      "--name", "Headless-E2E",
      "--encryption", "end-to-end",
      "--password", "headless-vault-password",
    ]);
    runClient(homeA, [
      "sync-setup",
      "--vault", "Headless-E2E",
      "--path", vaultA,
      "--password", "headless-vault-password",
      "--device-name", "headless-a",
      "--json",
    ]);
    runClient(homeA, ["sync", "--path", vaultA]);

    runClient(homeB, [
      "sync-setup",
      "--vault", "Headless-E2E",
      "--path", vaultB,
      "--password", "headless-vault-password",
      "--device-name", "headless-b",
      "--json",
    ]);
    runClient(homeB, ["sync", "--path", vaultB]);
    assert.deepEqual(fs.readFileSync(path.join(vaultB, "note.md")), markdown);
    assert.deepEqual(fs.readFileSync(path.join(vaultB, "image.png")), binary);

    continuous = spawn(process.execPath, [launcher, "sync", "--continuous", "--path", vaultB], {
      cwd: root,
      env: clientEnvironment(homeB),
      stdio: ["ignore", "pipe", "pipe"],
    });
    let continuousOutput = "";
    continuous.stdout.on("data", (chunk) => { continuousOutput += chunk; });
    continuous.stderr.on("data", (chunk) => { continuousOutput += chunk; });
    await waitFor(() => continuousOutput.includes("Fully synced"), "continuous client readiness");

    const background = Buffer.from("# Background Sync\n", "utf8");
    fs.writeFileSync(path.join(vaultB, "background.md"), background);
    await waitFor(() => continuousOutput.includes("Upload complete background.md"), "background upload");
    runClient(homeA, ["sync", "--path", vaultA]);
    assert.deepEqual(fs.readFileSync(path.join(vaultA, "background.md")), background);

    const returned = Buffer.from("# Return path\n", "utf8");
    fs.writeFileSync(path.join(vaultA, "return.md"), returned);
    runClient(homeA, ["sync", "--path", vaultA]);
    await waitFor(() => fs.existsSync(path.join(vaultB, "return.md")), "continuous download");
    assert.deepEqual(fs.readFileSync(path.join(vaultB, "return.md")), returned);

    fs.unlinkSync(path.join(vaultA, "return.md"));
    runClient(homeA, ["sync", "--path", vaultA]);
    await waitFor(() => !fs.existsSync(path.join(vaultB, "return.md")), "continuous deletion");

    continuous.kill("SIGINT");
    await waitFor(() => continuous.exitCode !== null, "continuous client shutdown");
    assert.equal(continuous.exitCode, 0, continuousOutput);
    continuous = null;

    await stopServer(server);
    server = await startServer(environment);
    runClient(homeA, ["sync-list-remote", "--json"]);
    runClient(homeA, ["sync", "--path", vaultA]);

    const backup = path.join(directory, "backup.sqlite");
    run(serverBinary, ["backup", database, backup]);
    run(serverBinary, ["verify", backup]);
    run(serverBinary, ["verify", database]);
  } finally {
    if (continuous && continuous.exitCode === null) continuous.kill("SIGKILL");
    await stopServer(server);
  }
});
