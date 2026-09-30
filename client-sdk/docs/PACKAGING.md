# Plan: gematik:pki als eigenes SDK-Repo mit Paketen

**Status:** geplant, nicht umgesetzt. Die nativen Bibliotheken für alle
Plattformen baut bereits der Workflow `pki-go` (Go-Core); es fehlen das
Zusammenstellen der Pakete, das Veröffentlichen und das Repo dafür.

## Ziel

Eine App bindet das PKI-SDK wie jede andere Abhängigkeit ein, ohne Generator,
ohne manuelles Kopieren nativer Bibliotheken:

| Ökosystem | Eintrag in der App |
|---|---|
| .NET | `dotnet add package <Prefix>.Pki` |
| Java (Desktop/Server) | Maven/Gradle-Abhängigkeit `<group>:pki` |
| Android | Gradle-Abhängigkeit `<group>:pki-android` (AAR) |
| Swift (iOS/macOS) | Swift Package über die Repo-URL |
| Flutter/Dart | `pki` in der `pubspec.yaml` |

## Repo-Schnitt

```
UniWtiGen (dieses Monorepo, bleibt die Quelle)     <prefix>-pki-sdk (neues Repo, nur Auslieferung)
  generator/            acme-wit-gen                   Package.swift              (SwiftPM, im Root)
  abi/, runtime/*       ABI + Runtimes     ── Export ─▶ dotnet/                    (NuGet-Projekt)
  packages/pki/wit      Vertrag (WIT)                  java/, android/            (Maven JAR, AAR)
  packages/pki/core-go  Go-Core (EUPL)                 dart/                      (pub-Paket, native assets)
  packages/pki/core-java Referenz-Core                 LICENSE, NOTICE, CHANGELOG
  Tests, CI                                            .github/workflows/release.yml
```

* **Das Monorepo bleibt die einzige Quelle.** Generator, Runtimes, WIT, Cores und
  Tests leben hier; nur hier wird entwickelt.
* **Das SDK-Repo enthält nur, was ausgeliefert wird:** generierten Code, die
  benötigten Runtime-Dateien, Paket-Manifeste und den Release-Workflow. Es wird
  von einem Export-Schritt befüllt (`scripts/export-pki-sdk.sh`, in CI als Pull
  Request in das SDK-Repo), nie von Hand bearbeitet.
* **Warum ein eigenes Repo:** SwiftPM erwartet `Package.swift` im Root und
  versioniert über Git-Tags; pub.dev und Maven wollen ein Paket pro Quelle; die
  Nutzer sehen weder Generator noch Referenz-Core; die Lizenzen bleiben sauber
  getrennt.
* **Alternative:** alles im Monorepo mit einem Release-Workflow. Geht für NuGet,
  Maven und pub.dev, aber SwiftPM bräuchte dann ein `Package.swift` im Root
  dieses Repos. Nur sinnvoll, wenn kein Swift Package gebraucht wird.

## Pakete

| Paket | Inhalt | Native Bibliothek | Noch zu tun |
|---|---|---|---|
| **NuGet** `<Prefix>.Pki` | generiertes C#-SDK + C#-Runtime (net8.0) | `runtimes/{linux-x64,linux-arm64,win-x64,osx-arm64,osx-x64,android-arm64,android-x64}/native/` | iOS (MAUI): `NativeReference` auf das xcframework und `DllImport("__Internal")` im Loader |
| **Maven** `<group>:pki` | Java-SDK (Panama, JDK 22+) | als Ressourcen im JAR (`native/<os>-<arch>/`), beim Start in ein Temp-Verzeichnis entpackt | Entpack-Lader in `runtime/java` (heute: Pfad per `ACME_NATIVE_LIB`/`java.library.path`) |
| **AAR** `<group>:pki-android` | Java-SDK mit JNI-Binding | `jniLibs/{arm64-v8a,x86_64}/libgematik_pki.so` + `libacme_jni.so` | Gradle-Build fürs AAR; `libacme_jni.so` per NDK für beide ABIs bauen |
| **SwiftPM** | Swift-SDK + Swift-Runtime | `binaryTarget(url:checksum:)` auf `GematikPki.xcframework.zip` im GitHub-Release | macOS-Slices ins xcframework aufnehmen (heute nur iOS + Simulator) |
| **pub.dev** `pki` | Dart-SDK + Dart-Runtime | über **Dart native assets** (`hook/build.dart`): lädt pro Plattform die passende Bibliothek aus dem GitHub-Release (Prüfsumme im Paket) | siehe unten „Flutter“ |

Die nativen Bibliotheken kommen ausschließlich aus dem Go-Core (`pki-go`):
Linux x64/arm64 (`.so`), Windows x64 (`.dll`), macOS universal (`.dylib`),
Android arm64-v8a/x86_64 (`jniLibs`), iOS Gerät + Simulator (`.xcframework`).
Der Java-Core wird nicht ausgeliefert (siehe `packages/pki/core-java/README.md`).

### Flutter

Die Flutter-Paketierung über Dart native assets steht in `docs/ROADMAP.md`
(„Priorität HOCH“) bisher für `acme_client`. Für pki gilt dasselbe:

