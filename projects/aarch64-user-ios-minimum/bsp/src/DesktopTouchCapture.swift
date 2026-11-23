#if !SCARLET_EXECUTION_PROBE
import SwiftUI
import UIKit

struct DesktopTouchCapture: UIViewRepresentable {
    let width: Int
    let height: Int

    func makeUIView(context: Context) -> TouchView {
        let view = TouchView()
        view.isMultipleTouchEnabled = true
        view.width = width
        view.height = height
        return view
    }

    func updateUIView(_ view: TouchView, context: Context) {
        view.width = width
        view.height = height
    }

    static func dismantleUIView(_ view: TouchView, coordinator: ()) {
        view.releaseContacts()
    }

    final class TouchView: UIView {
        private static let inactive = ScarletTouchContact(id: -1, x: 0, y: 0)
        private var owners = [ObjectIdentifier?](repeating: nil, count: Int(SCARLET_IOS_TOUCH_SLOTS))
        private var contacts = [ScarletTouchContact](repeating: inactive, count: Int(SCARLET_IOS_TOUCH_SLOTS))
        private var nextID: Int32 = 0
        var width = 1
        var height = 1

        override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent?) {
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
            if window == nil { releaseContacts() }
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
            let location = touch.location(in: self)
            contacts[slot].x = Self.pixel(location.x, in: bounds.width, extent: width)
            contacts[slot].y = Self.pixel(location.y, in: bounds.height, extent: height)
        }

        private static func pixel(_ point: CGFloat, in length: CGFloat, extent: Int) -> Int32 {
            let position = length > 0 ? point * CGFloat(extent) / length : 0
            return Int32(min(max(0, position), CGFloat(extent - 1)))
        }

        private func publish(cancelled: Bool = false) {
            contacts.withUnsafeBufferPointer {
                scarlet_ios_touch($0.baseAddress, $0.count, cancelled)
            }
        }
    }
}
#endif
