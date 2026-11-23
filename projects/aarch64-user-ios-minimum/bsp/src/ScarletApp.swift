import QuartzCore
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
    @State private var fullScreen = false
    @State private var resizing = false
    @Environment(\.scenePhase) private var scenePhase

    private enum SessionTab { case desktop, console }

    var body: some View {
        #if SCARLET_EXECUTION_PROBE
        TerminalView(console: session.console, acceptsInput: false)
            .onAppear { session.start() }
        #else
        if session.displayEnabled {
            if #available(iOS 26.0, *) {
                displaySession.onInteractiveResizeChange { resizing = $0 }
            } else {
                displaySession
            }
        } else {
            TerminalView(console: session.console, acceptsInput: !session.schedulerCheck)
                .onAppear { session.start() }
        }
        #endif
    }

    #if !SCARLET_EXECUTION_PROBE
    private var displaySession: some View {
        Group {
            if fullScreen {
                desktop
            } else {
                TabView(selection: $tab) {
                    desktop
                        .tabItem { Label("Desktop", systemImage: "display") }
                        .tag(SessionTab.desktop)
                    TerminalView(console: session.console, acceptsInput: true)
                        .tabItem { Label("Console", systemImage: "terminal") }
                        .tag(SessionTab.console)
                }
            }
        }
        .onReceive(NotificationCenter.default.publisher(for: UIApplication.willResignActiveNotification)) { _ in
            scarlet_ios_metal_set_active(false)
        }
        .onReceive(NotificationCenter.default.publisher(for: UIApplication.didBecomeActiveNotification)) { _ in
            scarlet_ios_metal_set_active(true)
        }
    }

    private var desktop: some View {
        DesktopView(display: session.display, active: tab == .desktop && scenePhase == .active,
                    resizing: resizing, resize: session.resize)
            .overlay(alignment: .topTrailing) {
                Button {
                    fullScreen.toggle()
                } label: {
                    Image(systemName: fullScreen ? "arrow.down.right.and.arrow.up.left"
                                                 : "arrow.up.left.and.arrow.down.right")
                        .font(.system(size: 17, weight: .semibold))
                        .foregroundStyle(.white)
                        .frame(width: 44, height: 44)
                        .background(.black.opacity(0.65), in: Circle())
                }
                .buttonStyle(.plain)
                .padding(8)
                .accessibilityLabel(fullScreen ? "Exit Full Screen" : "Enter Full Screen")
            }
    }
    #endif
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
    var displayEnabled: Bool {
        guard !schedulerCheck, !arguments.contains("--console") else { return false }
        return arguments.contains("--display")
            || Bundle.main.object(forInfoDictionaryKey: "ScarletDefaultDisplay") as? Bool == true
    }

    func resize(_ viewport: DesktopViewport) {
        if !started { display.openMetal(viewport) }
        if !display.metalLayer.isHidden {
            scarlet_ios_metal_viewport(UInt32(viewport.pixels.width), UInt32(viewport.pixels.height))
        }
        scarlet_ios_set_display_size(viewport.width, viewport.height)
        start(width: viewport.width, height: viewport.height)
    }
    #endif

    func start(width: UInt32 = 0, height: UInt32 = 0) {
        guard !started else { return }
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
            let tablet = UIDevice.current.userInterfaceIdiom == .pad || arguments.contains("--tablet")
            let arguments = arguments
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
    let resizing: Bool
    let resize: (DesktopViewport) -> Void

    var body: some View {
        ZStack {
            Color.black
            DesktopTouchCapture(
                active: active, resizing: resizing, metalLayer: display.metalLayer,
                image: display.frame, resize: resize
            )
                .accessibilityHidden(true)
        }
        .clipped()
        .background {
            Color.black.ignoresSafeArea(.container).allowsHitTesting(false)
        }
        .accessibilityElement()
        .accessibilityIdentifier("scarlet-display")
        .accessibilityLabel("Scarlet display")
        .background {
            DesktopTabInput(active: active)
                .frame(width: 1, height: 1)
                .accessibilityHidden(true)
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
    let metalLayer = CAMetalLayer()
    private var generation: UInt64 = 0
    private let colors = CGColorSpaceCreateDeviceRGB()
    private var pixels = Data()

    init() {
        metalLayer.isOpaque = true
        metalLayer.isHidden = true
    }

    func openMetal(_ viewport: DesktopViewport) {
        let layer = Unmanaged.passUnretained(metalLayer).toOpaque()
        metalLayer.isHidden = !scarlet_ios_open_metal(layer, viewport.width, viewport.height)
    }

    func render() {
        let mode = scarlet_ios_frame_info()
        guard mode.generation != 0 else { return }
        let width = Int(mode.width)
        let height = Int(mode.height)
        if mode.generation != generation {
            generation = mode.generation
            pixels = Data(count: width * height * 4)
        }
        guard pixels.withUnsafeMutableBytes({
            scarlet_ios_copy_frame($0.baseAddress, $0.count, mode.generation)
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
