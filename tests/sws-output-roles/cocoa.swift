// Read only the QEMU window belonging to this fixture. QMP screendump cannot
// read a live Cocoa GL texture. The View menu exposes individual GPU heads.
import ApplicationServices
import Foundation
import ImageIO

struct CaptureError: Error, CustomStringConvertible {
    let description: String
    init(_ message: String) { description = message }
}

func attribute(_ item: AXUIElement, _ name: String) -> CFTypeRef? {
    var value: CFTypeRef?
    AXUIElementCopyAttributeValue(item, name as CFString, &value)
    return value
}

func children(_ item: AXUIElement) -> [AXUIElement] {
    attribute(item, "AXChildren") as? [AXUIElement] ?? []
}

func selectHead(_ pid: Int32, _ head: String) throws {
    guard AXIsProcessTrusted() else { throw CaptureError("Cocoa console selection needs Accessibility access") }
    let app = AXUIElementCreateApplication(pid)
    guard let bar = attribute(app, "AXMenuBar"),
          let view = children(bar as! AXUIElement).first(where: { attribute($0, "AXTitle") as? String == "View" }),
          let menu = children(view).first,
          let gpu = children(menu).first(where: { attribute($0, "AXTitle") as? String == "gpu.\(head)" }),
          AXUIElementPerformAction(gpu, kAXPressAction as CFString) == .success
    else { throw CaptureError("QEMU View menu has no selectable gpu.\(head)") }
}

func capture(_ pid: Int32, width: Int, height: Int, path: String) throws {
    guard CGPreflightScreenCaptureAccess() else { throw CaptureError("Cocoa capture needs Screen Recording access") }
    let windows = CGWindowListCopyWindowInfo(.optionOnScreenOnly, kCGNullWindowID) as? [[String: Any]] ?? []
    guard let window = windows.first(where: {
        $0[kCGWindowOwnerPID as String] as? Int32 == pid && $0[kCGWindowName as String] as? String == "QEMU"
    }), let id = window[kCGWindowNumber as String] as? Int,
        let bounds = window[kCGWindowBounds as String] as? [String: CGFloat],
        bounds["Width"] == CGFloat(width)
    else { throw CaptureError("QEMU window did not reach the expected width \(width)") }

    let png = URL(fileURLWithPath: path + ".png")
    defer { try? FileManager.default.removeItem(at: png) }
    let shot = Process()
    shot.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
    shot.arguments = ["-x", "-o", "-l", String(id), png.path]
    try shot.run()
    shot.waitUntilExit()
    guard shot.terminationStatus == 0,
          let source = CGImageSourceCreateWithURL(png as CFURL, nil),
          let image = CGImageSourceCreateImageAtIndex(source, 0, nil)
    else { throw CaptureError("Failed to capture QEMU window \(id)") }

    // Strip window chrome and sample one pixel per guest pixel on Retina hosts.
    let scale = image.width / width
    guard scale > 0 && image.width == width * scale && image.height >= height * scale,
          let content = image.cropping(to: CGRect(x: 0, y: image.height - height * scale,
                                                  width: image.width, height: height * scale)),
          let context = CGContext(data: nil, width: width, height: height, bitsPerComponent: 8,
                                  bytesPerRow: width * 4, space: image.colorSpace ?? CGColorSpaceCreateDeviceRGB(),
                                  bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue),
          let rgba = context.data?.assumingMemoryBound(to: UInt8.self)
    else { throw CaptureError("Unexpected QEMU scanout image or pixel format") }
    context.interpolationQuality = .none
    context.draw(content, in: CGRect(x: 0, y: 0, width: width, height: height))
    var ppm = Data("P6\n\(width) \(height)\n255\n".utf8)
    ppm.reserveCapacity(ppm.count + width * height * 3)
    for offset in stride(from: 0, to: width * height * 4, by: 4) {
        ppm.append(rgba + offset, count: 3)
    }
    try ppm.write(to: URL(fileURLWithPath: path))
}

do {
    let args = CommandLine.arguments
    if args[1] == "select" {
        try selectHead(Int32(args[2])!, args[3])
    } else {
        try capture(Int32(args[2])!, width: Int(args[3])!, height: Int(args[4])!, path: args[5])
    }
} catch {
    FileHandle.standardError.write(Data("\(error)\n".utf8))
    exit(1)
}
