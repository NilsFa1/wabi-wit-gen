#!/usr/bin/env node
// wabi-wit-gen for Node.js (>= 20) and Bun: the generator is compiled once to
// WebAssembly (wasm32-wasip1) and runs here through WASI. Same binary, same
// output on every platform; no native download, no postinstall.
import { readdirSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { join, parse, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

// WASI is still flagged "experimental" in Node; the warning is noise for a CLI.
const emitWarning = process.emitWarning;
process.emitWarning = (warning, ...rest) => {
  const text = typeof warning === "string" ? warning : warning?.message;
  if (typeof text === "string" && text.includes("WASI")) return;
  return emitWarning.call(process, warning, ...rest);
};
const { WASI } = await import("node:wasi");

// Options whose value is a path, plus the positional WIT path.
const PATH_OPTIONS = new Set(["--out", "-o", "--lock", "--api-baseline"]);
// --core rust,go=<dir>: the directory behind `=` is a path, too.
const CORE_OPTIONS = new Set(["--core", "--cores", "-c"]);
const VALUE_OPTIONS = new Set([...PATH_OPTIONS, "--target", "--targets", "-t", "--core", "--cores", "-c"]);

// WASI sees one virtual file system. POSIX: "/" is the host root. Windows: every
// drive becomes "/<letter>" ("C:\\work\\x.wit" -> "/c/work/x.wit").
const windows = process.platform === "win32";
const preopens = {};
function toGuest(hostPath) {
  const abs = resolve(hostPath);
  if (!windows) {
    preopens["/"] = "/";
    return abs;
  }
  const root = parse(abs).root; // "C:\\"
  const drive = `/${root[0].toLowerCase()}`;
  preopens[drive] = root;
  return drive + "/" + abs.slice(root.length).split(sep).join("/");
}

const args = process.argv.slice(2);
const guestArgs = [];
for (let i = 0; i < args.length; i++) {
  const a = args[i];
  if (VALUE_OPTIONS.has(a) && i + 1 < args.length) {
    const v = args[++i];
    if (PATH_OPTIONS.has(a)) guestArgs.push(a, toGuest(v));
    else if (CORE_OPTIONS.has(a)) {
      const cores = v.split(",").map((c) => (c.includes("=") ? `${c.slice(0, c.indexOf("="))}=${toGuest(c.slice(c.indexOf("=") + 1))}` : c));
      guestArgs.push(a, cores.join(","));
    } else guestArgs.push(a, v);
  } else if (!a.startsWith("-")) {
    guestArgs.push(toGuest(a));
  } else {
    guestArgs.push(a);
  }
}
if (Object.keys(preopens).length === 0) toGuest(process.cwd());

const wasi = new WASI({
  version: "preview1",
  args: ["wabi-wit-gen", ...guestArgs],
  env: {},
  preopens,
  returnOnExit: true,
});
const wasmPath = fileURLToPath(new URL("../wabi-wit-gen.wasm", import.meta.url));
const module = await WebAssembly.compile(await readFile(wasmPath));
let memory;
const instance = await WebAssembly.instantiate(module, { wasi_snapshot_preview1: withReaddir(wasi.wasiImport) });
memory = instance.exports.memory;
process.exitCode = wasi.start(instance) ?? 0;

// Node's WASI (uvwasi) has no fd_readdir on Windows (ENOSYS), so the generator
// could not list a WIT directory there. Fallback: remember the host path of every
// fd opened through path_open and list it with node:fs. Only used where the
// runtime itself answers ENOSYS (or WABI_WASI_JS_READDIR=1, for tests).
function withReaddir(imports) {
  const ENOSYS = 52;
  const force = process.env.WABI_WASI_JS_READDIR === "1";
  const paths = new Map(Object.values(preopens).map((host, i) => [3 + i, host])); // preopens: fd 3, 4, …
  const view = () => new DataView(memory.buffer);
  const { path_open, fd_close, fd_renumber, fd_readdir } = imports;
  return {
    ...imports,
    path_open(dirfd, dirflags, pathPtr, pathLen, ...rest) {
      const r = path_open(dirfd, dirflags, pathPtr, pathLen, ...rest);
      const base = paths.get(dirfd);
      if (r === 0 && base !== undefined) {
        const rel = Buffer.from(memory.buffer, pathPtr, pathLen).toString("utf8");
        paths.set(view().getUint32(rest[rest.length - 1], true), join(base, rel));
      }
      return r;
    },
    fd_close(fd) {
      paths.delete(fd);
      return fd_close(fd);
    },
    fd_renumber(from, to) {
      const r = fd_renumber(from, to);
      if (r === 0) {
        if (paths.has(from)) paths.set(to, paths.get(from));
        else paths.delete(to);
        paths.delete(from);
      }
      return r;
    },
    fd_readdir(fd, buf, bufLen, cookie, bufUsedPtr) {
      const r = force ? ENOSYS : fd_readdir(fd, buf, bufLen, cookie, bufUsedPtr);
      const host = paths.get(fd);
      if (r !== ENOSYS || host === undefined) return r;
      let entries;
      try {
        entries = readdirSync(host, { withFileTypes: true });
      } catch {
        return 54; // ENOTDIR
      }
      // dirent: d_next u64, d_ino u64, d_namlen u32, d_type u8 (+3), then the name;
      // the last entry may be cut off at bufLen (the caller then retries larger).
      const out = new Uint8Array(memory.buffer, buf, bufLen);
      let used = 0;
      for (let i = Number(cookie); i < entries.length && used < bufLen; i++) {
        const e = entries[i];
        const name = Buffer.from(e.name, "utf8");
        const rec = new Uint8Array(24 + name.length);
        const dv = new DataView(rec.buffer);
        dv.setBigUint64(0, BigInt(i + 1), true);
        dv.setUint32(16, name.length, true);
        dv.setUint8(20, e.isDirectory() ? 3 : e.isFile() ? 4 : e.isSymbolicLink() ? 7 : 0);
        rec.set(name, 24);
        const n = Math.min(rec.length, bufLen - used);
        out.set(rec.subarray(0, n), used);
        used += n;
      }
      view().setUint32(bufUsedPtr, used, true);
      return 0;
    },
  };
}
