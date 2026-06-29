// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "vo",
    platforms: [
        .macOS("26.0")
    ],
    products: [
        .executable(name: "vo", targets: ["vo"])
    ],
    dependencies: [
        .package(url: "https://github.com/apple/swift-argument-parser.git", from: "1.5.0")
    ],
    targets: [
        .executableTarget(
            name: "vo",
            dependencies: [
                .product(name: "ArgumentParser", package: "swift-argument-parser")
            ],
            path: "Sources/vo"
        ),
        .testTarget(
            name: "voTests",
            dependencies: ["vo"],
            path: "Tests/voTests"
        )
    ]
)
