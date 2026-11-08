// Hand-written, generic: the build-hook side of Dart native assets. Every SDK
// package calls [buildNativeCore] from its `hook/build.dart`; Flutter and
// `dart run`/`dart test` then bundle the core library for the target platform
// without any manual step (no jniLibs, xcframework, DLL or dylib copying).
//
// Not part of the SDK API: only the build hook imports this library.
import 'dart:io';

import 'package:code_assets/code_assets.dart';
import 'package:crypto/crypto.dart';
import 'package:hooks/hooks.dart';

/// Asset name of the core library. The asset id is then
/// `package:<dart package>/src/generated/native_abi.dart`, the URI of the
/// library with the `@Native` bindings (`NativeAbi.asset()`), which is the
/// default asset id of `@Native`.
const nativeAbiAssetName = 'src/generated/native_abi.dart';

/// Checksum file of the release artifacts, relative to the package root.
/// One line per artifact, as written by `sha256sum`: `<hex>  <target>/<file>`.
const checksumFile = 'native/SHA256SUMS';

/// Runs the build hook of an SDK package whose core library is called
/// [library] (e.g. `@LIBRARY@` -> `lib@LIBRARY@.so`, `@LIBRARY@.dll`).
///
/// The library comes from the first of these that is configured, in the
/// app's `pubspec.yaml`:
///
/// ```yaml
/// hooks:
///   user_defines:
///     @LIBRARY@:                  # the SDK package
///       library: ../core/target/release/lib@LIBRARY@.so   # 1. one file (host builds)
///       prebuilt_dir: ../build/native                        # 2. per-target layout, below
///       release_url: https://example.org/releases/v1.0.0    # 3. download, checked against native/SHA256SUMS
/// ```
///
/// Without configuration, [defaultPrebuiltDir] (relative to the package root)
/// and then [defaultReleaseUrl] are tried. If none yields a library, no asset
/// is emitted and the SDK falls back to `WABI_NATIVE_LIB` and the naming
/// convention at runtime (see `NativeBackend.shared`); set `required: true`
/// to make that a build error instead.
///
/// Layout of a prebuilt directory and of a release (`<dir>/<target>/<file>`),
/// as produced by the chosen core's build process (see the SDK README.md);
/// the first existing candidate wins:
///
/// | target | candidates |
/// |---|---|
/// | Android | `android/jniLibs/<abi>/`, `jniLibs/<abi>/` (`arm64-v8a`, `armeabi-v7a`, `x86_64`, `x86`) |
/// | iOS device | `ios/` (dynamic library, arm64) |
/// | iOS simulator | `ios-simulator/` (dynamic library, arm64 + x86_64) |
/// | macOS | `macos/`, `osx/` (universal), `macos-<arch>/` |
/// | Linux | `linux-<arch>/` (`x64`, `arm64`, ...) |
/// | Windows | `windows-<arch>/`, `win-<arch>/` |
Future<void> buildNativeCore(List<String> args,
        {required String library, String? defaultPrebuiltDir, String? defaultReleaseUrl}) =>
    build(args, (input, output) async {
      if (!input.config.buildCodeAssets) return;
      final code = input.config.code;
      final file = code.targetOS.dylibFileName(library);
      final targets = targetDirectories(code);
      final defines = input.userDefines;
      final tried = <String>[];

      Uri? found = defines.path('library');
      if (found != null) {
        if (!File.fromUri(found).existsSync()) {
          throw HookException("user-define 'library': ${found.toFilePath()} does not exist");
        }
      }

      if (found == null) {
        final configured = defines.path('prebuilt_dir');
        final dir = configured ?? (defaultPrebuiltDir == null ? null : input.packageRoot.resolve(_slash(defaultPrebuiltDir)));
        if (dir != null) {
          found = _findPrebuilt(dir, targets, file, tried);
          if (found == null && configured != null) {
            throw HookException("user-define 'prebuilt_dir': no $file for ${_describe(code)}, tried:\n  ${tried.join('\n  ')}");
          }
          if (found == null) {
            // rebuild once the library appears
            output.dependencies.addAll(targets.map((t) => _asDirectory(dir).resolve('$t/$file')));
          }
        }
      }

      if (found == null) {
        final configured = defines['release_url'] as String?;
        final base = configured ?? defaultReleaseUrl;
        if (base != null) {
          found = await _download(input, output, base, targets, file, tried);
          if (found == null && configured != null) {
            throw HookException("user-define 'release_url': no $file for ${_describe(code)} in $checksumFile, "
                'tried:\n  ${tried.join('\n  ')}');
          }
        }
      }

      if (found == null) {
        if (defines['required'] == true) {
          throw HookException('no native core $file for ${_describe(code)}; configure library, prebuilt_dir or '
              'release_url (see ${input.packageName} docs). Tried:\n  ${tried.join('\n  ')}');
        }
        return; // runtime fallback: WABI_NATIVE_LIB / naming convention
      }
      if (found.isScheme('file')) output.dependencies.add(found);
      found = await _thin(input, code, found, file);
      output.assets.code.add(CodeAsset(
        package: input.packageName,
        name: nativeAbiAssetName,
        linkMode: DynamicLoadingBundled(),
        file: found,
      ));
    });

