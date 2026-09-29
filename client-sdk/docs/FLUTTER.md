# Dart-SDK in Flutter verwenden

Das Dart-SDK (`sdk/dart`, Paket `acme_client`) ist reines Dart mit `dart:ffi`.
Es hat keine Flutter-Abhängigkeit und läuft deshalb unverändert in Flutter-Apps
(Android, iOS, macOS, Windows, Linux). Nicht unterstützt ist **Flutter Web**,
weil es dort kein `dart:ffi` gibt. Dafür ist der Weg über die WebAssembly-
Komponente vorgesehen (siehe [COMPONENT_MODEL.md](COMPONENT_MODEL.md)).

## Einbinden

```yaml
# pubspec.yaml der App
dependencies:
  acme_client:
    path: ../client-sdk/sdk/dart   # oder git: / eigener Pub-Server
```

Hinweis: `sdk/dart/lib/src/{runtime,generated}` sind Symlinks auf
`runtime/dart` und `generated/dart`, analog zum Swift-Paket. Für einen
Pub-Server oder unter Windows ohne Symlink-Unterstützung werden die beiden
Verzeichnisse beim Paketieren kopiert.

## Native Bibliothek mitliefern

`NativeBackend.shared` lädt je Plattform (mit `ACME_NATIVE_LIB` überschreibbar):

| Plattform | geladen wird | Herkunft (`scripts/build-native.sh`) | Ablage im Flutter-Projekt |
|---|---|---|---|
| Android | `libacme_client.so` | `build/native/android/jniLibs/<abi>/` | `android/app/src/main/jniLibs/<abi>/` |
| iOS | `DynamicLibrary.process()` (statisch gelinkt) | `build/native/apple/AcmeClient.xcframework` | in Xcode einbinden oder per Podspec `vendored_frameworks`; Linker-Flag `-force_load`, damit die Symbole nicht wegoptimiert werden |
| macOS | `libacme_client.dylib` | `build/native/osx/` | in `Contents/Frameworks` des App-Bundles |
| Windows | `acme_client.dll` | `build/native/win-x64/` | neben die `.exe` (CMake `install`) |
| Linux | `libacme_client.so` | `build/native/linux-x64/` | `bundle/lib/` (CMake `install`) |

Mittelfristig ist die Paketierung über Dart **native assets**
(`hook/build.dart`) sauberer: Das Paket baut oder lädt die Bibliothek dann
selbst, und Flutter bündelt sie automatisch für jede Plattform (siehe
[ROADMAP.md](ROADMAP.md)).

## Threads, Isolates und UI

* Completion-Callbacks kommen von Threads des Cores. `NativeCallable.listener`
  kehrt dort sofort zurück und führt den Dart-Code später in der Event-Loop
  **des Isolates aus, das den Aufruf gemacht hat**. In einer Flutter-App ist das
  der UI-Isolate. `setState` nach `await client.getCustomer(...)` ist deshalb
  ohne weiteres Dispatching korrekt; die ABI selbst setzt aber keinen UI-Thread
  voraus (ABI.md §6).
* Die gesamte Netzwerkarbeit passiert im Core. Der UI-Isolate kodiert und
  dekodiert nur kleine Payloads; ein eigener Hintergrund-Isolate ist nicht nötig.
* Handles sind prozessweit gültig. Ein `AcmeClient` gehört aber zu seinem
  Isolate, weil Dart-Objekte nicht zwischen Isolates geteilt werden.

## Beispiel-Widget

```dart
class CustomerView extends StatefulWidget {
  const CustomerView({super.key, required this.client});
  final AcmeClient client;
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
