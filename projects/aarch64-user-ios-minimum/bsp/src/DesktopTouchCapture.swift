#if !SCARLET_EXECUTION_PROBE
import QuartzCore
import SwiftUI
import UIKit

struct DesktopViewport: Equatable {
    let pixels: CGSize
    let width: UInt32
    let height: UInt32

    init?(points: CGSize, nativeScale: CGFloat) {
        let width = (points.width * nativeScale).rounded()
        let height = (points.height * nativeScale).rounded()
        guard (2...4096).contains(width), (2...4096).contains(height) else { return nil }
        pixels = CGSize(width: width, height: height)
        // Give the guest a complete two-pixel logical cell on odd edges.
        self.width = (UInt32(width) + 1) & ~1
        self.height = (UInt32(height) + 1) & ~1
    }

    func cropped(_ image: CGImage) -> CGImage {
        guard image.width == Int(width), image.height == Int(height),
              pixels.width != CGFloat(width) || pixels.height != CGFloat(height) else {
            return image
        }
        return image.cropping(to: CGRect(origin: .zero, size: pixels)) ?? image
    }
}

struct DesktopTouchCapture: UIViewRepresentable {
    let active: Bool
    let resizing: Bool
    let metalLayer: CAMetalLayer
    let image: CGImage?
    let resize: (DesktopViewport) -> Void

    func makeUIView(context: Context) -> ClipView {
        let view = ClipView(frame: .zero)
        view.touch.metalLayer = metalLayer
        return view
    }

    func updateUIView(_ view: ClipView, context: Context) {
        view.touch.resize = resize
        view.touch.resizing = resizing
        view.touch.active = active
        view.touch.setImage(image)
    }

    static func dismantleUIView(_ view: ClipView, coordinator: ()) {
        view.touch.active = false
    }

    final class ClipView: UIView {
        let touch = TouchView()

        override init(frame: CGRect) {
            super.init(frame: frame)
            touch.isMultipleTouchEnabled = true
            touch.translatesAutoresizingMaskIntoConstraints = false
            addSubview(touch)
            let guide: UILayoutGuide
            if #available(iOS 26.0, *) {
                guide = layoutGuide(for: .safeArea(cornerAdaptation: .vertical))
            } else {
                guide = safeAreaLayoutGuide
            }
            NSLayoutConstraint.activate([
                touch.topAnchor.constraint(equalTo: guide.topAnchor),
                touch.leadingAnchor.constraint(equalTo: guide.leadingAnchor),
                touch.bottomAnchor.constraint(equalTo: guide.bottomAnchor),
                touch.trailingAnchor.constraint(equalTo: guide.trailingAnchor),
            ])
        }

        required init?(coder: NSCoder) { fatalError("Use init(frame:)") }

