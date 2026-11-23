import SwiftUI
import UIKit

@main
struct ScarletApp: App {
    var body: some Scene {
        WindowGroup {
            ScarletRootView()
        }
    }
}

private struct ScarletRootView: View {
    private let session = ScarletSession.shared
    @State private var tab: SessionTab = .desktop

    private enum SessionTab { case desktop, console }

    var body: some View {
        GeometryReader { geometry in
            Group {
                #if SCARLET_EXECUTION_PROBE
                TerminalView(console: session.console, acceptsInput: false)
                #else
                if session.displayEnabled {
                    TabView(selection: $tab) {
                        DesktopView(display: session.display, active: tab == .desktop)
                            .tabItem { Label("Desktop", systemImage: "display") }
                            .tag(SessionTab.desktop)
                        TerminalView(console: session.console, acceptsInput: true)
                            .tabItem { Label("Console", systemImage: "terminal") }
                            .tag(SessionTab.console)
                    }
                } else {
                    TerminalView(console: session.console, acceptsInput: !session.schedulerCheck)
                }
                #endif
            }
            .onAppear { session.start(size: geometry.size) }
        }
    }
}

private func report(_ line: String) {
    FileHandle.standardError.write(Data((line + "\n").utf8))
    DispatchQueue.main.async {
        ScarletSession.shared.console.append(line)
    }
}

private func receiveLog(_ line: UnsafePointer<CChar>?) {
    guard let line else { return }
    report(String(cString: line))
}

@MainActor
private final class ScarletSession {
    static let shared = ScarletSession()

    let console = TerminalConsole()
    private var started = false

    #if !SCARLET_EXECUTION_PROBE
    private let arguments = ProcessInfo.processInfo.arguments
    let display = DesktopDisplay()
    var schedulerCheck: Bool { arguments.contains("--scheduler-check") }
    var displayEnabled: Bool { !schedulerCheck && arguments.contains("--display") }
    #endif

    func start(size: CGSize) {
        guard !started, size.width >= 2, size.height >= 2 else { return }
        started = true

        #if SCARLET_EXECUTION_PROBE
        DispatchQueue.global(qos: .userInitiated).async {
            guard let frameworks = Bundle.main.privateFrameworksURL else {
                report("SCARLET_P0_FAIL: Missing Frameworks directory")
                return
            }
            let fixture = frameworks.appendingPathComponent("ScarletFixture.framework/ScarletFixture")
            let passed = fixture.withUnsafeFileSystemRepresentation { path in
                guard let path else { return false }
                return probe_run(path, receiveLog)
            }
            report(passed ? "SCARLET_P0_PASS" : "SCARLET_P0_FAIL")
        }
        #else
        if schedulerCheck {
            DispatchQueue.global(qos: .userInitiated).async {
                runSchedulerCheck()
            }
        } else {
            // Keep the virtual scanout an exact number of logical pixels at
            // the bundled Desktop's 2x output scale.
            let width = displayEnabled ? UInt32(size.width) & ~1 : 0
            let height = displayEnabled ? UInt32(size.height) & ~1 : 0
            let tablet = UIDevice.current.userInterfaceIdiom == .pad || arguments.contains("--tablet")
            let arguments = arguments
            if displayEnabled {
                display.prepare(width: Int(width), height: Int(height))
            }
            DispatchQueue.global(qos: .userInitiated).async {
                boot(arguments: arguments, width: width, height: height, tablet: tablet)
            }
        }
        #endif
    }
}

#if !SCARLET_EXECUTION_PROBE
private func receiveScreen(_ text: UnsafePointer<CChar>?, _ state: ScarletTerminalState) {
    guard let text else { return }
    let snapshot = TerminalSnapshot(
        rows: String(cString: text).components(separatedBy: "\n"),
        cursor: state.cursorVisible ? .init(row: Int(state.row), column: Int(state.column)) : nil,
        applicationCursor: state.applicationCursor,
        bracketedPaste: state.bracketedPaste
    )
    DispatchQueue.main.async {
        ScarletSession.shared.console.update(snapshot)
    }
}

private func receiveDamage() {
    DispatchQueue.main.async {
        ScarletSession.shared.display.render()
    }
}

private struct DesktopView: View {
    @ObservedObject var display: DesktopDisplay
    let active: Bool

    var body: some View {
        ZStack {
            Color.black
            if let frame = display.frame {
                Image(decorative: frame, scale: 1)
                    .resizable()
                    .scaledToFit()
                    .overlay {
                        DesktopTouchCapture(width: frame.width, height: frame.height)
                            .accessibilityHidden(true)
                    }
            }
        }
        .accessibilityElement()
        .accessibilityIdentifier("scarlet-display")
        .accessibilityLabel("Scarlet display")
        .background {
            DesktopTabInput(active: active)
                .frame(width: 1, height: 1)
                .accessibilityHidden(true)
        }
        .safeAreaInset(edge: .bottom, spacing: 0) {
            HStack {
                Spacer()
                Button("Tab") { scarlet_ios_desktop_tab() }
                    .frame(minHeight: 44)
                    .accessibilityLabel("Send Tab to Desktop")
            }
            .padding(.horizontal)
            .background(.bar)
        }
    }
}

