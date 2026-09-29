# Welche Sprachen taugen als Core? — Bewertung C# (NativeAOT) und Kotlin/Native

Frage: Kann man den Core statt in Rust auch in **Kotlin** oder **C#** schreiben,
wenn man ihn nativ kompiliert?

**Kurzantwort:**

* **C# mit NativeAOT: ja, nachgewiesen.** `alt-cores/csharp` ist ein
  vollständiger Core in C#, kompiliert zu einer normalen `libacme_client.so`
  (1,8 MB, hängt nur an `libc`/`libm`, keine installierte .NET-Runtime nötig).
  Er besteht **alle** Tests im vollen Profil: C-Konformität, C#-, Java-
  (Panama und JNI) und Dart-SDK. Die SDKs sind dabei unverändert, es sind
  sogar dieselben Binaries.
* **Kotlin/Native: grundsätzlich ja, aber mit mehr Reibung — hier nicht
  gebaut.** Der Compiler ließ sich in dieser Umgebung nicht herunterladen (die
  Maven-Downloads leiten auf GitHub Releases um, und die Toolchain-Pakete von
  `download.jetbrains.com` sind durch die Netzwerk-Policy gesperrt). Die
  Bewertung unten beruht auf der Dokumentation, nicht auf einem Test.

## Was muss eine Sprache können, um Core zu sein?

Das ergibt sich direkt aus [`ABI.md`](../abi/ABI.md) §11:

| Anforderung | Warum |
|---|---|
| 1. C-Funktionen mit **exakt vorgegebenen Namen** exportieren (`acme_call_sync` …) | SDKs binden per Symbolname |
| 2. **Structs by value** annehmen (`acme_bytes`, `acme_buffer`) | so ist die ABI definiert |
| 3. **C-Funktionszeiger aufrufen**, und zwar von eigenen Threads aus | Completion-Callback |
| 4. Aufrufe **von fremden Threads** entgegennehmen (JVM-, .NET-, Dart-, Swift-Threads) | SDKs rufen von beliebigen Threads |
| 5. Exceptions/Panics an der Grenze **abfangen** | nichts darf in C „durchfallen“ |
| 6. Eigene Hintergrund-Arbeit (Netzwerk, Timer) ohne den Aufrufer-Thread | Futures laufen im Core |
| 7. Als `.so`/`.dll`/`.dylib` bzw. statisch fürs iOS-xcframework bauen, für alle Zielplattformen | Artefakte |
| 8. Netzwerk (HTTP/WebSocket) und Krypto auf allen Zielplattformen | der eigentliche Use Case |

## C# mit NativeAOT — nachgewiesen

| Anforderung | Umsetzung in `alt-cores/csharp` | Ergebnis |
|---|---|---|
| 1. Exportnamen | `[UnmanagedCallersOnly(EntryPoint = "acme_call_sync")]` | ✅ genau 17 Exporte |
| 2. Structs by value | blittable `struct AcmeBytes { byte* Ptr; nuint Len; }` | ✅ |
| 3. Callbacks | `delegate* unmanaged[Cdecl]<void*, ulong, void>` vom Threadpool aus | ✅ |
| 4. Fremde Threads | NativeAOT hängt fremde Threads automatisch an | ✅ (JVM-, Dart-, CoreCLR-Threads getestet) |
| 5. Exceptions | `try/catch` in jedem Export → `ACME_ERR_INTERNAL` | ✅ |
| 6. Async | `async/await`, `Task`, `Channel<T>`, `CancellationToken` | ✅ Race-Tests, Abbruch, Streams |
| 7. Plattformen | Linux/Windows/macOS: offiziell unterstützt. iOS: NativeAOT seit .NET 8 (vor dem Einsatz den aktuellen Support-Status der verwendeten .NET-Version prüfen). **Android: der schwächste Punkt**, NativeAOT für Android (`linux-bionic`) war lange experimentell; vor einer Entscheidung für Android gezielt verifizieren | ⚠️ Android prüfen |
| 8. Netzwerk/Krypto | `System.Net.WebSockets`, `HttpClient`, `System.Security.Cryptography` sind in der BCL. Unter Linux nutzt die Krypto zur Laufzeit OpenSSL | ✅ (im Prototyp nicht genutzt) |

**Besonderheit:** Der C#-Core verwendet die **generierten** C#-Typen und den
Codec (`Types.g.cs`, `WireCodec.g.cs`) sowie die MessagePack-Runtime des SDKs.
Nur die Tabelle „Method-ID → Methode“ (`Dispatch.cs`) ist von Hand geschrieben.
Ein C#-Core-Emitter könnte auch sie erzeugen, analog zum Rust-Core.

**Kosten und Grenzen:**

* **Größe:** 1,8 MB für den Prototyp. Mit `HttpClient`, WebSockets und Krypto
  eher 5–10 MB (Rust: ca. 1–3 MB).
* **Eigene Runtime im Prozess:** GC und Threadpool. Das kostet einige MB
  Speicher und bringt GC-Pausen im Core. Neben einer .NET-App (CoreCLR) läuft
  dann eine zweite, unabhängige .NET-Runtime; das funktioniert (getestet), ist
  aber doppelte Infrastruktur.
* **Kein Cross-OS-Build:** Windows-DLLs werden auf Windows gebaut, macOS/iOS
  auf dem Mac, Linux auf Linux. CI braucht also pro Plattform einen Runner. Rust
  und Zig können hier cross-kompilieren.
