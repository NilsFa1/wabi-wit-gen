# Flutter-Beispiel

Flutter-App auf dem Wabi-Client-SDK. Die native Core-Bibliothek bündelt der
Build-Hook des SDK (`sdk/dart/hook/build.dart`, Dart native assets) für jede
Plattform automatisch; es gibt keine manuellen Schritte für jniLibs,
xcframework, DLL oder dylib.

```bash
# 1. Core für die Zielplattform(en) bauen -> client-sdk/build/native/
scripts/build-native.sh linux        # bzw. windows | apple | android
# 2. Plattformprojekte anlegen (nicht eingecheckt) und starten
cd examples/flutter
flutter create --platforms=android,ios,macos,windows,linux .
flutter run -d <gerät>
flutter test                          # Widget-Test gegen den Core des Hosts
flutter test integration_test -d <gerät>
```

Konfiguration des Hooks (`library`, `prebuilt_dir`, `release_url`, `required`)
in der `pubspec.yaml`: siehe [docs/FLUTTER.md](../../docs/FLUTTER.md).
