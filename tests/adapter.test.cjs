"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");
const { adaptSource, loadAndAdapt, sha256 } = require("../lib/adapter.cjs");
const { BASELINE } = require("../lib/baseline.cjs");

const upstreamPath = path.join(__dirname, "..", "cli.js");

test("adapts the exact reviewed upstream source", () => {
  const result = loadAndAdapt(upstreamPath);
  assert.equal(result.receipt.upstream.sha256, BASELINE.upstream.sha256);
  assert.equal(result.receipt.incisions.length, BASELINE.incisions.length);
  for (const incision of BASELINE.incisions) {
    assert.ok(result.source.includes(incision.replacement), incision.name);
  }
  assert.doesNotThrow(() => new vm.Script(result.source.replace(/^#!.*\n/, "")));
});

test("fails closed when the upstream file changes", () => {
  const source = fs.readFileSync(upstreamPath, "utf8");
  const changed = `${source.slice(0, -1)}${source.endsWith("\n") ? " " : "\n"}`;
  assert.throws(() => adaptSource(changed), /Compatibility check failed: upstream SHA-256/);
});

test("fails closed when a reviewed semantic anchor does not match", () => {
  const source = fs.readFileSync(upstreamPath, "utf8");
  const baseline = {
    ...BASELINE,
    upstream: { ...BASELINE.upstream, sha256: sha256(source) },
    incisions: [{ ...BASELINE.incisions[0], beforeSha256: "0".repeat(64) }],
  };
  assert.throws(() => adaptSource(source, baseline), /semantic anchor did not match/);
});
