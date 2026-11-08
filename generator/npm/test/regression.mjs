// Package-output and create-once regression matrix, shared by native and WASM.
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, readdirSync, statSync, lstatSync, unlinkSync, symlinkSync, rmSync, existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve, relative, dirname } from 'node:path';
import { spawnSync } from 'node:child_process';

const [native, launcher] = process.argv.slice(2).map(p => resolve(p));
assert(native && launcher, 'usage: node regression.mjs <native-generator> <npm-launcher>');
const root = mkdtempSync(join(tmpdir(), 'wabi regression-'));
function run(command, cwd, args, ok = true) {
  const r = spawnSync(command[0], [...command.slice(1), ...args], { cwd, encoding: 'utf8' });
  assert(!r.error, String(r.error));
  if (ok) assert.equal(r.status, 0, r.stderr + r.stdout);
  else assert.notEqual(r.status, 0, 'expected a rejected invocation');
  return r;
}
function tree(dir, metadata = false) {
  const result = {};
  function walk(path) {
    for (const entry of readdirSync(path, { withFileTypes: true })) {
      const file = join(path, entry.name);
      if (entry.isDirectory()) walk(file);
      else result[relative(dir, file).replaceAll('\\', '/')] = metadata
        ? [readFileSync(file, 'utf8'), statSync(file).mtimeMs]
        : readFileSync(file, 'utf8');
    }
  }
  walk(dir); return result;
}

