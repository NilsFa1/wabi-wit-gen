// swift-tools-version:5.9
// gematik:zero PoC Swift SDK = shared runtime (Sources/GematikZero/Runtime -> runtime/swift)
//                            + generated code (Sources/GematikZero/Generated -> packages/zero/generated/swift)
// linked against the Go core (libgematik_zero, libzero inside).
//
// macOS: `swift test -Xlinker -L<dir with libgematik_zero.dylib>`.
// iOS: link the core as an xcframework (static archive from `go build -buildmode=c-archive`).
import PackageDescription

let package = Package(
    name: "GematikZero",
    platforms: [.macOS(.v12), .iOS(.v15)],
    products: [
        .library(name: "GematikZero", targets: ["GematikZero"]),
    ],
    targets: [
        .systemLibrary(name: "CAcmeClient", path: "Sources/CAcmeClient"),
        .target(name: "GematikZero", dependencies: ["CAcmeClient"], path: "Sources/GematikZero"),
        .testTarget(name: "GematikZeroTests", dependencies: ["GematikZero"], path: "Tests/GematikZeroTests"),
    ]
)
