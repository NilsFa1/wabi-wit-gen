# Dart-SDK in Flutter verwenden

Das Dart-Paket, das der Generator schreibt (im Beispiel `example/sdk/dart`,
Paket `demo_client`), ist reines Dart mit `dart:ffi`.
Es hat keine Flutter-Abhängigkeit und läuft deshalb unverändert in Flutter-Apps
(Android, iOS, macOS, Windows, Linux). Nicht unterstützt ist **Flutter Web**,
weil es dort kein `dart:ffi` gibt. Dafür ist der Weg über die WebAssembly-
Komponente vorgesehen (siehe [COMPONENT_MODEL.md](COMPONENT_MODEL.md)).

## Einbinden

```yaml
# pubspec.yaml der App
dependencies:
  demo_client:
    path: ../example/sdk/dart   # oder git: / eigener Pub-Server
```

Mehr ist nicht nötig: Die native Core-Bibliothek bringt das Paket über
**Dart native assets** selbst mit (Flutter ≥ 3.38 bzw. Dart ≥ 3.10). Es gibt
keine manuellen Schritte für jniLibs, xcframework, DLL oder dylib. Lauffähiges
Beispiel mit Widget- und Integrationstest: [`example/apps/flutter`](../example/apps/flutter).

Das Paket ist vollständig generiert: `pubspec.yaml`, `hook/build.dart`, die
Bibliotheksdatei und unter `lib/src/generated` der Code samt Runtime.

## Native Bibliothek: Build-Hook (Dart native assets)

`hook/build.dart` des Pakets ruft den vom Generator erzeugten Hook
(`lib/src/generated/build_hook.dart`) auf; die Logik ist generisch
(`generator/runtime/dart/native_assets.dart`, im Paket unter `lib/src/generated/runtime/`) und für jedes WIT-Paket gleich. Flutter, `dart run` und
`dart test` führen den Hook pro Zielplattform und Architektur aus und bündeln
die gelieferte Bibliothek:

| Plattform | landet in (automatisch) |
|---|---|
| Android | `lib/<abi>/` im APK/AAB |
| iOS, macOS | als Framework im App-Bundle (Universal-Bibliotheken werden pro Architektur ausgedünnt) |
| Windows | neben der `.exe` |
| Linux | `bundle/lib/` |
| `dart run` / `dart test` / `flutter test` | `.dart_tool/`, für den Host |

Woher die Bibliothek kommt, legt die **App** in ihrer `pubspec.yaml` fest
(user-defines). Die erste zutreffende Quelle gewinnt:

```yaml
hooks:
  user_defines:
    demo_client:                     # Name des SDK-Pakets
      library: ../core/libdemo_client.so     # 1. genau eine Datei (nur für den Host sinnvoll)
      prebuilt_dir: ../sdk/native            # 2. Verzeichnis mit Unterordnern je Ziel (unten)
      release_url: https://…/releases/v1.0.0 # 3. Download, geprüft gegen native/SHA256SUMS des Pakets
      required: true                         # Build-Fehler statt Laufzeit-Fallback, wenn nichts gefunden wird
```

Ohne Angaben sucht der Hook in `../native` neben dem Paket, also in
`<out>/native`, wohin das erzeugte `build-native.sh` schreibt (im Beispiel
`example/sdk/native`).

Aufbau eines `prebuilt_dir` bzw. eines Releases (`<dir>/<ziel>/<datei>`):

| Ziel | Unterordner (erster vorhandener gewinnt) | Datei |
|---|---|---|
| Android | `android/jniLibs/<abi>/` (`arm64-v8a`, `armeabi-v7a`, `x86_64`) | `libdemo_client.so` |
| iOS-Gerät | `ios-arm64/`, `ios/` | `libdemo_client.dylib` |
| iOS-Simulator | `ios-simulator-<arch>/`, `ios-simulator/` | `libdemo_client.dylib` |
| macOS | `macos-<arch>/`, `macos/`, `osx/` | `libdemo_client.dylib` |
| Windows | `windows-<arch>/`, `win-<arch>/` | `demo_client.dll` |
| Linux | `linux-<arch>/` (`x64`, `arm64`) | `libdemo_client.so` |

