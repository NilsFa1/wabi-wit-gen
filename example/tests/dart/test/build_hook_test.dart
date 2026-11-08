// The native-assets build hook of the SDK package (hook/build.dart -> lib/src/generated/runtime/native_assets.dart)
// for every target platform, run in isolation on the host: which library it
// picks from a prebuilt directory, and the checksum-verified download.
import 'dart:io';

import 'package:demo_client/src/generated/build_hook.dart';
import 'package:demo_client/src/generated/runtime/native_assets.dart';
import 'package:code_assets/code_assets.dart';
import 'package:crypto/crypto.dart';
import 'package:hooks/hooks.dart';
import 'package:test/test.dart';

late Directory temp;

/// Creates `<temp>/<relative>` with its own path as content (not a real
/// library: the output validation skips files it does not recognize).
File fake(String relative) {
  final f = File('${temp.path}/$relative')..createSync(recursive: true);
  f.writeAsStringSync(relative);
  return f;
}

PackageUserDefines defines(Map<String, Object?> values) => PackageUserDefines(
    workspacePubspec: PackageUserDefinesSource(defines: values, basePath: Directory.current.uri));

/// Runs the hook for one target; returns the bundled file's content, or null if no asset.
Future<String?> run(OS os, Architecture arch, Map<String, Object?> userDefines,
    {IOSSdk iosSdk = IOSSdk.iPhoneOS}) async {
  String? bundled;
  await testCodeBuildHook(
    mainMethod: buildHook,
    targetOS: os,
    targetArchitecture: arch,
    targetIOSSdk: iosSdk,
    userDefines: defines(userDefines),
    check: (input, output) {
      final assets = output.assets.code.toList();
      expect(assets.length, lessThanOrEqualTo(1));
      if (assets.isEmpty) return;
      // the test harness runs the hook under the name of this test package
      expect(assets.single.id, endsWith('/$nativeAbiAssetName'));
      expect(assets.single.linkMode, DynamicLoadingBundled());
      bundled = File.fromUri(assets.single.file!).readAsStringSync();
    },
  );
  return bundled;
}

