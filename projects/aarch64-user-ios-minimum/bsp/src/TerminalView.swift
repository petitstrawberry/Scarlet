import SwiftUI
import UIKit

struct TerminalSnapshot: Equatable {
    struct Cursor: Equatable {
        let row: Int
        let column: Int
    }

    var rows: [String] = []
    var cursor: Cursor?
    var applicationCursor = false
    var bracketedPaste = false
}

@MainActor
final class TerminalConsole: ObservableObject {
    @Published private(set) var snapshot = TerminalSnapshot()
    @Published private(set) var inputError = false
    @Published private(set) var inputStalled = false
    @Published private(set) var hostMessage: String?
    private var hasScreen = false
    private var pendingInput = Data()
    private var draining = false

    func append(_ line: String) {
        if !hasScreen {
            snapshot.rows.append(line)
        } else {
            hostMessage = line
        }
    }

    func update(_ snapshot: TerminalSnapshot) {
        hasScreen = true
        if self.snapshot != snapshot {
            self.snapshot = snapshot
        }
    }

    func send(_ text: String) {
        #if !SCARLET_EXECUTION_PROBE
        let bytes = Data(text.utf8)
        guard !bytes.isEmpty else { return }
        guard bytes.count <= 64 * 1024 - pendingInput.count else {
            inputError = true
            return
        }
        pendingInput.append(bytes)
        guard !draining else { return }
        draining = true
        inputError = false
        Task { await drainInput() }
        #endif
    }

    #if !SCARLET_EXECUTION_PROBE
    private func drainInput() async {
        defer { draining = false }
        let clock = ContinuousClock()
        var deadline = clock.now + .seconds(2)
        while !pendingInput.isEmpty {
            let count = min(pendingInput.count, 4096)
            let sent = pendingInput.withUnsafeBytes { scarlet_ios_console_send($0.baseAddress, count) }
            if sent {
                pendingInput.removeFirst(count)
                if inputStalled { inputStalled = false }
                deadline = clock.now + .seconds(2)
            } else if clock.now < deadline {
                try? await Task.sleep(for: .milliseconds(8))
            } else {
                if !inputStalled { inputStalled = true }
                // A partially accepted paste still needs its pending suffix.
                try? await Task.sleep(for: .milliseconds(100))
            }
        }
    }
    #endif

    func paste(_ text: String) {
        let text = text.replacingOccurrences(of: "\r\n", with: "\n").replacingOccurrences(of: "\r", with: "\n")
        send(snapshot.bracketedPaste ? "\u{1b}[200~" + text + "\u{1b}[201~" : text)
    }

    func sendArrow(_ suffix: String) {
        send((snapshot.applicationCursor ? "\u{1b}O" : "\u{1b}[") + suffix)
    }
}

struct TerminalView: View {
    private struct Viewport: Equatable {
        let size: CGSize
        let cursor: TerminalSnapshot.Cursor?
        let rows: Int
    }

    @ObservedObject var console: TerminalConsole
    let acceptsInput: Bool
    @State private var input: TerminalKeyInput?
    @State private var control = false

    private let font = UIFont.monospacedSystemFont(ofSize: 14, weight: .regular)
    private var cellWidth: CGFloat { ("M" as NSString).size(withAttributes: [.font: font]).width }
    private var cellHeight: CGFloat { ceil(font.lineHeight) }
    private let background = Color(red: 0.05, green: 0.06, blue: 0.08)

