"use strict";

const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const CONFIG_NAME = "blackglass-headless";
const MAX_CONFIG_BYTES = 16 * 1024;

function configDirectory(environment = process.env, platform = process.platform) {
  if (platform === "linux") {
    return path.join(environment.XDG_CONFIG_HOME || path.join(os.homedir(), ".config"), CONFIG_NAME);
  }
  return path.join(os.homedir(), `.${CONFIG_NAME}`);
}

function canonicalControlOrigin(input) {
  if (typeof input !== "string" || input.trim() !== input || input.length === 0) {
    throw new Error("server origin must be a non-empty URL without surrounding whitespace");
  }

  let url;
  try {
    url = new URL(input);
  } catch {
    throw new Error("server origin must be a valid absolute URL");
  }

  if (url.username || url.password) throw new Error("server origin must not contain credentials");
  if (url.pathname !== "/" || url.search || url.hash) {
    throw new Error("server origin must not contain a path, query, or fragment");
  }

  const loopback = url.hostname === "localhost" || url.hostname === "127.0.0.1" || url.hostname === "[::1]";
  if (url.protocol !== "https:" && !(url.protocol === "http:" && loopback)) {
    throw new Error("server origin must use HTTPS; HTTP is allowed only for loopback testing");
  }

  return url.origin;
}

function assertPrivateRegularFile(filePath, stat) {
  if (!stat.isFile() || stat.isSymbolicLink()) throw new Error(`${filePath} must be a regular file`);
  if (typeof process.getuid === "function" && stat.uid !== process.getuid()) {
    throw new Error(`${filePath} is not owned by the current user`);
  }
  if ((stat.mode & 0o077) !== 0) throw new Error(`${filePath} permissions must be 0600 or stricter`);
  if (stat.size > MAX_CONFIG_BYTES) throw new Error(`${filePath} exceeds ${MAX_CONFIG_BYTES} bytes`);
}

function ensurePrivateDirectory(directory) {
  try {
    const stat = fs.lstatSync(directory);
    if (!stat.isDirectory() || stat.isSymbolicLink()) throw new Error(`${directory} must be a directory, not a symlink`);
    if (typeof process.getuid === "function" && stat.uid !== process.getuid()) {
      throw new Error(`${directory} is not owned by the current user`);
    }
    fs.chmodSync(directory, 0o700);
  } catch (error) {
    if (error.code !== "ENOENT") throw error;
    fs.mkdirSync(directory, { recursive: true, mode: 0o700 });
    fs.chmodSync(directory, 0o700);
  }
}

function serverConfigPath(environment = process.env, platform = process.platform) {
  return path.join(configDirectory(environment, platform), "server.json");
}

function readServerOrigin(environment = process.env, platform = process.platform) {
  const filePath = serverConfigPath(environment, platform);
  let stat;
  try {
    stat = fs.lstatSync(filePath);
  } catch (error) {
    if (error.code === "ENOENT") return null;
    throw error;
  }
  assertPrivateRegularFile(filePath, stat);
  const parsed = JSON.parse(fs.readFileSync(filePath, "utf8"));
  if (parsed.schemaVersion !== 1 || typeof parsed.controlOrigin !== "string") {
    throw new Error(`${filePath} has an unsupported format`);
  }
  return canonicalControlOrigin(parsed.controlOrigin);
}

function writeServerOrigin(origin, environment = process.env, platform = process.platform) {
  const controlOrigin = canonicalControlOrigin(origin);
  const directory = configDirectory(environment, platform);
  const filePath = serverConfigPath(environment, platform);
  ensurePrivateDirectory(directory);

  try {
    const existing = fs.lstatSync(filePath);
    assertPrivateRegularFile(filePath, existing);
  } catch (error) {
    if (error.code !== "ENOENT") throw error;
  }

  const temporary = path.join(directory, `.server.json.${process.pid}.${Date.now()}.tmp`);
  const body = `${JSON.stringify({ schemaVersion: 1, controlOrigin }, null, 2)}\n`;
  try {
    fs.writeFileSync(temporary, body, { encoding: "utf8", flag: "wx", mode: 0o600 });
    fs.renameSync(temporary, filePath);
    fs.chmodSync(filePath, 0o600);
  } finally {
    try {
      fs.unlinkSync(temporary);
    } catch (error) {
      if (error.code !== "ENOENT") throw error;
    }
  }
  return { controlOrigin, filePath };
}

module.exports = {
  canonicalControlOrigin,
  configDirectory,
  readServerOrigin,
  serverConfigPath,
  writeServerOrigin,
};
