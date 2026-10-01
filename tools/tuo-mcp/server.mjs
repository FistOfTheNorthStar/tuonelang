#!/usr/bin/env node
// tuo-mcp — the tuonelang agent protocol as MCP tools.
//
// A thin stdio bridge: it speaks the Model Context Protocol to the coding
// agent on one side and `tuo agent --stdio` (the versioned JSON-lines
// compiler-intelligence protocol, crates/tuo-agent) on the other. Every tool is
// one protocol method; nothing is computed here. One `tuo agent` process lives
// for the bridge's whole life, so the compiler database is reused across calls
// exactly as the protocol intends.
//
// No dependencies: MCP's stdio transport is newline-delimited JSON-RPC 2.0, and
// so is the agent protocol, so both ends are a line reader and JSON.
//
// The `tuo` binary is located by `TUO_BIN`, else `target/debug/tuo` under the
// repository root (two directories up from this file), else `tuo` on PATH.

import { spawn, spawnSync } from "node:child_process";
import { createInterface } from "node:readline";
import { existsSync, readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO = resolve(HERE, "..", "..");
const PROTOCOL_VERSIONS = ["2025-06-18", "2025-03-26", "2024-11-05"];

function locateTuo() {
  if (process.env.TUO_BIN) return process.env.TUO_BIN;
  const local = resolve(REPO, "target", "debug", "tuo");
  return existsSync(local) ? local : "tuo";
}

// ---------------------------------------------------------------------------
// The agent process: one child, requests correlated by id.
// ---------------------------------------------------------------------------

class Agent {
  constructor(bin) {
    this.bin = bin;
    this.nextId = 1;
    this.pending = new Map();
    this.child = null;
  }

  start() {
    if (this.child) return;
    const child = spawn(this.bin, ["agent", "--stdio"], {
      stdio: ["pipe", "pipe", "pipe"],
      cwd: REPO,
    });
    child.on("error", (error) => this.failAll(`cannot start \`${this.bin} agent --stdio\`: ${error.message}`));
    child.on("exit", (code, signal) => {
      this.child = null;
      this.failAll(`\`tuo agent\` exited (code ${code}, signal ${signal})`);
    });
    createInterface({ input: child.stdout }).on("line", (line) => {
      if (!line.trim()) return;
      let response;
      try {
        response = JSON.parse(line);
      } catch {
        return; // not protocol output; the agent keeps stdout clean, so ignore
      }
      const waiter = this.pending.get(response.id);
      if (!waiter) return;
      this.pending.delete(response.id);
      waiter(response);
    });
    child.stderr.on("data", () => {}); // logging only under --log; drain regardless
    this.child = child;
  }

  failAll(message) {
    for (const [, waiter] of this.pending) {
      waiter({ ok: false, error: { code: "unavailable", message } });
    }
    this.pending.clear();
  }

  call(method, params) {
    this.start();
    if (!this.child) {
      return Promise.resolve({ ok: false, error: { code: "unavailable", message: `no \`tuo agent\` process` } });
    }
    const id = this.nextId++;
    const line = JSON.stringify({ protocol_version: 1, id, method, params: params ?? {} });
    return new Promise((done) => {
      this.pending.set(id, done);
      this.child.stdin.write(line + "\n");
    });
  }
}

// ---------------------------------------------------------------------------
// Tool catalogue: one entry per protocol method, plus two conveniences that
// only read the disk or run the CLI (`tuo_open_file`, `tuo_cheatsheet`).
// ---------------------------------------------------------------------------

const S = {
  uri: { type: "string", description: "The document name given to tuo_open (any string; conventionally the file path)." },
  line: { type: "integer", description: "One-based line, as in file:line:col." },
  column: { type: "integer", description: "One-based column, counted in Unicode scalar values." },
};
const positional = {
  type: "object",
  properties: { uri: S.uri, line: S.line, column: S.column },
  required: ["uri", "line", "column"],
};
const byUri = { type: "object", properties: { uri: S.uri }, required: ["uri"] };
const none = { type: "object", properties: {} };

const pos = (a) => ({ uri: a.uri, position: { line: a.line, column: a.column } });

/** @type {Array<{name:string, description:string, inputSchema:object, method?:string, params?:(a:any)=>any, run?:(a:any)=>Promise<any>}>} */
const TOOLS = [
  {
    name: "tuo_open",
    description:
      "Open or update a tuonelang document in the compiler session from inline text. Call this before any per-document query, and again after every edit; the session keeps one incremental compiler database, so re-opening is cheap.",
    inputSchema: {
      type: "object",
      properties: { uri: S.uri, text: { type: "string", description: "The full source text." } },
      required: ["uri", "text"],
    },
    method: "set_document",
    params: (a) => ({ uri: a.uri, text: a.text }),
  },
  {
    name: "tuo_open_file",
    description:
      "Open or update a tuonelang document in the compiler session by reading it from disk. The path becomes its uri. Use after editing a .tuo file on disk.",
    inputSchema: { type: "object", properties: { path: { type: "string", description: "Path to a .tuo file." } }, required: ["path"] },
    run: async (a) => {
      const path = resolve(a.path);
      const text = readFileSync(path, "utf8");
      return agent.call("set_document", { uri: path, text });
    },
  },
  {
    name: "tuo_check",
    description:
      "Run the whole front end (parse, resolve, type check, ownership check) over every open document. Returns accepted, error/warning counts, and every diagnostic with its code, message, and range. Specs are checked, not executed.",
    inputSchema: none,
    method: "check",
    params: () => ({}),
  },
  {
    name: "tuo_verify",
    description:
      "All static checks plus execution of the program's colocated specs on the reference interpreter. Optionally restrict to the specs an edit to one document could affect.",
    inputSchema: { type: "object", properties: { affected_by: { type: "string", description: "Optional uri; run only specs whose dependency closure touches a symbol defined there." } } },
    method: "verify",
    params: (a) => (a.affected_by ? { affected_by: a.affected_by } : {}),
  },
  {
    name: "tuo_format",
    description: "Canonically format tuonelang source (deterministic, idempotent, zero configuration). Pass inline text, or the uri of an open document.",
    inputSchema: { type: "object", properties: { text: { type: "string" }, uri: S.uri } },
    method: "format",
    params: (a) => (a.text !== undefined ? { text: a.text } : { uri: a.uri }),
  },
  { name: "tuo_diagnostics", description: "The diagnostics of one open document.", inputSchema: byUri, method: "diagnostics", params: (a) => ({ uri: a.uri }) },
  { name: "tuo_type_at", description: "The type (and symbol, if any) at a position.", inputSchema: positional, method: "type_at", params: pos },
  { name: "tuo_definition", description: "Where the symbol at a position is defined.", inputSchema: positional, method: "definition", params: pos },
  {
    name: "tuo_references",
    description: "Every reference to the symbol at a position.",
    inputSchema: { ...positional, properties: { ...positional.properties, include_declaration: { type: "boolean", description: "Include the declaration itself (default true)." } } },
    method: "references",
    params: (a) => ({ ...pos(a), include_declaration: a.include_declaration ?? true }),
  },
  { name: "tuo_symbols", description: "The module-level symbols of an open document, with kinds and signatures.", inputSchema: byUri, method: "symbols", params: (a) => ({ uri: a.uri }) },
  { name: "tuo_signature", description: "The signature of the function being called at a position (parameter names, modes, types, return type).", inputSchema: positional, method: "signature", params: pos },
  { name: "tuo_members", description: "The members of the type at a position: today, an enum's variants.", inputSchema: positional, method: "members", params: pos },
  { name: "tuo_available_imports", description: "Every importable symbol across the open documents, with the module path each lives in.", inputSchema: none, method: "available_imports", params: () => ({}) },
  { name: "tuo_specs_for", description: "The colocated specs of the function at a position.", inputSchema: positional, method: "specs_for", params: pos },
  {
    name: "tuo_run_spec",
    description: "Execute specs on the reference interpreter: all of them, or those of one target function. Durations are measured observations, never promises.",
    inputSchema: { type: "object", properties: { target: { type: "string", description: "Optional function name." } } },
    method: "run_spec",
    params: (a) => (a.target ? { target: a.target } : {}),
  },
  {
    name: "tuo_apply_safe_fix",
    description: "Apply the compiler-authored, machine-applicable fixes to a document (for example qualifying a bare builtin name with its one owning module). Only fixes the compiler vouches for; never an invented edit.",
    inputSchema: byUri,
    method: "apply_safe_fix",
    params: (a) => ({ uri: a.uri }),
  },
  // Compiler-guided generation queries: what to write NEXT at a position.
  { name: "tuo_context_at", description: "Generation context at a position: the semantic block (enclosing function, expected type, visible symbols) and a syntactic block that is honestly flagged non-exhaustive.", inputSchema: positional, method: "context_at", params: pos },
  { name: "tuo_expected_type_at", description: "The type the checker expects at a position, and where that expectation came from (the enclosing expression's recorded type, or the enclosing function's declared return type).", inputSchema: positional, method: "expected_type_at", params: pos },
  { name: "tuo_visible_symbols_at", description: "The symbols in scope at a position. An over-approximation (block scoping is not modelled) and says so with complete=false.", inputSchema: positional, method: "visible_symbols_at", params: pos },
  { name: "tuo_valid_members_of", description: "The exact, exhaustive member set of the type at a position (struct fields or enum variants).", inputSchema: positional, method: "valid_members_of", params: pos },
  { name: "tuo_call_signature", description: "The signature of the call being written at a position, for filling in its arguments.", inputSchema: positional, method: "call_signature", params: pos },
  {
    name: "tuo_imports_for_symbol",
    description: "Which module(s) export a symbol of this name, and the import line that would bring it into scope. Use this instead of guessing a module path.",
    inputSchema: { type: "object", properties: { name: { type: "string" } }, required: ["name"] },
    method: "imports_for_symbol",
    params: (a) => ({ name: a.name }),
  },
  { name: "tuo_expected_syntax_at", description: "A conservative lexical guess at what may come next at a position. Always exhaustive=false: the compiler does not enumerate valid next tokens.", inputSchema: positional, method: "expected_syntax_at", params: pos },
  {
    name: "tuo_cheatsheet",
    description:
      "The context-injectable tuonelang language brief (ADR-0018), generated from the compiler: syntax skeleton, the real standard-library signatures, what runs natively versus only on the interpreter, and the anti-pattern table. Read it before writing any tuonelang.",
    inputSchema: none,
    run: async () => {
      const out = spawnSync(agent.bin, ["cheatsheet"], { cwd: REPO, encoding: "utf8", maxBuffer: 1 << 24 });
      if (out.error) return { ok: false, error: { code: "unavailable", message: out.error.message } };
      if (out.status !== 0) return { ok: false, error: { code: "internal", message: out.stderr || `exit ${out.status}` } };
      return { ok: true, text: out.stdout };
    },
  },
];

const agent = new Agent(locateTuo());

// ---------------------------------------------------------------------------
// MCP over stdio: newline-delimited JSON-RPC 2.0.
// ---------------------------------------------------------------------------

function send(message) {
  process.stdout.write(JSON.stringify(message) + "\n");
}

function reply(id, result) {
  send({ jsonrpc: "2.0", id, result });
}

function replyError(id, code, message) {
  send({ jsonrpc: "2.0", id, error: { code, message } });
}

function toolResult(response) {
  if (response.ok) {
    const text = typeof response.text === "string" ? response.text : JSON.stringify(response.result, null, 2);
    return { content: [{ type: "text", text }] };
  }
  const { code, message, data } = response.error ?? {};
  const detail = data === undefined ? "" : `\n${JSON.stringify(data)}`;
  return { content: [{ type: "text", text: `${code ?? "error"}: ${message ?? "unknown error"}${detail}` }], isError: true };
}

async function handle(message) {
  const { id, method, params } = message;
  const isRequest = id !== undefined && id !== null;
  switch (method) {
    case "initialize": {
      const asked = params?.protocolVersion;
      const version = PROTOCOL_VERSIONS.includes(asked) ? asked : PROTOCOL_VERSIONS[0];
      return reply(id, {
        protocolVersion: version,
        capabilities: { tools: {} },
        serverInfo: { name: "tuo-mcp", version: "0.1.0" },
        instructions:
          "tuonelang compiler intelligence. Open a document with tuo_open or tuo_open_file, then query it; call tuo_check after every edit. Call tuo_cheatsheet before writing tuonelang from scratch.",
      });
    }
    case "notifications/initialized":
    case "notifications/cancelled":
      return;
    case "ping":
      return reply(id, {});
    case "tools/list":
      return reply(id, { tools: TOOLS.map(({ name, description, inputSchema }) => ({ name, description, inputSchema })) });
    case "tools/call": {
      const tool = TOOLS.find((t) => t.name === params?.name);
      if (!tool) return replyError(id, -32602, `unknown tool ${params?.name}`);
      const args = params?.arguments ?? {};
      try {
        const response = tool.run ? await tool.run(args) : await agent.call(tool.method, tool.params(args));
        return reply(id, toolResult(response));
      } catch (error) {
        return reply(id, { content: [{ type: "text", text: String(error?.message ?? error) }], isError: true });
      }
    }
    default:
      if (isRequest) replyError(id, -32601, `method not found: ${method}`);
  }
}

createInterface({ input: process.stdin }).on("line", (line) => {
  if (!line.trim()) return;
  let message;
  try {
    message = JSON.parse(line);
  } catch {
    return replyError(null, -32700, "parse error");
  }
  handle(message).catch((error) => {
    if (message.id !== undefined) replyError(message.id, -32603, String(error?.message ?? error));
  });
});

process.stdin.on("end", () => {
  if (agent.child) agent.child.stdin.end();
  process.exit(0);
});
