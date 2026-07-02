// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "dicta",
    platforms: [
        .macOS("26.0")
    ],
    products: [
        .executable(name: "dicta", targets: ["dicta"])
    ],
    dependencies: [
        .package(url: "https://github.com/apple/swift-argument-parser.git", from: "1.5.0")
    ],
    targets: [
        .executableTarget(
            name: "dicta",
            dependencies: [
                .product(name: "ArgumentParser", package: "swift-argument-parser")
            ],
            path: "Sources/dicta"
        ),
        .testTarget(
            name: "dictaTests",
            dependencies: ["dicta"],
            path: "Tests/dictaTests"
        )
    ]
)
