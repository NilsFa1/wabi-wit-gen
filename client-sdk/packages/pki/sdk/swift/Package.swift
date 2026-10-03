// swift-tools-version:5.9
// gematik:pki PoC Swift SDK = runtime written by wabi-wit-gen (Sources/GematikPki/Generated/Runtime)
//                           + generated code (Sources/GematikPki/Generated -> packages/pki/generated/swift)
// linked against a gematik:pki core (libgematik_pki: core-go or core-java).
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
        .systemLibrary(name: "CGematikPki", path: "Sources/CGematikPki"),
        .target(name: "GematikPki", dependencies: ["CGematikPki"], path: "Sources/GematikPki"),
        .testTarget(name: "GematikPkiTests", dependencies: ["GematikPki"], path: "Tests/GematikPkiTests"),
    ]
)
