# Flutter-Beispiel

Flutter-App auf dem SDK des Beispiels. Die native Core-Bibliothek bündelt der
Build-Hook des SDK (`example/sdk/dart/hook/build.dart`, Dart native assets) für jede
Plattform automatisch; es gibt keine manuellen Schritte für jniLibs,
xcframework, DLL oder dylib.

```bash
# 1. Core für die Zielplattform(en) bauen -> example/sdk/native/
bash example/sdk/core-rust/build-native.sh example/core-rust host   # bzw. android | ios | macos | windows-x64
# 2. Plattformprojekte anlegen (nicht eingecheckt) und starten
cd example/apps/flutter
flutter create --platforms=android,ios,macos,windows,linux .
flutter run -d <gerät>
flutter test                          # Widget-Test gegen den Core des Hosts
flutter test integration_test -d <gerät>
```

Konfiguration des Hooks (`library`, `prebuilt_dir`, `release_url`, `required`)
in der `pubspec.yaml`: siehe [docs/FLUTTER.md](../../../docs/FLUTTER.md).
