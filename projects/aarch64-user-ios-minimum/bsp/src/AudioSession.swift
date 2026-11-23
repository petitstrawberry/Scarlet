#if !SCARLET_EXECUTION_PROBE
import AVFAudio
import Foundation
import UIKit

// Pure policy: explicit START while unavailable is retained, whereas an
// unsolicited lifecycle resume cannot override an interruption's denial.
private struct AudioStartPolicy {
    var resumeAllowed = true
    private(set) var explicitStartPending = false
    var mayResume: Bool { resumeAllowed || explicitStartPending }
    mutating func request(resuming: Bool) {
        if !resuming { explicitStartPending = true }
    }
    mutating func completed(activated: Bool) {
        explicitStartPending = false
        if activated { resumeAllowed = true }
    }
    mutating func reset() {
        resumeAllowed = false
        explicitStartPending = false
    }
}

// All mutable state is confined to control. Lifecycle work calls Rust on a
// separate serial queue: Rust may synchronously call back into control.
private final class HostAudioSession: @unchecked Sendable {
    static let shared = HostAudioSession()
    private let control = DispatchQueue(label: "scarlet.audio.session")
    private let lifecycle = DispatchQueue(label: "scarlet.audio.lifecycle")
    private var observers: [NSObjectProtocol] = []
    private var attached = false
    private var foreground = false
    private var interrupted = false
    private var startPolicy = AudioStartPolicy()
    private var active = false

    private var route = ""
    private enum Event: Sendable {
        case background, foreground, interruption(UInt, UInt), route(UInt), lost, reset
    }
    private init() {}

    @MainActor
    func attach() {
        let foreground = UIApplication.shared.applicationState != .background
        control.sync {
            guard !attached else { return }
            attached = true
            self.foreground = foreground
            observe(UIApplication.didEnterBackgroundNotification) { _ in .background }
            observe(UIApplication.didBecomeActiveNotification) { _ in .foreground }
            observe(AVAudioSession.interruptionNotification) { note in
                .interruption((note.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt) ?? 0,
                              (note.userInfo?[AVAudioSessionInterruptionOptionKey] as? UInt) ?? 0)
            }
            observe(AVAudioSession.routeChangeNotification) { note in
                .route((note.userInfo?[AVAudioSessionRouteChangeReasonKey] as? UInt) ?? 0)
            }
            observe(AVAudioSession.mediaServicesWereLostNotification) { _ in .lost }
            observe(AVAudioSession.mediaServicesWereResetNotification) { _ in .reset }
        }
        scarlet_ios_audio_prepare()
    }

    private func observe(_ name: Notification.Name,
                         _ decode: @escaping @Sendable (Notification) -> Event) {
        observers.append(NotificationCenter.default.addObserver(forName: name, object: nil, queue: nil) { [weak self] note in
            guard let self else { return }
            // Copy scalar data, never Notification/userInfo, across queues.
            let event = decode(note)
            self.control.async { self.handle(event) }
        })
    }

    private func routeIdentity() -> String {
        let session = AVAudioSession.sharedInstance()
        return "\(session.sampleRate):\(session.outputNumberOfChannels):" +
            session.currentRoute.outputs.map { $0.uid }.joined(separator: ",")
    }

    private func handle(_ event: Event) {
        switch event {
        case .background:
            foreground = false
            pause()
        case .foreground:
            foreground = true
            resume()
        case let .interruption(kind, options):
            if kind == AVAudioSession.InterruptionType.began.rawValue {
                interrupted = true
                active = false
                pause()
            } else if kind == AVAudioSession.InterruptionType.ended.rawValue {
                interrupted = false
                startPolicy.resumeAllowed = AVAudioSession.InterruptionOptions(rawValue: options).contains(.shouldResume)
                resume()
            }
        case let .route(raw):
            let reason = AVAudioSession.RouteChangeReason(rawValue: raw)
            guard reason != .categoryChange else { return }
            let current = routeIdentity()
            // Ignore notifications caused by our own activation/configuration.
            guard current != route || reason == .oldDeviceUnavailable else { return }
            route = current
            if reason == .oldDeviceUnavailable { startPolicy.reset() }
            pause()
        case .lost:
            interrupted = true
            active = false
            pause()
        case .reset:
            interrupted = false
            startPolicy.reset()
            active = false
            route = ""
            pause()
        }
    }

    private func pause() {
        lifecycle.async {
            scarlet_ios_audio_pause()
            // Reconcile current policy after parking, including a newer START.
            scarlet_ios_audio_resume()
        }
    }

    private func resume() {
        guard foreground, !interrupted, startPolicy.mayResume else { return }
        lifecycle.async { scarlet_ios_audio_resume() }
    }

    func activate(resuming: Bool) -> Int32 {
        control.sync {
            startPolicy.request(resuming: resuming)
            guard attached, foreground, !interrupted, startPolicy.mayResume else { return 1 }
            if active {
                startPolicy.completed(activated: true)
                return 0
            }
            do {
                let session = AVAudioSession.sharedInstance()
                try session.setCategory(.playback, mode: .default, options: [.mixWithOthers])
                try session.setPreferredSampleRate(48_000)
                try session.setActive(true)
                active = true
                route = routeIdentity()
                startPolicy.completed(activated: true)
                return 0
            } catch {
                NSLog("Scarlet audio activation failed: %@", String(describing: error))
                startPolicy.completed(activated: false)
                return -1
            }
        }
    }

    func deactivate() {
        control.sync {
            guard active else { return }
            do {
                try AVAudioSession.sharedInstance().setActive(false, options: [.notifyOthersOnDeactivation])
                active = false
            } catch {
                NSLog("Scarlet audio deactivation failed: %@", String(describing: error))
            }
        }
    }
}

@MainActor
func prepareHostAudio() { HostAudioSession.shared.attach() }

@_cdecl("scarlet_ios_audio_session_activate")
func activateHostAudio(_ resuming: Bool) -> Int32 { HostAudioSession.shared.activate(resuming: resuming) }

@_cdecl("scarlet_ios_audio_session_deactivate")
func deactivateHostAudio() { HostAudioSession.shared.deactivate() }
#endif
