#!/usr/bin/env node
// Smoke test for tuo-mcp: drives the bridge over its real stdio transport and
// checks that MCP requests reach `tuo agent` and come back as tool results.
// Run: node tools/tuo-mcp/test.mjs   (needs a built `tuo`; see server.mjs)

import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import assert from "node:assert/strict";

const server = resolve(dirname(fileURLToPath(import.meta.url)), "server.mjs");
const child = spawn(process.execPath, [server], { stdio: ["pipe", "pipe", "inherit"] });
const waiting = new Map();
createInterface({ input: child.stdout }).on("line", (line) => {
  const msg = JSON.parse(line);
  waiting.get(msg.id)?.(msg);
});
let next = 1;
const rpc = (method, params) =>
  new Promise((done) => {
    const id = next++;
    waiting.set(id, done);
    child.stdin.write(JSON.stringify({ jsonrpc: "2.0", id, method, params }) + "\n");
  });
const text = (r) => JSON.parse(r.result.content[0].text);

const init = await rpc("initialize", { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "test", version: "0" } });
assert.equal(init.result.serverInfo.name, "tuo-mcp");

const list = await rpc("tools/list", {});
const names = list.result.tools.map((t) => t.name);
assert.ok(names.includes("tuo_check") && names.includes("tuo_expected_type_at"), names);

const program = "fn helper(in n: Int) -> Int {\n    n + 1\n}\n\nfn main() -> Int {\n    helper(1)\n}\n";
await rpc("tools/call", { name: "tuo_open", arguments: { uri: "t.tuo", text: program } });

const check = text(await rpc("tools/call", { name: "tuo_check", arguments: {} }));
assert.equal(check.accepted, true, JSON.stringify(check));

const broken = program.replace("helper(1)", "gone()");
await rpc("tools/call", { name: "tuo_open", arguments: { uri: "t.tuo", text: broken } });
const recheck = text(await rpc("tools/call", { name: "tuo_check", arguments: {} }));
assert.equal(recheck.accepted, false);
assert.equal(recheck.diagnostics[0].code, "R0002", JSON.stringify(recheck.diagnostics));

const visible = text(await rpc("tools/call", { name: "tuo_visible_symbols_at", arguments: { uri: "t.tuo", line: 6, column: 5 } }));
assert.ok(visible.visible_symbols.some((s) => s.name === "helper"));

const bad = await rpc("tools/call", { name: "tuo_diagnostics", arguments: { uri: "nope.tuo" } });
assert.equal(bad.result.isError, true);

const sheet = await rpc("tools/call", { name: "tuo_cheatsheet", arguments: {} });
assert.ok(sheet.result.content[0].text.includes("tuonelang"));

child.stdin.end();
console.log("tuo-mcp: ok");
