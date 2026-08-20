#!/usr/bin/env node
"use strict";

const Module = require("node:module");
const path = require("node:path");
const WebSocket = require("ws");
const { loadAndAdapt } = require("./lib/adapter.cjs");
const { canonicalControlOrigin, readServerOrigin, writeServerOrigin } = require("./lib/config.cjs");

function usageError(message) {
  throw new Error(message);
}

function extractServerArgument(arguments_) {
  const remaining = [];
  let server = null;
  for (let index = 0; index < arguments_.length; index += 1) {
    const argument = arguments_[index];
    if (argument === "--server") {
      if (server !== null) usageError("--server may be specified only once");
      if (index + 1 >= arguments_.length) usageError("--server requires an origin");
      server = arguments_[index + 1];
      index += 1;
    } else if (argument.startsWith("--server=")) {
      if (server !== null) usageError("--server may be specified only once");
      server = argument.slice("--server=".length);
    } else {
      remaining.push(argument);
    }
  }
  return { remaining, server };
}

function commandName(arguments_) {
  return arguments_.find((argument) => !argument.startsWith("-")) || null;
}

function runUpstream(arguments_, controlOrigin) {
  process.env.BLACKGLASS_CONTROL_ORIGIN = controlOrigin;
  process.argv = [process.argv[0], __filename, ...arguments_];
  globalThis.WebSocket = class BlackglassWebSocket extends WebSocket {
    constructor(address, protocols) {
      if (protocols === undefined) super(address, [], { origin: "app://obsidian.md" });
      else super(address, protocols, { origin: "app://obsidian.md" });
    }
  };

  const upstreamPath = path.join(__dirname, "cli.js");
  const { source } = loadAndAdapt(upstreamPath);
  const cliModule = new Module(upstreamPath, module);
  cliModule.filename = upstreamPath;
  cliModule.paths = Module._nodeModulePaths(__dirname);
  cliModule._compile(source, upstreamPath);
}

function main() {
  const { remaining, server } = extractServerArgument(process.argv.slice(2));
  const command = commandName(remaining);

  if (command === "configure") {
    const show = remaining.includes("--show");
    const unsupported = remaining.filter((argument) => argument !== "configure" && argument !== "--show");
    if (unsupported.length > 0) usageError(`unknown configure option: ${unsupported[0]}`);
    if (show && server !== null) usageError("use either --show or --server, not both");
    if (show) {
      const current = readServerOrigin();
      if (!current) usageError("no Blackglass Server is configured");
      process.stdout.write(`${current}\n`);
      return;
    }
    if (server === null) usageError("configure requires --server <origin>");
    const saved = writeServerOrigin(server);
    process.stdout.write(`Blackglass Server: ${saved.controlOrigin}\nConfiguration: ${saved.filePath}\n`);
    return;
  }

  if (command && command.startsWith("publish")) {
    usageError("Blackglass Headless supports Sync only; Obsidian Publish commands are disabled");
  }

  const informational = command === null || command === "help" || remaining.includes("--help") || remaining.includes("--version") || remaining.includes("-V");
  const configured = server ?? process.env.BLACKGLASS_CONTROL_ORIGIN ?? readServerOrigin();
  if (!configured && !informational) {
    usageError("no Blackglass Server is configured; run: bgh configure --server https://sync.example.com");
  }
  const controlOrigin = configured ? canonicalControlOrigin(configured) : "http://127.0.0.1:1";
  runUpstream(remaining, controlOrigin);
}

try {
  main();
} catch (error) {
  process.stderr.write(`blackglass-headless: ${error.message}\n`);
  process.exitCode = 1;
}
