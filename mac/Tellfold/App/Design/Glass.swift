import SwiftUI

// Liquid Glass is macOS 26 and later; the app runs from macOS 15. These wrappers keep the glass on 26 and fall back
// to a translucent material with a hairline border on earlier systems, so call sites never check the OS themselves.

extension View {
    /// A floating surface over the canvases: Liquid Glass on macOS 26 and later, regular material before that.
    /// `interactive` lets the glass react to the pointer (zoom buttons); the fallback has no hover effect.
    @ViewBuilder func glassSurface(in shape: some InsettableShape, interactive: Bool = false) -> some View {
        if #available(macOS 26, *) {
            glassEffect(interactive ? .regular.interactive() : .regular, in: shape)
        } else {
            background(.regularMaterial, in: shape)
                .overlay(shape.strokeBorder(Theme.seam, lineWidth: Theme.seamWidth))
        }
    }

    /// The glass button style on macOS 26 and later, the bordered style before that.
    @ViewBuilder func glassButtonStyle() -> some View {
        if #available(macOS 26, *) {
            buttonStyle(.glass)
        } else {
            buttonStyle(.bordered)
        }
    }
}

/// Renders its glass children together on macOS 26 and later (glass cannot sample glass); a plain wrapper before that.
struct GlassGroup<Content: View>: View {
    var spacing: CGFloat? = nil
    @ViewBuilder var content: Content

    var body: some View {
        if #available(macOS 26, *) {
            GlassEffectContainer(spacing: spacing) { content }
        } else {
            content
        }
    }
}
