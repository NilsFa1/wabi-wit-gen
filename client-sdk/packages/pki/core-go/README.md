# gematik:pki — Go core (gempki)

The second core of the `gematik:pki` package, next to the Java core
(`../core-java`, gemLibPki via GraalVM). Both implement the same WIT contract
(`../wit/pki.wit`); the SDKs (C#, Java, Swift, Dart) cannot tell them apart and
run the same tests against both (`../run-tests.sh`).

| | Java core | Go core (this) |
|---|---|---|
| Basis | ref-GemLibPki (Java) | [gempki](https://github.com/gematik/zero-lab/tree/main/go/gempki) v0.23.0 (Go) |
| Native library | GraalVM native-image, two libraries, ~90 MB | `go build -buildmode=c-shared/c-archive`, one library, ~8–10 MB |
| Platforms | Linux/macOS/Windows; Android/iOS only via Gluon (experimental) | Linux, Windows, macOS, Android, iOS (`build.sh`) |
| Algorithms | RSA + ECC | ECC (Brainpool and NIST curves) |
| Licence | Apache-2.0 | **EUPL-1.2** |

## What it checks

TUC_PKI_018 in the classic TI trust model: **the TSL is the trust source.**
gempki supplies certificate and TSL parsing (with Brainpool), the admission
extension and the OCSP client; `tuc_pki_018.go` transcribes the checks of
gemLibPki 5.0.2 step by step — same order, same error codes, same texts:

1. issuer is a CA/PKC service of the TSL (`TE_1027`, `SE_1023`)
2. validity (`SE_1021`), signature (`SE_1024`), CA not revoked in the TSL before issuance (`SE_1036`)
3. OCSP if requested, at the TSL's ServiceSupplyPoint, responders from the TSL
   (`TE_1026`, `TE_1032`, `SE_1030`, `SE_1031`, `SW_1047`, `TW_1044`)
4. per profile, first match wins: key usage (`SE_1016`), extended key usage
   (`SE_1017`), certificate type OID (`SE_1033`, `SE_1018`), CA authorized for the
   type in the TSL (`SE_1061`), critical extensions (`CUSTOM_CERTIFICATE_EXCEPTION`)

gempki on its own follows the TI 2.0 model (trust from the gematik roots, TSL
only for SubCAs and responders); this core does not use that path.

Not yet: TSL signature verification (TUC_PKI_001), TSL download/update and
NextUpdate, an OCSP response cache, OCSP nonce. RSA profiles return
`UNSUPPORTED_RSA_PROFILE`.

## Build

```
go test ./...                     # unit tests against ../tests/fixtures
./build.sh linux-x64              # also: linux-arm64, windows-x64 (zig), android (NDK),
                                  #       macos, ios (on macOS, Xcode)
```

Output lands in `build/pki-go/<target>/`. CI: `.github/workflows/pki-go.yml`.

## Licence

This directory is licensed under the **EUPL-1.2** (`LICENSE`), because it
builds on gempki, which is EUPL-1.2. See `NOTICE` for the components of the
built library.

The licence boundary is the C ABI: the generator, the ABI runtimes and the
SDKs in this repository are separate works that talk to the library only
through its exported functions and are not covered by this licence. Whoever
distributes the library (as a file, inside an app, or by offering its
functions over a network) has to make this directory's source available
under the EUPL. Static linking on iOS (`GematikPki.xcframework`) and App Store
terms should be reviewed legally before a release.