* `hook/build.dart` liefert pro Zielplattform die Bibliothek als `CodeAsset`
  (Quelle: Release-Artefakte mit Prüfsumme; `ACME_NATIVE_LIB` bleibt Override).
* Der Hook sollte vom Generator mit erzeugt werden (ein Template für alle
  WIT-Pakete), statt pro Paket von Hand gepflegt.
* Abnahme: Beispiel-App baut und läuft auf Android (arm64 + x86_64-Emulator),
  iOS-Simulator, macOS, Windows und Linux; `flutter test` in CI.
* Übergangslösung, falls native assets auf einer Zielplattform noch nicht
  reichen: klassisches FFI-Plugin (`ffiPlugin: true`) mit Podspec (xcframework)
  und CMake (Linux/Windows).

## Versionierung

* SemVer, **eine Version für alle Pakete** eines Releases, Tag `pki-vX.Y.Z` im
  SDK-Repo (SwiftPM liest die Tags direkt).
* Minor-Version = WIT-Vertragsversion (`gematik:pki@X.Y`); Patch für
  Korrekturen am Core ohne Vertragsänderung. Die Laufzeitprüfung von
  Contract-ID und ABI-Version (heute schon in allen SDKs) fängt falsch
  kombinierte Bibliotheken ab.
* `CHANGELOG.md` im SDK-Repo, erzeugt aus den Commits des Monorepos.

## Release-Ablauf

1. Tag im Monorepo (`pki-vX.Y.Z`) oder manueller Start.
2. `pki-go`-Jobs bauen und testen alle nativen Bibliotheken (wie heute), plus
   Smoke-Tests: Android-Emulator (x86_64), iOS-Simulator, Windows-Runner.
3. Export in das SDK-Repo (Pull Request), dort Paket-Builds:
   `dotnet pack`, Gradle (`jar`, `aar`), xcframework-Zip + Prüfsumme in
   `Package.swift`, `dart pub publish --dry-run`.
4. GitHub-Release mit allen Paketen und Bibliotheken als Assets, `SHA256SUMS`,
   SBOM (CycloneDX).
5. Veröffentlichen (siehe Registries).

## Registries und Zugänge

| Registry | Zugang | Einrichtung |
|---|---|---|
| GitHub Packages (NuGet, Maven) | `GITHUB_TOKEN` | keine, **erster Schritt** |
| nuget.org | API-Key als Secret | Konto + Paket-Präfix reservieren |
| Maven Central | Sonatype Central Portal, GPG-Signatur | Namespace (Domain) verifizieren, GPG-Schlüssel als Secret |
| pub.dev | OIDC aus GitHub Actions (kein Secret) | Publisher (verifizierte Domain) |
| SwiftPM | keine Registry, Git-Tags | nur das SDK-Repo |

Die Konten und Schlüssel müssen die Verantwortlichen anlegen; im Workflow
werden sie nur als Secrets referenziert.

## Lizenz

* Jede Bibliothek aus `core-go` enthält gempki und steht unter **EUPL-1.2**.
  Jedes Paket liefert deshalb `LICENSE` (EUPL), `NOTICE` und einen Verweis auf
  den Quellcode von `core-go` in genau der Version mit (Release-Tag im
  Monorepo). Der Release-Workflow prüft das (Datei vorhanden, Tag erreichbar).
* Die SDKs, Runtimes und der Generator sind eigene Werke hinter der C-ABI und
  behalten die Lizenz des Projekts.
* Vor dem ersten öffentlichen Release rechtlich prüfen: statisches Linken auf
  iOS (xcframework) und App-Store-Bedingungen (siehe `docs/ROADMAP.md`).

## Offene Entscheidungen

* **Name und Namensraum:** Paket-IDs wie `Gematik.Pki` oder `de.gematik` sehen
  nach einem offiziellen gematik-Paket aus. Ohne Absprache mit der gematik
  einen eigenen Präfix wählen (Organisation/Domain des Herausgebers).
* **Organisation und Sichtbarkeit** des SDK-Repos (öffentlich/privat).
* **Unterstützte Plattformen v1:** Windows arm64, Linux musl, Android armv7?
* **Minimale Versionen:** .NET 8, JDK 22 (Panama) bzw. Android API 24,
  iOS 13, macOS 11, Dart 3.x/Flutter.

## Meilensteine

| # | Schritt | Aufwand |
|---|---|---|
| 1 | Export-Skript + SDK-Repo-Gerüst + Release mit Assets (ohne Veröffentlichen) | ~2 Tage |
| 2 | Loader-Anpassungen: Java-Entpacken, C# iOS `__Internal`, macOS-Slices im xcframework | ~1 Tag |
| 3 | Veröffentlichen auf GitHub Packages (NuGet, Maven) + SwiftPM-Tags | ~1 Tag |
| 4 | Flutter über native assets, generierter Hook, Beispiel-App auf allen Plattformen | ~2–3 Tage |
| 5 | Smoke-Tests auf Android-Emulator, iOS-Simulator, Windows in CI | ~1 Tag |
| 6 | Öffentliche Registries (nuget.org, Maven Central, pub.dev) | ~1 Tag + Konten |