private struct DesktopTabInput: UIViewRepresentable {
    let active: Bool

    func makeUIView(context: Context) -> KeyView { KeyView() }

    func updateUIView(_ view: KeyView, context: Context) { view.active = active }

    static func dismantleUIView(_ view: KeyView, coordinator: ()) { view.active = false }

    final class KeyView: UIView {
        var active = false {
            didSet {
                if active != oldValue { updateFocus() }
            }
        }

        override var canBecomeFirstResponder: Bool { active }

        override func didMoveToWindow() {
            super.didMoveToWindow()
            updateFocus()
        }

        private func updateFocus() {
            // Changing responders can invalidate SwiftUI's focus state.
            // Apply it after the current view update, using the latest selection.
            DispatchQueue.main.async { [weak self] in
                guard let self else { return }
                if self.active && self.window != nil {
                    if !self.isFirstResponder { self.becomeFirstResponder() }
                } else if self.isFirstResponder {
                    self.resignFirstResponder()
                }
            }
        }

        override func pressesBegan(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
            guard active else {
                super.pressesBegan(presses, with: event)
                return
            }
            var unhandled = presses
            for press in presses {
                guard let key = press.key, key.keyCode == .keyboardTab,
                      key.modifierFlags.intersection([.command, .alternate, .control, .shift]).isEmpty else {
                    continue
                }
                scarlet_ios_desktop_tab()
                unhandled.remove(press)
            }
            if !unhandled.isEmpty { super.pressesBegan(unhandled, with: event) }
        }
    }
}

@MainActor
private final class DesktopDisplay: ObservableObject {
    @Published private(set) var frame: CGImage?
    private var width = 0
    private var height = 0
    private let colors = CGColorSpaceCreateDeviceRGB()
    private var pixels = Data()

    func prepare(width: Int, height: Int) {
        self.width = width
        self.height = height
        pixels = Data(count: width * height * 4)
    }

    func render() {
        guard pixels.withUnsafeMutableBytes({
            scarlet_ios_copy_frame($0.baseAddress, $0.count)
        }) else { return }

        // The image provider keeps immutable pixels while the next
        // damage update writes into the persistent buffer.
        guard let provider = CGDataProvider(data: pixels as CFData),
              let image = CGImage(
                width: width, height: height, bitsPerComponent: 8, bitsPerPixel: 32,
                bytesPerRow: width * 4, space: colors,
                bitmapInfo: [.byteOrder32Little, CGBitmapInfo(rawValue: CGImageAlphaInfo.noneSkipFirst.rawValue)],
                provider: provider, decode: nil, shouldInterpolate: false, intent: .defaultIntent
              ) else { return }
        frame = image
    }
}

private func prepareRoot(in bundle: Bundle) throws -> URL {
    let files = FileManager.default
    let directory = try files.url(
        for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: true
    )
    let root = directory.appendingPathComponent("scarlet-root.ext2")
    if files.fileExists(atPath: root.path) { return root }

    let seed = bundle.bundleURL.appendingPathComponent("root.ext2")
    let stage = directory.appendingPathComponent(UUID().uuidString)
    defer { try? files.removeItem(at: stage) }
    try files.copyItem(at: seed, to: stage)
    try files.moveItem(at: stage, to: root)
    return root
}

private func boot(arguments: [String], width: UInt32, height: UInt32, tablet: Bool) {
    let bundle = Bundle.main
    do {
        let root = try prepareRoot(in: bundle)
        bundle.bundleURL.withUnsafeFileSystemRepresentation { bundlePath in
            root.withUnsafeFileSystemRepresentation { rootPath in
                guard let bundlePath, let rootPath else {
                    report("SCARLET_BOOT_FAIL: Cannot represent the bundle or root path")
                    return
                }
                let display = ScarletDisplayConfiguration(
                    width: width, height: height, tablet: tablet, damage: receiveDamage
                )
                scarlet_ios_boot(
                    bundlePath, rootPath, arguments.contains("--boot-check"),
                    arguments.contains("--login"), arguments.contains("--shell"),
                    display, receiveLog, receiveScreen
                )
            }
        }
    } catch {
        report("SCARLET_BOOT_FAIL: \(error)")
    }
}

private func runSchedulerCheck() {
    let bundle = Bundle.main
    guard let framework = bundle.privateFrameworksURL,
          let elf = bundle.url(forResource: "scheduler", withExtension: "elf") else {
        report("SCARLET_AOT_FAIL: Missing scheduler fixture")
        return
    }
    let image = framework.appendingPathComponent("ScarletGuest.framework/ScarletGuest")
    image.withUnsafeFileSystemRepresentation { imagePath in
        elf.withUnsafeFileSystemRepresentation { elfPath in
            guard let imagePath, let elfPath else {
                report("SCARLET_AOT_FAIL: Cannot represent the fixture paths")
                return
            }
            scarlet_ios_run(imagePath, elfPath, receiveLog)
        }
    }
}
#endif
