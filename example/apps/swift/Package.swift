// swift-tools-version:5.9
// The example app in Swift, on the generated SDK package (../../sdk/swift/DemoClient):
//   swift run -Xlinker -L<dir with libdemo_client.dylib> -Xlinker -rpath -Xlinker <dir> DemoExample
import PackageDescription

let package = Package(
    name: "DemoExample",
    platforms: [.macOS(.v12)],
    dependencies: [.package(path: "../../sdk/swift/DemoClient")],
    targets: [
        .executableTarget(name: "DemoExample", dependencies: [.product(name: "DemoClient", package: "DemoClient")], path: "Sources/DemoExample"),
    ]
)
