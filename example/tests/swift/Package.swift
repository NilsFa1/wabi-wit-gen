// swift-tools-version:5.9
// The Swift tests of the example: the generated SDK package (../../sdk/swift/DemoClient)
// against any core. Run on macOS with the core on the linker path:
//   swift test -Xlinker -L<dir with libdemo_client.dylib> -Xlinker -rpath -Xlinker <dir>
import PackageDescription

let package = Package(
    name: "DemoClientTests",
    platforms: [.macOS(.v12)],
    dependencies: [.package(path: "../../sdk/swift/DemoClient")],
    targets: [
        .testTarget(name: "DemoClientTests", dependencies: [.product(name: "DemoClient", package: "DemoClient")], path: "Tests/DemoClientTests"),
    ]
)
