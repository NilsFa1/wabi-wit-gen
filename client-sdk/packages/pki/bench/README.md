# gematik:pki — performance: SDK + native library vs. gemLibPki directly on the JVM

`run-bench.sh` measures the same operation — TUC_PKI_018 verification of a
valid SMC-B certificate against the test TSL, OCSP off — in four setups:

| Setup | What runs |
|---|---|
| **JVM direct** | a Java app calls the wrapper (gemLibPki) in-process — HotSpot/Graal **JIT** |
| **Java SDK → native (Panama / JNI)** | Java app → generated SDK → ABI → `libgematik_pki.so` (GraalVM **AOT**) |
| **C# SDK → native** | .NET app → generated SDK → ABI → same library |
| **ABI round trip** | raw C caller, one async call the core rejects immediately (no crypto) |

## Results (4 vCPU cloud VM, one run each; ±15 % run-to-run noise)

Latency = sequential calls after 3000 warm-up calls; throughput = 8 parallel callers.

| Setup | 1st call (incl. TSL parse) | verify p50 | verify mean | verify p99 | throughput (8 threads) | SDK+ABI overhead p50 ¹ |
|---|---|---|---|---|---|---|
| JVM direct, HotSpot 25 (JIT) | 2394 ms | **316 µs** | 362 µs | 790 µs | 8 932 /s | – |
| JVM direct, GraalVM 21 (JIT) | 942 ms | 348 µs | 384 µs | 826 µs | **9 119 /s** | – |
| Java SDK → native, Panama | 76 ms | 688 µs | 748 µs | 1 863 µs | 7 367 /s | 65 µs |
| Java SDK → native, JNI | **47 ms** | 584 µs | 638 µs | 1 320 µs | 7 783 /s | 50 µs |
| C# SDK → native | 59 ms ² | 527 µs | 593 µs | 1 499 µs | 7 914 /s | 81 µs |
| ABI round trip only (C) | – | – | – | – | – | **33 µs** |
| *native image with `-O3 -march=native --gc=G1`:* | | | | | | |
| Java SDK → native, JNI | 52 ms | 556 µs | 629 µs | 1 526 µs | 8 918 /s | 19 µs |
| C# SDK → native | 69 ms ² | 598 µs | 663 µs | 2 015 µs | 8 247 /s | 77 µs |

¹ "rejected call": the same `verify` with an empty profile list, which the core
answers without any certificate work — i.e. marshalling + FFI + thread hand-offs.
² incl. loading the library.

## What this means

* **Per call, the native library is ~1.7–2× slower than gemLibPki warmed up on
  the JVM** (≈ 550–690 µs vs. ≈ 320–350 µs p50). Two parts:
  * **~20–80 µs call overhead** (SDK + ABI): mostly two thread hand-offs
    (host → core thread → completion callback), not marshalling. The raw ABI
    round trip is ~33 µs; the SDK runtimes add one more hop (JNI < Panama < .NET here).
  * **~200 µs slower computation**: AOT-compiled code (no JIT, no runtime
    profiling; BouncyCastle's BigInteger/ECDSA math benefits from HotSpot
    intrinsics and profile-guided inlining). `-O3 -march=native` barely changes
    latency; G1 instead of Serial GC helps throughput (≈ JVM level) but doubles
    the image (65 → 120 MB).
* **Throughput under load is close to the JVM** (7.4–8.9 k vs. 8.9–9.1 k
  verifications/s): the per-call overhead parallelises well.
* **Startup is 15–50× better native**: first verification after 47–76 ms
  instead of 0.9–2.4 s (JVM class loading + JIT warm-up of gemLibPki/JAXB/BC).
  A mobile or desktop app that verifies a handful of certificates never reaches
  the JVM's warm steady state.
* **No JVM in the app process** — the point of the exercise for C#, Dart/Flutter,
  Swift; for a Java *server* that already runs on a JVM, using gemLibPki
  directly stays the fastest option.

## Levers not tried yet

* **PGO** (Oracle GraalVM `--pgo-instrument` → run this benchmark → `--pgo`):
  the usual way to close most of the AOT-vs-JIT gap for compute-heavy code.
* **Fewer hand-offs**: run short `verify` calls inline on the calling thread
  (a sync WIT func, or an "eager" future completed before `call_async`
  returns) — saves most of the 20–80 µs where latency matters.
* Keep `CertificateFactory`/`TucPki018Verifier` instances per verifier instead
  of per call (applies to both JVM and native).
