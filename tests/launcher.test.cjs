"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { spawnSync } = require("node:child_process");
const test = require("node:test");

const root = path.join(__dirname, "..");
const launcher = path.join(root, "blackglass-headless.cjs");

function run(arguments_, configHome) {
  return spawnSync(process.execPath, [launcher, ...arguments_], {
    cwd: root,
    encoding: "utf8",
    env: { ...process.env, XDG_CONFIG_HOME: configHome, HOME: configHome },
  });
}

test("configures a server and runs the adapted CLI", (context) => {
  const temporary = fs.mkdtempSync(path.join(os.tmpdir(), "blackglass-headless-launcher-"));
  context.after(() => fs.rmSync(temporary, { recursive: true, force: true }));

  const configured = run(["configure", "--server", "http://127.0.0.1:8787"], temporary);
  assert.equal(configured.status, 0, configured.stderr);
  assert.match(configured.stdout, /Blackglass Server: http:\/\/127\.0\.0\.1:8787/);

  const shown = run(["configure", "--show"], temporary);
  assert.equal(shown.status, 0, shown.stderr);
  assert.equal(shown.stdout.trim(), "http://127.0.0.1:8787");

  const version = run(["--version"], temporary);
  assert.equal(version.status, 0, version.stderr);
  assert.equal(version.stdout.trim(), "0.1.0");

  const help = run(["--help"], temporary);
  assert.equal(help.status, 0, help.stderr);
  assert.match(help.stdout, /Logout from Blackglass/);
  assert.doesNotMatch(help.stdout, /publish-list-sites/);

  const commandHelp = run(["help", "sync"], path.join(temporary, "no-config"));
  assert.equal(commandHelp.status, 0, commandHelp.stderr);
  assert.match(commandHelp.stdout, /Usage: blackglass-headless sync/);
});

test("blocks Publish without loading or contacting a service", () => {
  const result = run(["publish-list-sites"], path.join(os.tmpdir(), "blackglass-headless-absent"));
  assert.equal(result.status, 1);
  assert.match(result.stderr, /supports Sync only/);
});

test("requires an explicit server for operational commands", (context) => {
  const temporary = fs.mkdtempSync(path.join(os.tmpdir(), "blackglass-headless-launcher-"));
  context.after(() => fs.rmSync(temporary, { recursive: true, force: true }));
  const result = run(["login"], temporary);
  assert.equal(result.status, 1);
  assert.match(result.stderr, /no Blackglass Server is configured/);
});