* **Trimming/AOT-Regeln:** kein Reflection-lastiger Code, JSON nur mit
  Source-Generatoren. Für einen Core ist das meist unproblematisch.

**Wann sinnvoll:** wenn das Team C#-Know-how hat und die Zielplattformen vor
allem Desktop/Server/iOS sind. Für Android erst nach einem Proof auf einem Gerät.

## Kotlin/Native — bewertet, nicht gebaut

| Anforderung | Kotlin/Native | Einschätzung |
|---|---|---|
| 1. Exportnamen | Standardmäßig exportiert K/N eine eigene API (`libacme_symbols()->kotlin.root…`). Mit `@CName("acme_call_sync")` (experimentell, `@OptIn(ExperimentalNativeApi)`) lassen sich flache C-Namen exportieren | ⚠️ geht, aber experimentell |
| 2. Structs by value | Exportierte Funktionen erlauben im Wesentlichen primitive Typen und Pointer; `acme_bytes` **by value** anzunehmen ist nicht vorgesehen | ⚠️ **Knackpunkt**, Lösung siehe unten |
| 3. Callbacks | C-Funktionszeiger über cinterop aufrufbar (`CPointer<CFunction<…>>.invoke`) | ✅ |
| 4. Fremde Threads | Mit dem neuen Memory Manager (Standard seit Kotlin 1.7.20) werden fremde Threads automatisch angebunden | ✅ |
| 5. Exceptions | `try/catch` in jeder exportierten Funktion; ungefangene Kotlin-Exceptions beenden den Prozess | ✅ mit Disziplin |
| 6. Async | Coroutines (`kotlinx.coroutines`), `Dispatchers.Default` multithreaded, `Channel` | ✅ |
| 7. Plattformen | Linux, Windows (mingw), macOS, iOS: gut unterstützt. Android als *native* Ziel (`androidNativeArm64` …) gibt es, ist aber ein Randfall; Apple-Ziele nur auf macOS-Hosts | ⚠️ Android, Build-Hosts |
| 8. Netzwerk/Krypto | Keine Standardbibliothek dafür. Ktor-Client braucht je Plattform eine Engine (Darwin, Curl, WinHttp); für `androidNative` gibt es keine etablierte Engine. Krypto über Drittbibliotheken (z. B. `cryptography-kotlin` mit OpenSSL/CryptoKit) | ⚠️ größter Aufwand |

**Lösung für Punkt 2, generisch und unabhängig von Kotlin:** ein ca. 40 Zeilen
großer C-Adapter, der die 17 ABI-Funktionen exportiert und `acme_bytes`
in `ptr` + `len` zerlegt, bevor er die Kotlin-Funktionen aufruft. Das ist
dasselbe Muster wie der JNI-Shim (`runtime/java/jni/acme_jni.c`). Alternativ
könnte eine ABI-Version 2 Structs by value ganz vermeiden (Zeiger + Länge als
separate Parameter). Das wäre für manche Sprachen etwas bequemer, ist aber
kein Muss.

**Einordnung:** Kotlin spielt seine Stärke eher anders aus. Mit **Kotlin
Multiplatform** baut man Android/JVM-Code *ohne* native ABI und für iOS ein
Swift-Framework. Das ist aber ein anderes Architekturmodell (ein SDK pro
Plattform aus einer Codebasis) als hier (ein nativer Core, dünne SDKs). Als
*nativer Core hinter dieser ABI* ist Kotlin/Native machbar, bringt aber die
meiste Reibung: experimentelle Exporte, Struct-Adapter, schwache Netzwerk- und
Krypto-Story auf Android-native.

## Gesamtvergleich

| | Rust | Go | Zig | C# NativeAOT | Kotlin/Native |
|---|---|---|---|---|---|
| In diesem Repo | ✅ voll | ✅ voll | ✅ Stub | ✅ voll | ❌ (nicht baubar hier) |
| Flache C-Exporte | ✅ | ✅ (cgo) | ✅ | ✅ | ⚠️ experimentell |
| Structs by value | ✅ | ✅ | ✅ | ✅ | ⚠️ Adapter nötig |
| Runtime/GC im Prozess | nein | ja (Go-Runtime) | nein | ja | ja |
| Binärgröße (grob) | 1–3 MB | 3–6 MB | < 1 MB | 2–10 MB | 1–5 MB |
| Cross-Compile | ✅ sehr gut | ✅ gut | ✅ sehr gut | ❌ pro OS | ⚠️ Apple nur auf macOS |
| iOS | ✅ | ⚠️ möglich (c-archive), selten | ✅ | ✅ (Status prüfen) | ✅ |
| Android (native) | ✅ | ✅ | ✅ | ⚠️ prüfen | ⚠️ Randfall |
| HTTP/WebSocket/Krypto | ✅ Crates | ✅ stdlib | ⚠️ wenig Ökosystem | ✅ BCL | ⚠️ pro Plattform |

**Empfehlung:** Rust bleibt der Standard-Core, weil es die beste Kombination
aus Größe, Cross-Compile, Android/iOS und Ökosystem bietet. **C# NativeAOT ist
eine ernsthafte, nachgewiesene Alternative**, besonders für ein C#-starkes
Team; Android vorher gezielt prüfen. Kotlin/Native nur, wenn es einen
konkreten Grund gibt, und dann mit dem generischen C-Adapter für die
Struct-Parameter.

Wichtig für alle Varianten: **Die SDKs ändern sich nie.** Welche Sprache den
Core implementiert, entscheidet allein, ob `tests/compatibility/run.sh` für
diesen Core grün ist.
