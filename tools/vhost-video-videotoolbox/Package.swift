// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "vhost-video-videotoolbox",
    platforms: [.macOS(.v13)],
    targets: [
        .executableTarget(
            name: "vhost-video-videotoolbox"
        ),
    ]
)
