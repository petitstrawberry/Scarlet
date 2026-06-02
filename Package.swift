// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "ScarletHostTools",
    platforms: [
        .macOS(.v13),
    ],
    products: [
        .executable(
            name: "vhost-video-videotoolbox",
            targets: ["vhost-video-videotoolbox"]
        ),
    ],
    targets: [
        .executableTarget(
            name: "vhost-video-videotoolbox",
            path: "tools",
            exclude: [
                "guest",
                "linux",
                "scpm-pack",
                "vhost_video_stub.py",
            ],
            sources: ["vhost_video_videotoolbox.swift"],
            linkerSettings: [
                .linkedFramework("CoreMedia"),
                .linkedFramework("CoreVideo"),
                .linkedFramework("Foundation"),
                .linkedFramework("VideoToolbox"),
            ]
        ),
    ]
)
