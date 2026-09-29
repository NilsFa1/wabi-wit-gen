// swift-tools-version:5.9
// Acme Swift SDK = hand-written runtime (Sources/AcmeClient/Runtime -> runtime/swift)
//                + generated code     (Sources/AcmeClient/Generated -> generated/swift)
// linked against any core that implements the Acme native ABI (abi/acme_abi.h).
//
// Desktop/Linux: `swift build -Xlinker -L<dir with libacme_client>`.
// iOS: replace the system library with the binary target below
//      (built by scripts/build-apple.sh).
import PackageDescription

let package = Package(
    name: "AcmeClient",
    platforms: [.macOS(.v12), .iOS(.v15)],
    products: [
        .library(name: "AcmeClient", targets: ["AcmeClient"]),
    ],
    targets: [
        .systemLibrary(name: "CAcmeClient", path: "Sources/CAcmeClient"),
        // .binaryTarget(name: "CAcmeClient", path: "../../build/apple/AcmeClient.xcframework"),
        .target(name: "AcmeClient", dependencies: ["CAcmeClient"], path: "Sources/AcmeClient"),
        .testTarget(name: "AcmeClientTests", dependencies: ["AcmeClient"], path: "Tests/AcmeClientTests"),
        .executableTarget(name: "AcmeExample", dependencies: ["AcmeClient"], path: "Sources/AcmeExample"),
    ]
)