/// Directory names (relative, without trailing slash) that may hold the
/// library for the build target, most specific first.
List<String> targetDirectories(CodeConfig code) {
  final arch = code.targetArchitecture.name; // arm64, x64, arm, ia32, riscv64
  switch (code.targetOS) {
    case OS.android:
      final abi = const {'arm64': 'arm64-v8a', 'arm': 'armeabi-v7a', 'x64': 'x86_64', 'ia32': 'x86'}[arch] ?? arch;
      return ['android/jniLibs/$abi', 'jniLibs/$abi', 'android-$arch'];
    case OS.iOS:
      return code.iOS.targetSdk == IOSSdk.iPhoneSimulator
          ? ['ios-simulator-$arch', 'ios-simulator']
          : ['ios-$arch', 'ios'];
    case OS.macOS:
      return ['macos-$arch', 'macos', 'osx'];
    case OS.windows:
      return ['windows-$arch', 'win-$arch'];
    default:
      return ['${code.targetOS.name}-$arch'];
  }
}

String _describe(CodeConfig code) {
  final sdk = code.targetOS == OS.iOS ? ' (${code.iOS.targetSdk})' : '';
  return '${code.targetOS.name}-${code.targetArchitecture.name}$sdk';
}

String _slash(String dir) => dir.endsWith('/') ? dir : '$dir/';

Uri? _findPrebuilt(Uri dir, List<String> targets, String file, List<String> tried) {
  for (final t in targets) {
    final candidate = _asDirectory(dir).resolve('$t/$file');
    tried.add(candidate.toFilePath());
    if (File.fromUri(candidate).existsSync()) return candidate;
  }
  return null;
}

Uri _asDirectory(Uri dir) => dir.path.endsWith('/') ? dir : dir.replace(path: '${dir.path}/');

/// Flutter builds Apple targets per architecture and merges the assets with
/// `lipo -create`, which fails for two universal inputs. A universal (fat)
/// Mach-O library such as `osx/lib@LIBRARY@.dylib` is therefore reduced to
/// the target architecture.
Future<Uri> _thin(BuildInput input, CodeConfig code, Uri library, String file) async {
  if (code.targetOS != OS.macOS && code.targetOS != OS.iOS) return library;
  if (!isUniversalMachO(File.fromUri(library))) return library;
  final arch = const {'x64': 'x86_64', 'arm64': 'arm64', 'arm': 'armv7', 'ia32': 'i386'}[code.targetArchitecture.name] ??
      code.targetArchitecture.name;
  final out = File.fromUri(input.outputDirectory.resolve(file));
  out.parent.createSync(recursive: true);
  final r = await Process.run('lipo', [library.toFilePath(), '-thin', arch, '-output', out.path]);
  if (r.exitCode != 0) {
    throw HookException('lipo -thin $arch ${library.toFilePath()}: ${r.stderr}'.trim());
  }
  return out.uri;
}

