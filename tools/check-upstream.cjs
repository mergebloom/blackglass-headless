#!/usr/bin/env node
"use strict";

const fs = require("node:fs");
const path = require("node:path");
const { loadAndAdapt } = require("../lib/adapter.cjs");

const root = path.join(__dirname, "..");
const result = loadAndAdapt(path.join(root, "cli.js"));

const forbidden = [
  new RegExp(["be", "ai", "ni"].join(""), "i"),
  new RegExp(`(?:[a-z0-9-]+\\.)*${["mk", "na"].join("")}\\.ca`, "i"),
];
const inspect = [];
const textExtensions = new Set([".cjs", ".json", ".md", ".yaml", ".yml"]);
function collect(directory) {
  for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
    if ([".git", "btime", "node_modules"].includes(entry.name)) continue;
    const absolute = path.join(directory, entry.name);
    if (entry.isDirectory()) collect(absolute);
    else if (entry.name !== "cli.js" && (textExtensions.has(path.extname(entry.name)) || entry.name === "pnpm-lock.yaml")) {
      inspect.push(path.relative(root, absolute));
    }
  }
}
collect(root);
for (const relative of inspect) {
  const body = fs.readFileSync(path.join(root, relative), "utf8");
  for (const pattern of forbidden) {
    if (pattern.test(body)) throw new Error(`forbidden private identifier in ${relative}`);
  }
}

process.stdout.write(`${JSON.stringify(result.receipt, null, 2)}\n`);
