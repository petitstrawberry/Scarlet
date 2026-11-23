#if !SCARLET_EXECUTION_PROBE
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
    let resize: (DesktopViewport) -> Void

    func makeUIView(context: Context) -> TouchView {
        let view = TouchView()
        view.isMultipleTouchEnabled = true
        view.resize = resize
        return view
    }

    func updateUIView(_ view: TouchView, context: Context) {
        view.resize = resize
    }

    static func dismantleUIView(_ view: TouchView, coordinator: ()) {
        view.releaseContacts()
    }

    final class TouchView: UIView {
        private static let inactive = ScarletTouchContact(id: -1, x: 0, y: 0)
        private var owners = [ObjectIdentifier?](repeating: nil, count: Int(SCARLET_IOS_TOUCH_SLOTS))
        private var contacts = [ScarletTouchContact](repeating: inactive, count: Int(SCARLET_IOS_TOUCH_SLOTS))
        private var nextID: Int32 = 0
        var resize: ((DesktopViewport) -> Void)?
        private var viewport: DesktopViewport?

        override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent?) {
            guard viewport != nil else { return }
            var changed = false
            for touch in touches where touch.type == .direct || touch.type == .pencil {
                guard let slot = owners.firstIndex(of: nil) else { break }
                owners[slot] = ObjectIdentifier(touch)
                contacts[slot].id = nextID
                nextID = nextID == Int32.max ? 0 : nextID + 1
                update(touch, at: slot)
                changed = true
            }
            if changed { publish() }
        }

        override func touchesMoved(_ touches: Set<UITouch>, with event: UIEvent?) {
            var changed = false
            for touch in touches {
                guard let slot = owners.firstIndex(of: ObjectIdentifier(touch)) else { continue }
                update(touch, at: slot)
                changed = true
            }
            if changed { publish() }
        }

        override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent?) {
            remove(touches, cancelled: false)
        }

        override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent?) {
            remove(touches, cancelled: true)
        }

        override func didMoveToWindow() {
            super.didMoveToWindow()
            if window == nil {
                viewport = nil
                releaseContacts()
            } else {
                setNeedsLayout()
            }
        }

        override func layoutSubviews() {
            super.layoutSubviews()
            guard let screen = window?.windowScene?.screen,
                  let size = DesktopViewport(points: bounds.size, nativeScale: screen.nativeScale) else {
                viewport = nil
                releaseContacts()
                return
            }
            guard size != viewport else { return }
            releaseContacts()
            viewport = size
            DispatchQueue.main.async { [weak self] in
                guard let self, self.viewport == size, self.window != nil else { return }
                self.resize?(size)
            }
        }

        func releaseContacts() {
            guard owners.contains(where: { $0 != nil }) else { return }
            for slot in owners.indices { release(slot) }
            publish(cancelled: true)
        }

        private func remove(_ touches: Set<UITouch>, cancelled: Bool) {
            var changed = false
            for touch in touches {
                guard let slot = owners.firstIndex(of: ObjectIdentifier(touch)) else { continue }
                release(slot)
                changed = true
            }
            if changed { publish(cancelled: cancelled) }
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

        private func publish(cancelled: Bool = false) {
            contacts.withUnsafeBufferPointer {
                scarlet_ios_touch($0.baseAddress, $0.count, cancelled)
            }
        }
    }
}
#endif
