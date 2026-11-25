// A Debug Adapter Protocol session over `dump-analyzer serve`: VS Code's
// threads, call stack, variables, and debug console panes, the
// disassembly view, and memory, for a crash dump. Nothing here imports
// `vscode`, so that it runs (and is tested) under Node alone.
//
// A dump doesn't run: the session stops once, at the dump's exception
// (or its first thread), and every request is a question the server
// answers about the dump.
"use strict";

/** Frames' and variables' references: small integers for DAP, each
 * standing for what it refers to. A dump's answers never change, so a
 * reference stays good for the session. */
class Handles {
  constructor() {
    this.next = 1;
    this.byKey = new Map();
    this.byId = new Map();
  }
  get(value) {
    const key = JSON.stringify(value);
    let id = this.byKey.get(key);
    if (id === undefined) {
      id = this.next++;
      this.byKey.set(key, id);
      this.byId.set(id, value);
    }
    return id;
  }
  lookup(id) {
    return this.byId.get(id);
  }
}

/** What a frame is called in the call stack. */
function frameName(f) {
  const module = f.module || "?";
  const name = f.function ? `${module}!${f.function}+${f.offset}` : `${module}!${f.pc}`;
  return f.confidence === "likely" ? `${name} (likely)` : name;
}

/** A value's text in the variables pane. */
function valueText(v) {
  if (!v) {
    return "";
  }
  if (v.text) {
    return v.text;
  }
  if (v.pointer) {
    return v.pointer;
  }
  if (v.children && v.children.length) {
    return `{${v.type}}`;
  }
  return "";
}

class DumpSession {
  /**
   * @param {(args: string[]) => Promise<any>} request runs a dump-analyzer
   *   command line on the server, and resolves with its JSON.
   * @param {(message: object) => void} send sends a DAP message to VS Code.
   */
  constructor(request, send) {
    this.request = request;
    this.send = send;
    this.seq = 1;
    this.dump = null;
    this.options = [];
    this.frames = new Handles();
    this.vars = new Handles();
    this.stopped = null;
  }

  emit(message) {
    this.send({ seq: this.seq++, ...message });
  }

  event(event, body) {
    this.emit({ type: "event", event, body });
  }

  respond(req, body, error) {
    const r = { type: "response", request_seq: req.seq, command: req.command, success: !error };
    if (error) {
      r.message = String(error.message || error);
    } else if (body !== undefined) {
      r.body = body;
    }
    this.emit(r);
  }

  /** Runs a command on the session's dump. */
  ask(command, ...args) {
    return this.request([command, this.dump, ...args, ...this.options]);
  }

  /** Handles a DAP request (answers it, and sends any events). */
  async handle(req) {
    try {
      const handler = this["on_" + req.command];
      if (!handler) {
        this.respond(req, undefined, new Error(`${req.command} isn't supported for dumps`));
        return;
      }
      this.respond(req, await handler.call(this, req.arguments || {}));
      if (req.command === "launch") {
        this.event("initialized", {});
      } else if (req.command === "configurationDone") {
        await this.stop();
      }
    } catch (e) {
      this.respond(req, undefined, e);
    }
  }

  async stop() {
    const summary = await this.ask("summary");
    const e = summary.exception;
    const threads = await this.ask("threads");
    const thread = e ? e.thread : threads.length ? threads[0].id : 0;
    this.stopped = thread;
    const text = e ? `${e.name} at ${e.address}` : "dump";
    this.event("stopped", {
      reason: e ? "exception" : "pause",
      description: text,
      text,
      threadId: thread,
      allThreadsStopped: true,
    });
    this.event("output", {
      category: "console",
      output: `${summary.os || "?"} ${summary.arch} dump: ${summary.threads} threads, ${summary.modules} modules (${summary.images_found} images and ${summary.debug_found} with debug information found)\n` +
        (e ? `${e.name} at ${e.address} on thread ${e.thread}\n` : ""),
    });
  }

