// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "MaestroGui",
    platforms: [.macOS(.v13)],
    targets: [
        .executableTarget(
            name: "MaestroGui",
            path: "Sources/MaestroGui"
        )
    ]
)
