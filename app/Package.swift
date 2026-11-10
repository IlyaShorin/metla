// swift-tools-version: 6.2
// The Metla app: MetlaCoreFFI (the Rust core's generated bindings),
// MetlaKit (the engine client, view models, text and formatting; tested),
// MetlaApp (SwiftUI views, AppKit glue, @main).
//
// scripts/build-core.sh generates Sources/MetlaCoreFFI/generated and the
// static library first; scripts/build-app.sh and scripts/quality-swift.sh
// pass the library's directory to the linker (-Xlinker -L<dir>).
import PackageDescription

/// Swift 6 language mode (complete strict concurrency) and no warnings.
let strict: [SwiftSetting] = [
    .swiftLanguageMode(.v6),
    .treatAllWarnings(as: .error),
]

let package = Package(
    name: "Metla",
    defaultLocalization: "en",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "Metla", targets: ["MetlaApp"]),
    ],
    targets: [
        .systemLibrary(
            name: "metla_ffiFFI",
            path: "Sources/MetlaCoreFFI/generated/include"
        ),
        .target(
            name: "MetlaCoreFFI",
            dependencies: ["metla_ffiFFI"],
            path: "Sources/MetlaCoreFFI",
            exclude: ["generated/include"],
            swiftSettings: strict
        ),
        .target(
            name: "MetlaKit",
            dependencies: ["MetlaCoreFFI"],
            resources: [.process("Resources")],
            swiftSettings: strict
        ),
        .executableTarget(
            name: "MetlaApp",
            dependencies: ["MetlaKit"],
            swiftSettings: strict
        ),
        .testTarget(
            name: "MetlaKitTests",
            dependencies: ["MetlaKit"],
            swiftSettings: strict
        ),
    ]
)