    var body: some View {
        VStack(spacing: 0) {
            GeometryReader { geometry in
                ScrollViewReader { scroll in
                    let snapshot = console.snapshot
                    ScrollView([.horizontal, .vertical]) {
                        VStack(alignment: .leading, spacing: 0) {
                            ForEach(snapshot.rows.indices, id: \.self) { index in
                                Text(verbatim: snapshot.rows[index])
                                    .font(.system(size: font.pointSize, design: .monospaced))
                                    .fixedSize(horizontal: true, vertical: false)
                                    .frame(height: cellHeight, alignment: .leading)
                                    .overlay(alignment: .topLeading) {
                                        if let cursor = snapshot.cursor, cursor.row == index {
                                            cursorCell(column: cursor.column)
                                        }
                                    }
                                    .id(index)
                            }
                        }
                        .frame(minWidth: acceptsInput ? 80 * cellWidth : 0, alignment: .leading)
                        .padding(14)
                        .frame(minWidth: geometry.size.width, minHeight: geometry.size.height, alignment: .topLeading)
                        .contentShape(Rectangle())
                        .onTapGesture {
                            if acceptsInput { input?.becomeFirstResponder() }
                        }
                        .contextMenu {
                            if acceptsInput {
                                PasteButton(payloadType: String.self) { items in
                                    if let text = items.first { input?.sendPaste(text) }
                                }
                            }
                        }
                        .accessibilityElement(children: .ignore)
                        .accessibilityLabel(snapshot.rows.joined(separator: "\n"))
                        .accessibilityIdentifier("scarlet-console-output")
                    }
                    .task(id: Viewport(size: geometry.size, cursor: snapshot.cursor, rows: snapshot.rows.count)) {
                        if snapshot.cursor != nil {
                            scroll.scrollTo("cursor")
                        } else if let row = snapshot.rows.indices.last {
                            scroll.scrollTo(row)
                        }
                    }
                }
            }
            if acceptsInput {
                if let message = consoleMessage {
                    Text(message)
                        .font(.caption)
                        .foregroundStyle(.orange)
                        .padding(.bottom, 6)
                        .accessibilityIdentifier("scarlet-console-input-error")
                }
                keyBar
                    .background(alignment: .leading) {
                        TerminalInput(console: console, input: $input, control: $control)
                            .frame(width: 1, height: 1)
                    }
            }
        }
        .foregroundStyle(Color(white: 0.9))
        .background(background.ignoresSafeArea())
        .preferredColorScheme(.dark)
        .onDisappear {
            input?.resignFirstResponder()
            control = false
        }
    }

    private var consoleMessage: String? {
        if let message = console.hostMessage { return message }
        if console.inputStalled { return "Waiting for the guest to accept input" }
        if console.inputError { return "Terminal input buffer is full" }
        return nil
    }

    private func cursorCell(column: Int) -> some View {
        HStack(spacing: 0) {
            Color.clear.frame(width: CGFloat(column) * cellWidth, height: 1)
            Color.clear
                .frame(width: cellWidth, height: cellHeight)
                .overlay(alignment: .bottom) {
                    Rectangle().fill(.white.opacity(0.8)).frame(height: 2)
                }
                .id("cursor")
        }
        .accessibilityHidden(true)
    }

    private var keyBar: some View {
        HStack(spacing: 2) {
            Button("Esc") { console.send("\u{1b}") }
            Button("Tab") {
                if let input { input.sendTab() }
                else { console.send("\t") }
            }
            Button("Ctrl") {
                control.toggle()
                input?.becomeFirstResponder()
            }
            .foregroundStyle(control ? .cyan : .secondary)
            .accessibilityAddTraits(control ? .isSelected : [])
            Spacer(minLength: 6)
            key("arrow.left", label: "Left") { console.sendArrow("D") }
            key("arrow.up", label: "Up") { console.sendArrow("A") }
            key("arrow.down", label: "Down") { console.sendArrow("B") }
            key("arrow.right", label: "Right") { console.sendArrow("C") }
            key("keyboard", label: "Toggle keyboard") {
                if let input {
                    if input.isFirstResponder { input.resignFirstResponder() }
                    else { input.becomeFirstResponder() }
                }
            }
        }
        .buttonStyle(TerminalKeyStyle())
        .padding(.horizontal, 8)
        .frame(minHeight: 46)
        .background(.white.opacity(0.05))
        .overlay(alignment: .top) { Divider() }
    }

    private func key(_ image: String, label: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: image)
        }
        .accessibilityLabel(label)
    }
}

private struct TerminalKeyStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 13, weight: .medium, design: .monospaced))
            .frame(minWidth: 36, minHeight: 44)
            .background(.white.opacity(configuration.isPressed ? 0.14 : 0), in: RoundedRectangle(cornerRadius: 6))
            .contentShape(Rectangle())
    }
}

// A single UIKit responder supplies the system keyboard and marked-text input.
// Only the guest's echo and cursor appear in the SwiftUI terminal.
private struct TerminalInput: UIViewRepresentable {
    let console: TerminalConsole
    @Binding var input: TerminalKeyInput?
    @Binding var control: Bool

    func makeUIView(context: Context) -> TerminalKeyInput {
        let field = TerminalKeyInput()
        field.textColor = .clear
        field.tintColor = .clear
        field.smartQuotesType = .no
        field.smartDashesType = .no
        field.autocorrectionType = .no
        field.autocapitalizationType = .none
        field.spellCheckingType = .no
        field.accessibilityLabel = "Terminal keyboard input"
        field.accessibilityIdentifier = "scarlet-console-input"
        field.console = console
        field.control = $control
        DispatchQueue.main.async {
            input = field
        }
        return field
    }

    func updateUIView(_ field: TerminalKeyInput, context: Context) {
        field.console = console
        field.control = $control
    }

    static func dismantleUIView(_ field: TerminalKeyInput, coordinator: ()) {
        field.resignFirstResponder()
    }
}