void main() {
  setUp(() => temp = Directory.systemTemp.createTempSync('wabi_hook'));
  tearDown(() => temp.deleteSync(recursive: true));

  test('picks the library for each target from a prebuilt directory', () async {
    // build-native.sh layout (Rust core) plus the core-go/build.sh names
    for (final f in [
      'linux-x64/libdemo_client.so',
      'linux-arm64/libdemo_client.so',
      'win-x64/demo_client.dll',
      'windows-arm64/demo_client.dll',
      'osx/libdemo_client.dylib',
      'android/jniLibs/arm64-v8a/libdemo_client.so',
      'android/jniLibs/armeabi-v7a/libdemo_client.so',
      'android/jniLibs/x86_64/libdemo_client.so',
      'ios/libdemo_client.dylib',
      'ios-simulator/libdemo_client.dylib',
    ]) {
      fake(f);
    }
    final dir = {'prebuilt_dir': temp.path};
    expect(await run(OS.linux, Architecture.x64, dir), 'linux-x64/libdemo_client.so');
    expect(await run(OS.linux, Architecture.arm64, dir), 'linux-arm64/libdemo_client.so');
    expect(await run(OS.windows, Architecture.x64, dir), 'win-x64/demo_client.dll');
    expect(await run(OS.windows, Architecture.arm64, dir), 'windows-arm64/demo_client.dll');
    expect(await run(OS.macOS, Architecture.arm64, dir), 'osx/libdemo_client.dylib');
    expect(await run(OS.macOS, Architecture.x64, dir), 'osx/libdemo_client.dylib');
    expect(await run(OS.android, Architecture.arm64, dir), 'android/jniLibs/arm64-v8a/libdemo_client.so');
    expect(await run(OS.android, Architecture.arm, dir), 'android/jniLibs/armeabi-v7a/libdemo_client.so');
    expect(await run(OS.android, Architecture.x64, dir), 'android/jniLibs/x86_64/libdemo_client.so');
    expect(await run(OS.iOS, Architecture.arm64, dir), 'ios/libdemo_client.dylib');
    expect(await run(OS.iOS, Architecture.arm64, dir, iosSdk: IOSSdk.iPhoneSimulator),
        'ios-simulator/libdemo_client.dylib');
    expect(await run(OS.iOS, Architecture.x64, dir, iosSdk: IOSSdk.iPhoneSimulator),
        'ios-simulator/libdemo_client.dylib');
  });

  test('a configured prebuilt_dir without the target library is a build error', () async {
    fake('linux-x64/libdemo_client.so');
    await expectLater(run(OS.android, Architecture.ia32, {'prebuilt_dir': temp.path}),
        throwsA(predicate((e) => '$e'.contains('android/jniLibs/x86/libdemo_client.so'))));
  });

  test('explicit library wins; nothing configured and nothing found emits no asset', () async {
    final lib = fake('custom/libdemo_client.so');
    fake('linux-x64/libdemo_client.so');
    expect(await run(OS.linux, Architecture.x64, {'library': lib.path, 'prebuilt_dir': temp.path}),
        'custom/libdemo_client.so');
    // The package default (../native) has no Fuchsia library: the SDK falls back at runtime.
    expect(await run(OS.fuchsia, Architecture.x64, {}), isNull);
    await expectLater(run(OS.fuchsia, Architecture.x64, {'required': true}), throwsA(isA<Object>()));
  });

  test('downloads from release_url and verifies the checksum', () async {
    final release = Directory('${temp.path}/release');
    final lib = File('${release.path}/android/jniLibs/arm64-v8a/libdemo_client.so')..createSync(recursive: true);
    lib.writeAsStringSync('release build');
    final sum = sha256.convert(lib.readAsBytesSync()).toString();
    final sums = File(checksumFile);
    final hadSums = sums.existsSync();
    final saved = hadSums ? sums.readAsStringSync() : null;
    final hadDir = sums.parent.existsSync();
    addTearDown(() {
      if (hadSums) {
        sums.writeAsStringSync(saved!);
      } else if (hadDir) {
        sums.deleteSync();
      } else {
        sums.parent.deleteSync(recursive: true);
      }
    });

    sums
      ..createSync(recursive: true)
      ..writeAsStringSync('$sum  android/jniLibs/arm64-v8a/libdemo_client.so\n');
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    addTearDown(server.close);
    server.listen((r) {
      final f = File('${release.path}${r.uri.path.replaceFirst('/v1', '')}');
      if (f.existsSync()) {
        r.response.add(f.readAsBytesSync());
      } else {
        r.response.statusCode = HttpStatus.notFound;
      }
      r.response.close();
    });
    final url = {'release_url': 'http://127.0.0.1:${server.port}/v1'};
    expect(await run(OS.android, Architecture.arm64, url), 'release build');

    sums.writeAsStringSync('${'0' * 64}  android/jniLibs/arm64-v8a/libdemo_client.so\n');
    await expectLater(run(OS.android, Architecture.arm64, url),
        throwsA(predicate((e) => '$e'.contains('checksum mismatch'))));
    // a target missing from the checksum file is never downloaded
    await expectLater(run(OS.android, Architecture.x64, url),
        throwsA(predicate((e) => '$e'.contains('not in $checksumFile'))));
  });

  test('recognizes universal Mach-O libraries (thinned for Flutter)', () {
    File write(String name, List<int> bytes) => File('${temp.path}/$name')..writeAsBytesSync(bytes);
    expect(isUniversalMachO(write('fat', [0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 2, 0, 0])), isTrue);
    expect(isUniversalMachO(write('fat64', [0xca, 0xfe, 0xba, 0xbf, 0, 0, 0, 2])), isTrue);
    expect(isUniversalMachO(write('thin', [0xcf, 0xfa, 0xed, 0xfe, 0x0c, 0, 0, 1])), isFalse);
    expect(isUniversalMachO(write('class', [0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 65])), isFalse);
    expect(isUniversalMachO(write('short', [0xca, 0xfe])), isFalse);
  });

  test('parses sha256sum output', () {
    final a = 'a' * 64, b = 'B' * 64;
    expect(parseChecksums('$a  ./linux-x64/libx.so\n$b *win-x64/x.dll\n\ngarbage\n'),
        {'linux-x64/libx.so': a, 'win-x64/x.dll': 'b' * 64});
  });
}