try {
  for (const [namespace, pkg] of [['alpha', 'archive-reader'], ['beta', 'volume-tools']]) {
    for (const core of ['none', 'rust', 'go', 'csharp']) {
      const result = [];
      for (const [runner, command] of [['native', [native]], ['wasm', [process.execPath, launcher]]]) {
        const cwd = join(root, `${namespace}-${pkg}-${core}-${runner}`); mkdirSync(join(cwd, 'wit'), { recursive: true });
        const source = `package ${namespace}:${pkg}@1.0.0; interface api { resource reader { constructor(); ping: func() -> u32; } } world api-world { export api; }\n`;
        writeFileSync(join(cwd, 'wit/api.wit'), source);
        const args = ['wit', '--out', 'sdk', '--target', 'csharp,java,swift,dart,ts', '--core', core, '--scaffold', 'examples'];
        const initial = tree(cwd, true);
        run(command, cwd, [...args, "--check"], false);
        assert.deepEqual(tree(cwd, true), initial);
        run(command, cwd, args);
        const sdk = tree(join(cwd, 'sdk'));
        const prefix = `${namespace}_${pkg.replaceAll('-', '_')}`;
        const pn = pkg.split('-').map(w => w[0].toUpperCase() + w.slice(1)).join('');
        const nn = namespace[0].toUpperCase() + namespace.slice(1);
        const ns = `${nn}.${pn}`, module = `${nn}${pn}`, jp = `com/${namespace}/${pkg.replaceAll('-', '_')}/runtime`;
        assert(sdk['csharp/NativeAbi.g.cs'].includes(`namespace ${ns}.Runtime;`));
        assert(sdk['csharp/NativeAbi.g.cs'].includes(`SymbolPrefix = "${prefix}_"`));
        assert(sdk[`java/${jp}/AbiConstants.java`].includes(`LIBRARY = "${prefix}"`));
        assert(sdk[`java/${jp}/JniAbi.java`].includes(`System.loadLibrary("${prefix}_jni")`));
        const mangled = `com_${namespace}_${pkg.replaceAll('-', '_1')}_runtime_JniAbi`;
        assert(sdk[`java/jni/${prefix}_jni.c`].includes(`Java_${mangled}_`));
        assert(sdk[`swift/${module}/Sources/${module}/AbiConstants.swift`].includes(`library = "${prefix}"`));
        assert(sdk['dart/lib/src/generated/abi_constants.dart'].includes(`library = '${prefix}'`));
        assert(sdk['ts/abi_constants.ts'].includes(`symbolPrefix: "${prefix}_"`));
        for (const [path, text] of Object.entries(sdk)) {
          assert(!/wabi_client|Wabi\.Client|com\.wabi\.client|@LIBRARY@|@JNI_CLASS@/.test(text), `reference package in ${path}`);
          if (path.endsWith('.md')) {
            for (const [, link] of text.matchAll(/\]\(([^)]+)\)/g)) {
              if (/^(https?:|#)/.test(link)) continue;
              assert(existsSync(resolve(cwd, 'sdk', dirname(path), link.split('#')[0])), `${path}: broken ${link}`);
            }
          }
        }
        for (const lang of ['rust', 'go']) {
          assert.equal(existsSync(join(cwd, `sdk/core-${lang}/build-native.sh`)), core === lang);
          if (core !== lang) assert(!sdk['README.md'].includes(`core-${lang}/build-native.sh`));
        }
        if (core === 'csharp') {
          assert(sdk['README.md'].includes('dotnet publish core-csharp/Core.csproj'));
          assert(sdk['README.md'].includes(`native/linux-x64/lib${prefix}.so`));
        }
        const before = tree(cwd, true);
        run(command, cwd, []); // project config, no repeated options
        assert.deepEqual(tree(cwd, true), before);
        run(command, cwd, ['--check', '--scaffold', 'examples']);
        assert.deepEqual(tree(cwd, true), before);
        writeFileSync(join(cwd, 'sdk/README.md'), sdk['README.md'] + '\nstale\n');
        const stale = tree(cwd, true);
        run(command, cwd, ['--check'], false);
        assert.deepEqual(tree(cwd, true), stale);
        run(command, cwd, []);
        // Persist user modifications across regeneration, config loading and scaffold reruns.
        for (const path of ['sdk/examples/ts/main.ts', 'wabi-gen.conf', ...(core === 'csharp' ? ['sdk/core-csharp/Core.cs', 'sdk/core-csharp/Core.csproj'] : [])]) {
          writeFileSync(join(cwd, path), readFileSync(join(cwd, path), 'utf8') + '\n// user-owned\n'.replace('//', path.endsWith('.conf') ? '#' : '//'));
        }
        const coreFiles = Object.keys(sdk).filter(p => /^(core-rust\/(Cargo.toml|src\/lib.rs)|core-go\/(go.mod|starter.go)|core-csharp\/(Core.cs|Core.csproj|GlobalUsings.cs))$/.test(p));
        const protectedCore = Object.fromEntries(coreFiles.map(p => [p, readFileSync(join(cwd, 'sdk', p), 'utf8')]));
        const preserved = readFileSync(join(cwd, 'sdk/examples/ts/main.ts'), 'utf8');
        const conf = readFileSync(join(cwd, 'wabi-gen.conf'), 'utf8');
        const missing = join(cwd, 'sdk/examples/java/Main.java'); unlinkSync(missing);
        run(command, cwd, ['--check']); assert(!existsSync(missing));
        run(command, cwd, []); assert(!existsSync(missing));
        run(command, cwd, ['--scaffold', 'examples']); assert(existsSync(missing));
        assert.equal(readFileSync(join(cwd, 'sdk/examples/ts/main.ts'), 'utf8'), preserved);
        assert.equal(readFileSync(join(cwd, 'wabi-gen.conf'), 'utf8'), conf);
        for (const [p, text] of Object.entries(protectedCore)) assert.equal(readFileSync(join(cwd, 'sdk', p), 'utf8'), text);
        if (process.platform !== 'win32') {
          const link = join(cwd, 'sdk/examples/ts/main.ts'); unlinkSync(link);
          symlinkSync('not-created.ts', link);
          run(command, cwd, ['--scaffold', 'examples']); assert(lstatSync(link).isSymbolicLink());
          assert(!existsSync(join(cwd, 'sdk/examples/ts/not-created.ts')));
          unlinkSync(link); writeFileSync(link, preserved);
        }
        writeFileSync(join(cwd, 'wit/api.wit'), source.replace('ping: func()', 'pong: func(); ping: func()'));
        run(command, cwd, []);
        assert.equal(readFileSync(join(cwd, 'sdk/examples/ts/main.ts'), 'utf8'), preserved);
        assert(tree(cwd)['wit/abi-ids.lock'].includes('reader.pong'));
        for (const [p, text] of Object.entries(protectedCore)) assert.equal(readFileSync(join(cwd, 'sdk', p), 'utf8'), text);
        run(command, cwd, ['--out', 'override', '--target', 'ts', '--core', 'none']);
        assert(existsSync(join(cwd, 'override/ts/package.json')));
        assert(!existsSync(join(cwd, 'override/csharp')));
        assert(!existsSync(join(cwd, 'override/core-csharp')));
        assert.equal(readFileSync(join(cwd, 'wabi-gen.conf'), 'utf8'), conf);
        // Config paths remain relative to the config even when invoked elsewhere.
        run(command, root, ['--config', join(cwd, 'wabi-gen.conf'), '--check']);
        result.push(tree(cwd));
      }
      assert.deepEqual(result[0], result[1], `${namespace}:${pkg}/${core}: native/WASM mismatch`);
      console.log(`PASS names, docs, core=${core}, config, preservation, determinism, check and native/WASM: ${namespace}:${pkg}`);
    }
  }
} finally {
  rmSync(root, { recursive: true, force: true });
}