  on_initialize() {
    return {
      supportsConfigurationDoneRequest: true,
      supportsDisassembleRequest: true,
      supportsReadMemoryRequest: true,
      supportsInstructionBreakpoints: false,
      supportsTerminateRequest: true,
      supportsEvaluateForHovers: false,
      supportsSteppingGranularity: false,
      exceptionBreakpointFilters: [],
    };
  }

  on_launch(args) {
    if (!args.dump) {
      throw new Error("no dump given (launch.json's \"dump\")");
    }
    this.dump = args.dump;
    this.options = [];
    for (const d of [].concat(args.symbols || [])) {
      this.options.push("-symbols", String(d));
    }
    for (const d of [].concat(args.sysroot || [])) {
      this.options.push("-sysroot", String(d));
    }
    return {};
  }

  on_configurationDone() {
    return {};
  }

  async on_threads() {
    const threads = await this.ask("threads");
    return {
      threads: threads.map((t) => {
        const where = t.frame ? ` ${frameName(t.frame)}` : "";
        const name = t.name ? ` (${t.name})` : "";
        return { id: t.id, name: `${t.id}${name}${where}` };
      }),
    };
  }

  async on_stackTrace(args) {
    const st = await this.ask("stack", "-thread", String(args.threadId));
    const out = [];
    for (const f of st.frames) {
      // The calls inlined at its PC, innermost first, as frames of their
      // own (their locals are the real frame's).
      for (const inline of f.inlines || []) {
        out.push({
          id: this.frames.get({ thread: args.threadId, index: f.index, inline: out.length }),
          name: `${f.module || "?"}!${inline.function} (inlined)`,
          source: inline.file ? { path: inline.file } : undefined,
          line: inline.line || 0,
          column: inline.line ? 1 : 0,
          instructionPointerReference: f.pc,
          presentationHint: "normal",
        });
      }
      out.push({
        id: this.frames.get({ thread: args.threadId, index: f.index }),
        name: frameName(f),
        source: f.file ? { path: f.file } : undefined,
        line: f.line || 0,
        column: f.line ? 1 : 0,
        instructionPointerReference: f.pc,
        presentationHint: f.confidence === "likely" ? "subtle" : "normal",
      });
    }
    const start = args.startFrame || 0;
    const levels = args.levels || out.length;
    return { stackFrames: out.slice(start, start + levels), totalFrames: out.length };
  }

  on_scopes(args) {
    const f = this.frames.lookup(args.frameId);
    if (!f) {
      throw new Error("no such frame");
    }
    return {
      scopes: [
        { name: "Locals", presentationHint: "locals", variablesReference: this.vars.get({ kind: "locals", thread: f.thread, index: f.index }), expensive: false },
        { name: "Registers", presentationHint: "registers", variablesReference: this.vars.get({ kind: "registers", thread: f.thread, index: f.index }), expensive: false },
      ],
    };
  }

  /** A value as a variable: its children (a structure's members, an
   * array's elements) behind a reference. */
  variable(name, v, path) {
    const kids = v && v.children && v.children.length;
    return {
      name,
      value: valueText(v),
      type: v ? v.type : undefined,
      variablesReference: kids ? this.vars.get({ ...path }) : 0,
      memoryReference: v && v.pointer ? v.pointer : undefined,
    };
  }