Auf iOS braucht native assets eine **dynamische** Bibliothek; die
Build-Skripte legen deshalb neben dem xcframework (statisch, für Swift) auch
`ios/` und `ios-simulator/` an.

Beim Download (`release_url`) wird nur geladen, was in `native/SHA256SUMS` des
Pakets steht (Format von `sha256sum`, Pfade wie oben); jede Datei wird gegen
ihre Prüfsumme geprüft und im Build-Cache abgelegt. Das ist der Weg für ein
veröffentlichtes Paket.

### Laden zur Laufzeit

`NativeBackend.shared` sucht den Core in dieser Reihenfolge:

1. `WABI_NATIVE_LIB` (expliziter Pfad, z. B. Tests gegen mehrere Cores),
2. das Code-Asset des Pakets (`NativeAbi.asset()`, `@Native`-Bindings mit der
   Asset-ID `package:<paket>/src/generated/native_abi.dart`) bzw. dieselben
   Symbole im Prozess (statisch gelinktes xcframework),
3. die Namenskonvention (`libdemo_client.so`, `demo_client.dll`,
   `libdemo_client.dylib`) für von Hand mitgelieferte Bibliotheken.

Die bisherige manuelle Ablage (jniLibs, `Contents/Frameworks`, neben die
`.exe`, `bundle/lib/`, xcframework mit `-force_load`) funktioniert also weiter,
ist aber nicht mehr nötig.

## Threads, Isolates und UI

* Completion-Callbacks kommen von Threads des Cores. `NativeCallable.listener`
  kehrt dort sofort zurück und führt den Dart-Code später in der Event-Loop
  **des Isolates aus, das den Aufruf gemacht hat**. In einer Flutter-App ist das
  der UI-Isolate. `setState` nach `await client.getCustomer(...)` ist deshalb
  ohne weiteres Dispatching korrekt; die ABI selbst setzt aber keinen UI-Thread
  voraus (ABI.md §6).
* Die gesamte Netzwerkarbeit passiert im Core. Der UI-Isolate kodiert und
  dekodiert nur kleine Payloads; ein eigener Hintergrund-Isolate ist nicht nötig.
* Handles sind prozessweit gültig. Ein `DemoClient` gehört aber zu seinem
  Isolate, weil Dart-Objekte nicht zwischen Isolates geteilt werden.

## Beispiel-Widget

```dart
class CustomerView extends StatefulWidget {
  const CustomerView({super.key, required this.client});
  final DemoClient client;
  @override
  State<CustomerView> createState() => _CustomerViewState();
}

class _CustomerViewState extends State<CustomerView> {
  final _cancel = CancellationToken();
  StreamSubscription<ClientEvent>? _events;
  Customer? _customer;
  String? _error;

  @override
  void initState() {
    super.initState();
    _events = widget.client.events().listen((e) {
      if (e case ClientEventCustomerUpdated(:final value)) setState(() => _customer = value);
    });
    widget.client.getCustomer('4711', cancellationToken: _cancel).then(
      (c) => setState(() => _customer = c),
      onError: (Object e) => setState(() => _error = switch (e) {
        ClientErrorNotFound() => 'Kunde nicht gefunden',
        ClientError() => 'Fehler: $e',
        _ => '$e',
      }),
    );
  }

  @override
  void dispose() {
    _cancel.cancel();     // bricht den laufenden Request im Core ab
    _events?.cancel();    // gibt den nativen Stream frei
    super.dispose();
  }

  @override
  Widget build(BuildContext context) =>
      Text(_error ?? _customer?.name ?? 'lädt …');
}
```

`StreamBuilder(stream: client.events(), …)` funktioniert ebenso: Der Stream
startet die native Subscription beim `listen` und gibt sie beim Abmelden frei.
Pausiert der Listener, fragt das SDK keine weiteren Elemente beim Core an
(Backpressure).
