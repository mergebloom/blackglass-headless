"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");
const {
  canonicalControlOrigin,
  readServerOrigin,
  serverConfigPath,
  writeServerOrigin,
} = require("../lib/config.cjs");

test("canonicalizes secure and loopback control origins", () => {
  assert.equal(canonicalControlOrigin("https://sync.example.com/"), "https://sync.example.com");
  assert.equal(canonicalControlOrigin("https://sync.example.com:8443"), "https://sync.example.com:8443");
  assert.equal(canonicalControlOrigin("http://127.0.0.1:8787"), "http://127.0.0.1:8787");
  assert.equal(canonicalControlOrigin("http://[::1]:8787"), "http://[::1]:8787");
});

test("rejects unsafe or ambiguous control origins", () => {
  for (const value of [
    "http://sync.example.com",
    "https://user:password@sync.example.com",
    "https://sync.example.com/api",
    "https://sync.example.com/?x=1",
    "https://sync.example.com/#fragment",
    " https://sync.example.com",
    "not a URL",
  ]) {
    assert.throws(() => canonicalControlOrigin(value), Error, value);
  }
});

test("writes and reads a private atomic server configuration", (context) => {
  const temporary = fs.mkdtempSync(path.join(os.tmpdir(), "blackglass-headless-config-"));
  context.after(() => fs.rmSync(temporary, { recursive: true, force: true }));
  const environment = { XDG_CONFIG_HOME: temporary };
  const saved = writeServerOrigin("https://sync.example.com/", environment, "linux");
  assert.equal(saved.controlOrigin, "https://sync.example.com");
  assert.equal(readServerOrigin(environment, "linux"), "https://sync.example.com");
  assert.equal(fs.statSync(serverConfigPath(environment, "linux")).mode & 0o777, 0o600);
  assert.equal(fs.statSync(path.dirname(serverConfigPath(environment, "linux"))).mode & 0o777, 0o700);
});

test("rejects a permissive or symlinked configuration", (context) => {
  const temporary = fs.mkdtempSync(path.join(os.tmpdir(), "blackglass-headless-config-"));
  context.after(() => fs.rmSync(temporary, { recursive: true, force: true }));
  const environment = { XDG_CONFIG_HOME: temporary };
  const target = writeServerOrigin("https://sync.example.com", environment, "linux").filePath;
  fs.chmodSync(target, 0o644);
  assert.throws(() => readServerOrigin(environment, "linux"), /permissions/);
  fs.unlinkSync(target);
  const elsewhere = path.join(temporary, "elsewhere");
  fs.writeFileSync(elsewhere, "{}", { mode: 0o600 });
  fs.symlinkSync(elsewhere, target);
  assert.throws(() => readServerOrigin(environment, "linux"), /regular file/);
});
