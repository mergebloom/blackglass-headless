"use strict";

const crypto = require("node:crypto");
const fs = require("node:fs");
const path = require("node:path");
const { BASELINE } = require("./baseline.cjs");

function sha256(value) {
  return crypto.createHash("sha256").update(value).digest("hex");
}

function fail(message) {
  throw new Error(`Compatibility check failed: ${message}`);
}

function adaptSource(source, baseline = BASELINE) {
  const sourceBuffer = Buffer.from(source, "utf8");
  if (sourceBuffer.byteLength !== baseline.upstream.bytes) {
    fail(`expected ${baseline.upstream.bytes} bytes, received ${sourceBuffer.byteLength}`);
  }
  const sourceHash = sha256(sourceBuffer);
  if (sourceHash !== baseline.upstream.sha256) {
    fail(`upstream SHA-256 is ${sourceHash}, expected ${baseline.upstream.sha256}`);
  }

  const ordered = [...baseline.incisions].sort((left, right) => right.offset - left.offset);
  let adapted = source;
  for (const incision of ordered) {
    const before = source.slice(incision.offset, incision.offset + incision.length);
    const beforeHash = sha256(before);
    if (before.length !== incision.length || beforeHash !== incision.beforeSha256) {
      fail(`${incision.name} semantic anchor did not match the reviewed baseline`);
    }
    adapted = `${adapted.slice(0, incision.offset)}${incision.replacement}${adapted.slice(incision.offset + incision.length)}`;
  }

  return {
    source: adapted,
    receipt: {
      schemaVersion: baseline.schemaVersion,
      upstream: { ...baseline.upstream },
      adaptedSha256: sha256(adapted),
      incisions: baseline.incisions.map(({ name, offset, length, beforeSha256, replacement }) => ({
        name,
        offset,
        length,
        beforeSha256,
        replacementSha256: sha256(replacement),
      })),
    },
  };
}

function loadAndAdapt(upstreamPath = path.join(__dirname, "..", BASELINE.upstream.path)) {
  const stat = fs.lstatSync(upstreamPath);
  if (!stat.isFile() || stat.isSymbolicLink()) {
    fail(`${upstreamPath} must be a regular file`);
  }
  return adaptSource(fs.readFileSync(upstreamPath, "utf8"));
}

module.exports = { adaptSource, loadAndAdapt, sha256 };