/// True for a universal ("fat") Mach-O file (magic 0xcafebabe / 0xcafebabf, big endian).
bool isUniversalMachO(File f) {
  final raf = f.openSync();
  try {
    final b = raf.readSync(8);
    if (b.length < 8 || b[0] != 0xca || b[1] != 0xfe || b[2] != 0xba || (b[3] != 0xbe && b[3] != 0xbf)) return false;
    // Java class files share the magic; there the next word is a version >= 45, here a small arch count.
    final count = (b[4] << 24) | (b[5] << 16) | (b[6] << 8) | b[7];
    return count > 0 && count < 45;
  } finally {
    raf.closeSync();
  }
}

/// Downloads `<base>/<target>/<file>` for the first target listed in the
/// package's checksum file, verifies the SHA-256 and caches it in the shared
/// output directory (keyed by checksum, so a new release downloads anew).
Future<Uri?> _download(BuildInput input, BuildOutputBuilder output, String base, List<String> targets,
    String file, List<String> tried) async {
  final sumsFile = File.fromUri(input.packageRoot.resolve(checksumFile));
  output.dependencies.add(sumsFile.uri);
  if (!sumsFile.existsSync()) {
    tried.add('${sumsFile.path} (missing, needed for release_url)');
    return null;
  }
  final sums = parseChecksums(sumsFile.readAsStringSync());
  for (final t in targets) {
    final key = '$t/$file';
    final expected = sums[key];
    tried.add('$base/$key${expected == null ? ' (not in $checksumFile)' : ''}');
    if (expected == null) continue;
    final target = File.fromUri(input.outputDirectoryShared.resolve('$expected/$file'));
    if (target.existsSync() && sha256.convert(target.readAsBytesSync()).toString() == expected) return target.uri;
    final bytes = await _get(Uri.parse('${base.endsWith('/') ? base : '$base/'}$key'));
    final actual = sha256.convert(bytes).toString();
    if (actual != expected) {
      throw HookException('checksum mismatch for $base/$key: expected $expected, got $actual');
    }
    target.parent.createSync(recursive: true);
    final partial = File('${target.path}.part')..writeAsBytesSync(bytes, flush: true);
    partial.renameSync(target.path); // atomic: a crashed download never looks complete
    return target.uri;
  }
  return null;
}

/// `sha256sum` output: `<hex>  <path>` or `<hex> *<path>` per line.
Map<String, String> parseChecksums(String text) {
  final sums = <String, String>{};
  for (final line in text.split('\n')) {
    final m = RegExp(r'^([0-9a-fA-F]{64})\s+\*?(.+?)\s*$').firstMatch(line);
    if (m != null) sums[m.group(2)!.replaceFirst(RegExp(r'^\./'), '')] = m.group(1)!.toLowerCase();
  }
  return sums;
}

Future<List<int>> _get(Uri url) async {
  if (url.isScheme('file')) return File.fromUri(url).readAsBytesSync();
  final client = HttpClient()..findProxy = HttpClient.findProxyFromEnvironment;
  try {
    final response = await (await client.getUrl(url)).close();
    if (response.statusCode != HttpStatus.ok) {
      throw HookException('download $url: HTTP ${response.statusCode}');
    }
    final bytes = <int>[];
    await response.forEach(bytes.addAll);
    return bytes;
  } finally {
    client.close();
  }
}

/// A build-hook failure with a message meant for the app developer.
final class HookException implements Exception {
  HookException(this.message);
  final String message;
  @override
  String toString() => 'native core: $message';
}