  async on_variables(args) {
    const ref = this.vars.lookup(args.variablesReference);
    if (!ref) {
      return { variables: [] };
    }
    if (ref.kind === "registers") {
      const regs = await this.ask("registers", "-thread", String(ref.thread), "-frame", String(ref.index));
      return {
        variables: regs.map((r) => ({
          name: r.register,
          value: r.clobbered ? "<clobbered>" : r.value,
          type: r.clobbered ? "clobbered by a call" : `from ${r.from}`,
          variablesReference: 0,
        })),
      };
    }
    const locals = await this.ask("locals", "-thread", String(ref.thread), "-frame", String(ref.index));
    if (!Array.isArray(locals)) {
      return { variables: [{ name: "(locals)", value: `<${locals.unavailable}>`, variablesReference: 0 }] };
    }
    if (ref.kind === "locals") {
      return {
        variables: locals.map((l, i) => {
          if (!l.value) {
            return { name: l.name, value: `<${l.unavailable}>`, type: l.type, variablesReference: 0 };
          }
          const v = this.variable(l.name, l.value, { kind: "value", thread: ref.thread, index: ref.index, local: i, path: [] });
          v.type = l.provenance === "image" ? `${l.type} (from the image file)` : l.type;
          return v;
        }),
      };
    }
    // A value's children: down its path from its local.
    let v = locals[ref.local] && locals[ref.local].value;
    for (const k of ref.path) {
      v = v && v.children ? v.children[k] : undefined;
    }
    const kids = (v && v.children) || [];
    return {
      variables: kids.map((c, k) =>
        this.variable(c.name || `[${k}]`, c, { ...ref, path: [...ref.path, k] }),
      ),
    };
  }

  /** The debug console: a dump-analyzer command on the dump (`modules`,
   * `memory 0x1000 64`, `reconstruct -thread 4`). */
  async on_evaluate(args) {
    const words = String(args.expression || "").trim().split(/\s+/).filter(Boolean);
    if (!words.length) {
      return { result: "", variablesReference: 0 };
    }
    const [command, ...rest] = words;
    const answer = await this.request([command, this.dump, ...rest, ...this.options]);
    return { result: JSON.stringify(answer, null, 2), variablesReference: 0 };
  }

  async on_disassemble(args) {
    const base = BigInt(args.memoryReference) + BigInt(args.offset || 0);
    const offset = args.instructionOffset || 0;
    const count = args.instructionCount || 1;
    // (Instructions before the address: from a little before it.)
    const back = offset < 0 ? BigInt(-offset * 8) : 0n;
    const from = base > back ? base - back : 0n;
    const answer = await this.ask("disasm", "-address", "0x" + from.toString(16), "-count", String(count + Math.max(0, -offset)));
    let list = answer.map((i) => ({
      address: i.address,
      instructionBytes: i.bytes,
      instruction: i.text,
    }));
    if (offset < 0) {
      // Those before the address, then from it.
      const at = list.findIndex((i) => BigInt(i.address) >= base);
      const before = at < 0 ? list : list.slice(0, at);
      list = before.slice(Math.max(0, before.length + offset)).concat(at < 0 ? [] : list.slice(at));
    } else if (offset > 0) {
      list = list.slice(offset);
    }
    // DAP wants exactly so many: pad with what isn't there.
    while (list.length < count) {
      list.push({ address: "0x0", instruction: "??", presentationHint: "invalid" });
    }
    return { instructions: list.slice(0, count) };
  }

  async on_readMemory(args) {
    const start = BigInt(args.memoryReference) + BigInt(args.offset || 0);
    const count = Math.min(args.count || 0, 1 << 20);
    const runs = await this.ask("memory", "0x" + start.toString(16), String(count));
    // DAP's answer: bytes from the start, contiguous, up to the first
    // that aren't there.
    let bytes = "";
    let at = start;
    for (const r of runs) {
      if (BigInt(r.address) !== at) {
        break;
      }
      bytes += r.bytes;
      at += BigInt(r.length);
    }
    const data = Buffer.from(bytes, "hex").toString("base64");
    return {
      address: "0x" + start.toString(16),
      data,
      unreadableBytes: count - bytes.length / 2,
    };
  }

  on_disconnect() {
    return {};
  }

  on_terminate() {
    this.event("terminated", {});
    return {};
  }

  // A dump doesn't run: these answer, and nothing moves.
  on_setBreakpoints(args) {
    return { breakpoints: (args.breakpoints || []).map(() => ({ verified: false, message: "a dump doesn't run" })) };
  }

  on_setExceptionBreakpoints() {
    return {};
  }

  on_continue() {
    throw new Error("a dump doesn't run");
  }

  on_source() {
    throw new Error("no source for this frame");
  }
}

module.exports = { DumpSession, Handles, frameName, valueText };