        override func point(inside point: CGPoint, with event: UIEvent?) -> Bool {
            touch.frame.contains(point) && super.point(inside: point, with: event)
        }
    }

    final class TouchView: UIView {
        private static let inactive = ScarletTouchContact(id: -1, x: 0, y: 0)
        private var owners = [ObjectIdentifier?](repeating: nil, count: Int(SCARLET_IOS_TOUCH_SLOTS))
        private var contacts = [ScarletTouchContact](repeating: inactive, count: Int(SCARLET_IOS_TOUCH_SLOTS))
        private var nextID: Int32 = 0
        var resize: ((DesktopViewport) -> Void)?
        var metalLayer: CAMetalLayer?
        var resizing = false {
            didSet {
                guard oldValue != resizing else { return }
                if resizing { releaseContacts() } else { publishViewport() }
            }
        }
        var active = false {
            didSet {
                guard oldValue != active else { return }
                if active { setNeedsLayout() } else { detach() }
            }
        }
        private var viewport: DesktopViewport?
        private var publishedViewport: DesktopViewport?
        private var viewportScheduled = false
        private var image: CGImage?
        private let imageLayer = CALayer()

        override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent?) {
            guard active, !resizing, let viewport, viewport == publishedViewport else { return }
            var timestamp: TimeInterval?
            for touch in touches where touch.type == .direct || touch.type == .pencil {
                guard let slot = owners.firstIndex(of: nil) else { break }
                owners[slot] = ObjectIdentifier(touch)
                contacts[slot].id = nextID
                nextID = nextID == Int32.max ? 0 : nextID + 1
                update(touch, at: slot)
                timestamp = max(timestamp ?? touch.timestamp, touch.timestamp)
            }
            if let timestamp { publish(at: timestamp) }
        }

        override func touchesMoved(_ touches: Set<UITouch>, with event: UIEvent?) {
            publishMotion(touches, with: event)
        }

        override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent?) {
            // The lift can include motion since the last move callback.
            publishMotion(touches, with: event)
            remove(touches, cancelled: false)
        }

        override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent?) {
            remove(touches, cancelled: true)
        }

        override func didMoveToWindow() {
            super.didMoveToWindow()
            if window == nil {
                detach()
            } else {
                setNeedsLayout()
            }
        }

        override func layoutSubviews() {
            super.layoutSubviews()
            guard active, let screen = window?.windowScene?.screen,
                  let size = DesktopViewport(points: bounds.size, nativeScale: screen.nativeScale) else {
                detach()
                return
            }
            if let metalLayer {
                CATransaction.begin()
                CATransaction.setDisableActions(true)
                if metalLayer.superlayer !== layer { layer.addSublayer(metalLayer) }
                metalLayer.contentsScale = screen.nativeScale
                metalLayer.frame = bounds
                CATransaction.commit()
            }
            if size != viewport {
                releaseContacts()
                viewport = size
                updateImageLayer()
            }
            publishViewport()
        }

        private func publishViewport() {
            guard active, !resizing, !viewportScheduled, viewport != publishedViewport else { return }
            viewportScheduled = true
            DispatchQueue.main.async { [weak self] in
                guard let self else { return }
                self.viewportScheduled = false
                guard self.active, !self.resizing, self.window != nil,
                      let size = self.viewport, size != self.publishedViewport else { return }
                self.publishedViewport = size
                self.resize?(size)
            }
        }

        func setImage(_ image: CGImage?) {
            guard self.image !== image else { return }
            self.image = image
            updateImageLayer()
        }

        private func updateImageLayer() {
            guard image != nil || imageLayer.superlayer != nil else { return }
            CATransaction.begin()
            CATransaction.setDisableActions(true)
            if imageLayer.superlayer == nil { layer.insertSublayer(imageLayer, at: 0) }
            imageLayer.frame = bounds
            imageLayer.magnificationFilter = .nearest
            imageLayer.minificationFilter = .nearest
            imageLayer.contents = image.map { viewport?.cropped($0) ?? $0 }
            CATransaction.commit()
        }

        private func detach() {
            viewport = nil
            publishedViewport = nil
            releaseContacts()
            if let metalLayer, metalLayer.superlayer === layer {
                metalLayer.removeFromSuperlayer()
                scarlet_ios_metal_viewport(0, 0)
            }
        }

        private func releaseContacts() {
            guard owners.contains(where: { $0 != nil }) else { return }
            for slot in owners.indices { release(slot) }
            publish(cancelled: true)
        }

        private func publishMotion(_ touches: Set<UITouch>, with event: UIEvent?) {
            // One finger drives the tablet navigation gesture. A multitouch
            // callback stays one atomic frame so history cannot multiply its
            // event-queue footprint by the number of contacts.
            let singleContact = owners.lazy.compactMap { $0 }.prefix(2).count == 1
            var timestamp: TimeInterval?
            for touch in touches {
                guard let slot = owners.firstIndex(of: ObjectIdentifier(touch)) else { continue }
                if singleContact, let samples = event?.coalescedTouches(for: touch), !samples.isEmpty {
                    for sample in samples {
                        update(sample, at: slot)
                        publish(at: sample.timestamp)
                    }
                    return
                }
                update(touch, at: slot)
                timestamp = max(timestamp ?? touch.timestamp, touch.timestamp)
            }
            if let timestamp { publish(at: timestamp) }
        }

        private func remove(_ touches: Set<UITouch>, cancelled: Bool) {
            var timestamp: TimeInterval?
            for touch in touches {
                guard let slot = owners.firstIndex(of: ObjectIdentifier(touch)) else { continue }
                release(slot)
                timestamp = max(timestamp ?? touch.timestamp, touch.timestamp)
            }
            if let timestamp { publish(at: timestamp, cancelled: cancelled) }
        }

        private func release(_ slot: Int) {
            owners[slot] = nil
            contacts[slot] = Self.inactive
        }

        private func update(_ touch: UITouch, at slot: Int) {
            guard let viewport else { return }
            let location = touch.location(in: self)
            contacts[slot].x = Self.coordinate(
                location.x, in: bounds.width, visiblePixels: viewport.pixels.width, scanout: viewport.width
            )
            contacts[slot].y = Self.coordinate(
                location.y, in: bounds.height, visiblePixels: viewport.pixels.height, scanout: viewport.height
            )
        }

        private static func coordinate(
            _ point: CGFloat, in length: CGFloat, visiblePixels: CGFloat, scanout: UInt32
        ) -> Int32 {
            let position = length > 0 ? min(max(0, point / length), 1) : 0
            let normalized = position * visiblePixels / CGFloat(scanout)
            return Int32((normalized * CGFloat(SCARLET_IOS_TOUCH_MAX)).rounded())
        }

        private func publish(at timestamp: TimeInterval? = nil, cancelled: Bool = false) {
            let now = ProcessInfo.processInfo.systemUptime
            let ageNS = UInt64(max(0, now - (timestamp ?? now)) * 1_000_000_000)
            contacts.withUnsafeBufferPointer {
                scarlet_ios_touch($0.baseAddress, $0.count, cancelled, ageNS)
            }
        }
    }
}
#endif
