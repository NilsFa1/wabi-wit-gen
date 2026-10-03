// swift-tools-version:5.9
// gematik:zero PoC Swift SDK = runtime written by wabi-wit-gen (Sources/GematikZero/Generated/Runtime)
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
        .systemLibrary(name: "CGematikZero", path: "Sources/CGematikZero"),
        .target(name: "GematikZero", dependencies: ["CGematikZero"], path: "Sources/GematikZero"),
        .testTarget(name: "GematikZeroTests", dependencies: ["GematikZero"], path: "Tests/GematikZeroTests"),
    ]
)
