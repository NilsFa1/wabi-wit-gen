// swift-tools-version:5.9
// gematik:pki PoC Swift SDK = shared runtime (Sources/GematikPki/Runtime -> runtime/swift)
//                           + generated code (Sources/GematikPki/Generated -> packages/pki/generated/swift)
// linked against the Java core compiled with GraalVM native-image (libgematik_pki).
//
// macOS: `swift test -Xlinker -L<dir with libgematik_pki.dylib>`.
import PackageDescription

let package = Package(
    name: "GematikPki",
    platforms: [.macOS(.v12), .iOS(.v15)],
    products: [
        .library(name: "GematikPki", targets: ["GematikPki"]),
    ],
    targets: [
        .systemLibrary(name: "CAcmeClient", path: "Sources/CAcmeClient"),
        .target(name: "GematikPki", dependencies: ["CAcmeClient"], path: "Sources/GematikPki"),
        .testTarget(name: "GematikPkiTests", dependencies: ["GematikPki"], path: "Tests/GematikPkiTests"),
    ]
)