private final class TerminalKeyInput: UITextField, UITextFieldDelegate {
    weak var console: TerminalConsole?
    var control: Binding<Bool>?

    override init(frame: CGRect) {
        super.init(frame: frame)
        delegate = self
        addTarget(self, action: #selector(editingChanged), for: .editingChanged)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("Implement init(frame:) instead")
    }

    @objc private func editingChanged() {
        guard markedTextRange == nil, let text, !text.isEmpty else { return }
        self.text = nil
        if let control, control.wrappedValue {
            control.wrappedValue = false
            if let key = Self.controlByte(text) {
                console?.send(key)
                return
            }
        }
        console?.send(text)
    }

    override func unmarkText() {
        super.unmarkText()
        editingChanged()
    }

    override func deleteBackward() {
        if markedTextRange == nil && (text?.isEmpty ?? true) {
            console?.send("\u{7f}")
        } else {
            super.deleteBackward()
        }
    }

    func textFieldShouldReturn(_ textField: UITextField) -> Bool {
        if markedTextRange == nil {
            editingChanged()
            console?.send("\n")
        }
        return false
    }

    override func paste(_ sender: Any?) {
        if let text = UIPasteboard.general.string, !text.isEmpty {
            sendPaste(text)
        }
    }

    func sendPaste(_ text: String) {
        commitInput()
        console?.paste(text)
    }

    func sendTab() {
        commitInput()
        console?.send("\t")
    }

    private func commitInput() {
        if markedTextRange != nil { unmarkText() }
        editingChanged()
    }

    override func pressesBegan(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        guard markedTextRange == nil else {
            super.pressesBegan(presses, with: event)
            return
        }
        var unhandled = presses
        for press in presses {
            guard let key = press.key, key.keyCode == .keyboardTab,
                  key.modifierFlags.intersection([.command, .alternate, .control, .shift]).isEmpty else {
                continue
            }
            sendTab()
            unhandled.remove(press)
        }
        if !unhandled.isEmpty {
            super.pressesBegan(unhandled, with: event)
        }
    }

    override var keyCommands: [UIKeyCommand]? {
        guard markedTextRange == nil else { return nil }
        let keys = [
            UIKeyCommand.inputEscape, UIKeyCommand.inputUpArrow, UIKeyCommand.inputDownArrow,
            UIKeyCommand.inputLeftArrow, UIKeyCommand.inputRightArrow, UIKeyCommand.inputHome,
            UIKeyCommand.inputEnd, UIKeyCommand.inputDelete
        ]
        let commands = keys.map { command($0, modifiers: []) }
        let controlCommands = "abcdefghijklmnopqrstuvwxyz[\\]^_ ".map { command(String($0), modifiers: .control) }
        let paste = UIKeyCommand(input: "v", modifierFlags: .command, action: #selector(paste(_:)))
        paste.wantsPriorityOverSystemBehavior = true
        return commands + controlCommands + [paste]
    }

    override func canPerformAction(_ action: Selector, withSender sender: Any?) -> Bool {
        if action == #selector(handleKey(_:)) {
            return markedTextRange == nil
        }
        return super.canPerformAction(action, withSender: sender)
    }

    private func command(_ key: String, modifiers: UIKeyModifierFlags) -> UIKeyCommand {
        let command = UIKeyCommand(input: key, modifierFlags: modifiers, action: #selector(handleKey(_:)))
        command.wantsPriorityOverSystemBehavior = true
        return command
    }

    private static func controlByte(_ key: String) -> String? {
        guard key.utf8.count == 1, let byte = key.utf8.first,
              byte == 32 || (64...95).contains(byte) || (97...122).contains(byte) else { return nil }
        return String(UnicodeScalar(byte & 0x1f))
    }

    @objc private func handleKey(_ command: UIKeyCommand) {
        guard markedTextRange == nil, let key = command.input else { return }
        if command.modifierFlags.contains(.control) {
            if let byte = Self.controlByte(key) { console?.send(byte) }
            return
        }
        switch key {
        case UIKeyCommand.inputEscape: console?.send("\u{1b}")
        case UIKeyCommand.inputUpArrow: console?.sendArrow("A")
        case UIKeyCommand.inputDownArrow: console?.sendArrow("B")
        case UIKeyCommand.inputRightArrow: console?.sendArrow("C")
        case UIKeyCommand.inputLeftArrow: console?.sendArrow("D")
        case UIKeyCommand.inputHome: console?.sendArrow("H")
        case UIKeyCommand.inputEnd: console?.sendArrow("F")
        case UIKeyCommand.inputDelete: console?.send("\u{7f}")
        default: console?.send(key)
        }
    }
}
