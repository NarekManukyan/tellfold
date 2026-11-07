import AppKit
import SwiftUI

/// What the shared pointer handling needs from a canvas controller (the knowledge graph's and the crowd graph's).
@MainActor
protocol CanvasGesturing: AnyObject {
    var isHovering: Bool { get }
    var hasUserCamera: Bool { get }
    func hover(_ phase: HoverPhase)
    func dragChanged(start: CGPoint, location: CGPoint, translation: CGSize)
    func dragEnded()
    func magnifyChanged(_ magnification: Double, anchor: CGPoint)
    func magnifyEnded()
    func scroll(dx: Double, dy: Double, precise: Bool, zoomModifier: Bool)
    func zoom(by factor: Double, anchor: CGPoint?)
    func fit()
}

extension GraphCanvasController: CanvasGesturing {
    var hasUserCamera: Bool { userCamera != nil }
}

/// Pointer, gesture, scroll-wheel and keyboard handling shared by the graph canvases.
/// - drag on empty space pans, drag on a node moves it where the canvas allows (released when the drag ends)
/// - pinch zooms about the pinch point; trackpad scroll pans; mouse wheel or ⌘-scroll zooms about the pointer
/// - click selects through `onTap`; `+` / `-` zoom, `0` re-frames, Escape clears selection through `onEscape`
struct CanvasInteraction: ViewModifier {
    let controller: any CanvasGesturing
    var onTap: (CGPoint) -> Void
    var onEscape: () -> Void

    @State private var scrollMonitor: ScrollMonitorToken?

    func body(content: Content) -> some View {
        content
            .contentShape(Rectangle())
            .onContinuousHover(coordinateSpace: .local) { controller.hover($0) }
            .gesture(
                DragGesture(minimumDistance: 3, coordinateSpace: .local)
                    .onChanged { v in
                        controller.dragChanged(start: v.startLocation, location: v.location, translation: v.translation)
                    }
                    .onEnded { _ in controller.dragEnded() }
            )
            .simultaneousGesture(
                MagnifyGesture()
                    .onChanged { v in controller.magnifyChanged(v.magnification, anchor: v.startLocation) }
                    .onEnded { _ in controller.magnifyEnded() }
            )
            .simultaneousGesture(
                SpatialTapGesture(coordinateSpace: .local).onEnded { v in onTap(v.location) }
            )
            .focusable()
            .focusEffectDisabled()
            .onKeyPress(characters: CharacterSet(charactersIn: "+=-0")) { press in
                switch press.characters {
                case "+", "=": controller.zoom(by: 1.25, anchor: nil)
                case "-": controller.zoom(by: 0.8, anchor: nil)
                default: controller.fit()
                }
                return .handled
            }
            .onKeyPress(.escape) {
                onEscape()
                return .handled
            }
            .onChange(of: controller.isHovering, initial: true) { _, hovering in
                if hovering { installMonitor() } else { removeMonitor() }
            }
            .onDisappear { removeMonitor() }
    }

    private func installMonitor() {
        guard scrollMonitor == nil else { return }
        let controller = controller
        scrollMonitor = ScrollMonitorToken(NSEvent.addLocalMonitorForEvents(matching: .scrollWheel) { event in
            let dx = event.scrollingDeltaX, dy = event.scrollingDeltaY
            let precise = event.hasPreciseScrollingDeltas
            let zoomModifier = !event.modifierFlags.isDisjoint(with: [.command, .option])
            // Local monitors run on the main thread.
            let consumed = MainActor.assumeIsolated { () -> Bool in
                guard controller.isHovering else { return false }
                controller.scroll(dx: dx, dy: dy, precise: precise, zoomModifier: zoomModifier)
                return true
            }
            return consumed ? nil : event
        })
    }

    private func removeMonitor() {
        scrollMonitor?.remove()
        scrollMonitor = nil
    }
}

/// Owns an `NSEvent` monitor handle.
@MainActor
final class ScrollMonitorToken {
    private var handle: Any?

    init(_ handle: Any?) { self.handle = handle }

    func remove() {
        if let handle { NSEvent.removeMonitor(handle) }
        handle = nil
    }
}

extension View {
    func graphCanvasInteraction(_ controller: any CanvasGesturing, onTap: @escaping (CGPoint) -> Void,
                                onEscape: @escaping () -> Void) -> some View {
        modifier(CanvasInteraction(controller: controller, onTap: onTap, onEscape: onEscape))
    }
}

/// Zoom in / out / fit buttons overlaid on a canvas.
struct CanvasZoomControls: View {
    let controller: any CanvasGesturing
    /// Re-reads `hasUserCamera` when the owner re-renders; the fit button is disabled while the camera is automatic.
    var hasUserCamera = true

    var body: some View {
        // One container so the three glass circles are rendered together (glass cannot sample glass).
        GlassEffectContainer(spacing: 6) {
            HStack(spacing: 8) {
                button("minus.magnifyingglass", "Zoom out") { controller.zoom(by: 0.8, anchor: nil) }
                button("plus.magnifyingglass", "Zoom in") { controller.zoom(by: 1.25, anchor: nil) }
                button("arrow.up.left.and.arrow.down.right", "Fit to view") { controller.fit() }
                    .disabled(!hasUserCamera)
            }
        }
    }

    private func button(_ symbol: String, _ label: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: symbol)
                .font(.system(size: 13, weight: .medium))
                .foregroundStyle(Theme.secondary)
                .frame(width: 32, height: 32)
                .contentShape(Circle())
        }
        .buttonStyle(.plain)
        .glassEffect(.regular.interactive(), in: .circle)
        .help(label)
        .accessibilityLabel(label)
    }
}
