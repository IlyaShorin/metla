import SwiftUI

/// A checkbox drawn with SwiftUI shapes in the accent colour: it looks
/// like the system's, works with Space and VoiceOver as a toggle, and,
/// unlike the system checkbox and switch, whose marks the window server
/// composes, the capture mode can draw it, so a picture never contradicts
/// its "selected N" header or a setting's state.
struct SymbolCheckbox: ToggleStyle {
    @Environment(\.isEnabled) private var enabled

    func makeBody(configuration: Configuration) -> some View {
        Button {
            configuration.isOn.toggle()
        } label: {
            HStack(spacing: 6) {
                Box(on: configuration.isOn)
                    .frame(width: 14, height: 14)
                    .opacity(enabled ? 1 : 0.4)
                configuration.label
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityRepresentation {
            Toggle(isOn: configuration.$isOn) { configuration.label }
        }
    }

    /// The box and, when on, its check mark.
    struct Box: View {
        let on: Bool


        var body: some View {
            ZStack {
                RoundedRectangle(cornerRadius: 3.5)
                    .fill(on ? Color.accentColor : Color(nsColor: .controlBackgroundColor))
                RoundedRectangle(cornerRadius: 3.5)
                    .strokeBorder(on ? Color.accentColor : Color.secondary.opacity(0.6), lineWidth: 1)
                if on {
                    // The mark as text: text is what every renderer draws,
                    // the capture's included.
                    Text("\u{2713}")
                        .font(.system(size: 11, weight: .heavy))
                        .foregroundStyle(Color.white)
                        .accessibilityHidden(true)
                }
            }
        }
    }
}

extension ToggleStyle where Self == SymbolCheckbox {
    static var symbolCheckbox: SymbolCheckbox { SymbolCheckbox() }
}

/// A number of days with − and + buttons beside it.
struct DaysField: View {
    let title: String
    let value: String
    let change: (Int) -> Void

    var body: some View {
        LabeledContent(title) {
            HStack(spacing: 6) {
                Text(value).monospacedDigit()
                Button("\u{2212}") { change(-1) }
                Button("+") { change(1) }
            }
        }
    }
}
