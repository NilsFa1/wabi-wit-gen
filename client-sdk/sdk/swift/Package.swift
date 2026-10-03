// swift-tools-version:5.9
// Wabi Swift SDK = runtime written by wabi-wit-gen (Sources/WabiClient/Generated/Runtime)
//                + generated code     (Sources/WabiClient/Generated -> generated/swift)
// linked against any core that implements the Wabi native ABI (abi/wabi_abi.h).
//
// Desktop/Linux: `swift build -Xlinker -L<dir with libwabi_client>`.
// iOS: replace the system library with the binary target below
//      (built by scripts/build-apple.sh).
import PackageDescription

let package = Package(
    name: "WabiClient",
    platforms: [.macOS(.v12), .iOS(.v15)],
    products: [
        .library(name: "WabiClient", targets: ["WabiClient"]),
    ],
    targets: [
        .systemLibrary(name: "CWabiClient", path: "Sources/CWabiClient"),
        // .binaryTarget(name: "CWabiClient", path: "../../build/apple/WabiClient.xcframework"),
        .target(name: "WabiClient", dependencies: ["CWabiClient"], path: "Sources/WabiClient"),
        .testTarget(name: "WabiClientTests", dependencies: ["WabiClient"], path: "Tests/WabiClientTests"),
        .executableTarget(name: "WabiExample", dependencies: ["WabiClient"], path: "Sources/WabiExample"),
    ]
)
