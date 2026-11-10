import AppKit
import CoreImage
import IOSurface
import QuartzCore
import MetlaCoreFFI
import MetlaKit
import SwiftUI

/// `--capture <dir>`: renders each main screen of the real window, light
/// and dark, to PNG files, from a fixture engine, then the app quits.
///
/// It draws the app's own layers in-process (no screen recording, which
/// needs a permission): sidebar, title, toolbar and the screens' content
/// as they are shown. On macOS 26 the window server composes Liquid Glass
/// itself: the glass surfaces (the sidebar's and toolbar items' panes,
/// the blur behind them) and the marks of checked boxes are left out.
@MainActor
enum Capture {
    static let screens: [(String, Screen)] = [
        ("overview", .overview),
        ("category", .category(.buildArtifact)),
        ("worktrees", .category(.gitWorktree)),
        ("folders", .folders),
    ]

    /// The window's content size in the images, in points.
    static let size = NSSize(width: 1120, height: 720)

    /// Renders every screen in both appearances; how many files it wrote.
    static func run(_ model: AppModel, into directory: String) async -> Int {
        try? FileManager.default.createDirectory(atPath: directory, withIntermediateDirectories: true)
        await settle(model)
        guard let window = NSApp.windows.first(where: { $0.isVisible }) else {
            return 0
        }
        var written = 0
        for (appearance, suffix) in [(NSAppearance.Name.aqua, "light"), (.darkAqua, "dark")] {
            NSApp.appearance = NSAppearance(named: appearance)
            for (name, screen) in screens {
                await model.show(screen)
                await prepare(model, screen)
                window.setContentSize(size)
                try? await Task.sleep(for: .milliseconds(900))
                window.setContentSize(size)
                window.displayIfNeeded()
                let file = (directory as NSString).appendingPathComponent("\(name)-\(suffix).png")
                if let png = image(of: window), (try? png.write(to: URL(fileURLWithPath: file))) != nil {
                    written += 1
                }
            }
        }
        return written
    }

    /// Waits until the first scan is done and the worktrees are known.
    private static func settle(_ model: AppModel) async {
        let deadline = Date().addingTimeInterval(60)
        while Date() < deadline {
            await model.refresh([.picture])
            if model.freshness?.fullScan != nil, case .idle? = model.freshness?.state {
                break
            }
            try? await Task.sleep(for: .milliseconds(200))
        }
        try? await Task.sleep(for: .seconds(2))
        await model.refresh([.picture, .worktrees])
    }

    /// Waits until the worktrees' states are known; opens the outline a
    /// few levels deep and selects a row in it.
    private static func prepare(_ model: AppModel, _ screen: Screen) async {
        if case .category(let category) = screen, let table = model.category(category) {
            let deadline = Date().addingTimeInterval(30)
            while table.rows.contains(where: { $0.state == model.strings.text("worktree.checking") }), Date() < deadline {
                try? await Task.sleep(for: .milliseconds(300))
                await table.reload()
            }
        }
        guard case .folders = screen, let folders = model.folders else {
            return
        }
        for name in ["home", "code", "app"] {
            let level = folders.top + folders.top.flatMap { $0.children ?? [] }
                + folders.top.flatMap { ($0.children ?? []).flatMap { $0.children ?? [] } }
            if let node = level.first(where: { $0.name == name && $0.isDirectory }) {
                await folders.expand(node)
                folders.selection = node.id
            }
        }
    }

    /// The whole window, title bar and toolbar included, drawn layer by
    /// layer (`LayerPainter`): SwiftUI draws into layers that neither
    /// `cacheDisplay` nor `CALayer.render(in:)` can draw.
    private static func image(of window: NSWindow) -> Data? {
        guard let frame = window.contentView?.superview, let layer = frame.layer else {
            return nil
        }
        let scale = 2.0
        let size = layer.bounds.size
        let width = Int(size.width * scale)
        let height = Int(size.height * scale)
        let info = CGBitmapInfo.byteOrder32Little.rawValue | CGImageAlphaInfo.premultipliedFirst.rawValue
        guard let space = CGColorSpace(name: CGColorSpace.sRGB),
            let context = CGContext(
                data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
                space: space, bitmapInfo: info)
        else {
            return nil
        }
        var background = CGColor(gray: 1, alpha: 1)
        window.effectiveAppearance.performAsCurrentDrawingAppearance {
            background = NSColor.windowBackgroundColor.cgColor
        }
        context.setFillColor(background)
        context.fill(CGRect(x: 0, y: 0, width: width, height: height))
        context.scaleBy(x: scale, y: scale)
        window.effectiveAppearance.performAsCurrentDrawingAppearance {
            LayerPainter.paint(layer, in: context, flipped: false, alpha: 1)
            for view in detached(below: frame) {
                let place = view.convert(view.bounds, to: frame)
                context.saveGState()
                context.translateBy(x: place.minX, y: place.minY)
                if let own = view.layer {
                    LayerPainter.paint(own, in: context, flipped: false, alpha: 1, placed: true)
                }
                context.restoreGState()
            }
        }
        guard let image = context.makeImage() else {
            return nil
        }
        return NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:])
    }
}

/// Views whose layer is not in their superview's layer tree: Liquid Glass
/// shows such content (the sidebar, toolbar items) through a portal, so
/// the window's root layer does not reach it.
@MainActor
private func detached(below root: NSView) -> [NSView] {
    var found: [NSView] = []
    var pending = root.subviews
    while !pending.isEmpty {
        let view = pending.removeFirst()
        if let layer = view.layer, let parent = view.superview?.layer, layer.superlayer !== parent,
            !hasAncestor(layer, parent)
        {
            found.append(view)
            continue
        }
        pending.append(contentsOf: view.subviews)
    }
    return found
}

private func hasAncestor(_ layer: CALayer, _ ancestor: CALayer) -> Bool {
    var current = layer.superlayer
    while let next = current {
        if next === ancestor { return true }
        current = next.superlayer
    }
    return false
}

/// Draws a layer tree into a bitmap with public `CALayer` API: geometry,
/// opacity, clipping, background, border, image contents, and the layer's
/// own drawing. Blurs and other effects of the window server are left out.
@MainActor
enum LayerPainter {
    /// Paints `layer` and what lies on it; `flipped` says whether its
    /// parent's coordinates run top down.
    static func paint(
        _ layer: CALayer, in context: CGContext, flipped: Bool, alpha: CGFloat, placed: Bool = false
    ) {
        let alpha = alpha * CGFloat(layer.opacity)
        guard !layer.isHidden, alpha > 0.01, !effect(layer) else {
            return
        }
        context.saveGState()
        defer { context.restoreGState() }
        let bounds = layer.bounds
        if !placed {
            context.translateBy(x: layer.position.x, y: layer.position.y)
            context.concatenate(layer.affineTransform())
            context.translateBy(x: -bounds.width * layer.anchorPoint.x, y: -bounds.height * layer.anchorPoint.y)
        }
        context.translateBy(x: -bounds.origin.x, y: -bounds.origin.y)
        context.setAlpha(alpha)
        if layer.masksToBounds {
            context.addPath(CGPath(roundedRect: bounds, cornerWidth: layer.cornerRadius, cornerHeight: layer.cornerRadius, transform: nil))
            context.clip()
        }
        let glass = layer.delegate.map { NSStringFromClass(type(of: $0)).contains("Glass") } ?? false
        if !glass {
            own(layer, in: context, flipped: flipped)
        }
        let inner = flipped != layer.isGeometryFlipped
        if layer.isGeometryFlipped {
            context.translateBy(x: 0, y: bounds.minY + bounds.maxY)
            context.scaleBy(x: 1, y: -1)
        }
        let sublayers = (layer.sublayers ?? []).sorted { $0.zPosition < $1.zPosition }
        for sublayer in sublayers {
            paint(sublayer, in: context, flipped: inner, alpha: alpha)
        }
    }

    /// A layer's contents as an image, and the colour a template image is
    /// tinted with (Core Animation's private `CATintedImage` holds both;
    /// vibrant labels use it).
    static func picture(_ layer: CALayer) -> (CGImage, CGColor?)? {
        guard let contents = layer.contents else {
            return nil
        }
        let object = contents as AnyObject
        if let image = image(object) {
            return (image, nil)
        }
        let name = object_getClass(object).map(NSStringFromClass) ?? "?"
        let drawing = NSStringFromClass(type(of: layer)).contains("Drawing")
        guard name == "CATintedImage", drawing,
            let inner = object.perform(NSSelectorFromString("image"))?.takeUnretainedValue(),
            let image = image(inner)
        else {
            return nil
        }
        let tint = object.perform(NSSelectorFromString("tint"))?.takeUnretainedValue()
        let color = tint.flatMap { CFGetTypeID($0) == CGColor.typeID ? ($0 as! CGColor) : nil }
        // Without a tint of its own the window server colours it (vibrancy):
        // the label colour of the appearance being drawn.
        return (image, color ?? NSColor.labelColor.cgColor)
    }

    /// A `CGImage`, or an IOSurface's pixels as one.
    static func image(_ object: AnyObject) -> CGImage? {
        if CFGetTypeID(object) == CGImage.typeID {
            return (object as! CGImage)
        }
        guard CFGetTypeID(object) == IOSurfaceGetTypeID() else {
            return nil
        }
        let surface = object as! IOSurfaceRef
        let image = CIImage(ioSurface: surface)
        return CIContext().createCGImage(image, from: image.extent)
    }

    /// Blurs, mirrors and glass shapes the window server composes from
    /// other layers: what they hold is a mask, not what is shown.
    static func effect(_ layer: CALayer) -> Bool {
        let name = NSStringFromClass(type(of: layer))
        let owner = layer.delegate.map { NSStringFromClass(type(of: $0)) } ?? ""
        return name.contains("Backdrop") || name.contains("Portal") || name.contains("SDF")
            || owner.hasPrefix("_NSCoreHostingView")
    }

    /// Whether the layer's own drawing shows on screen: a view's layer
    /// without contents shows nothing of its own, and one whose pixels sit
    /// in a content sublayer shows them there (drawing it as well would
    /// draw a title twice).
    static func drawn(_ layer: CALayer) -> Bool {
        guard NSStringFromClass(type(of: layer)) == "NSViewBackingLayer" else {
            return true
        }
        let content = (layer.sublayers ?? []).contains {
            NSStringFromClass(type(of: $0)).contains("NSViewBackingLayerContents")
        }
        return layer.contents != nil && !content
    }

    /// What the layer itself shows: its background, its image or its own
    /// drawing (a view's `draw(_:)` included), its border.
    private static func own(_ layer: CALayer, in context: CGContext, flipped: Bool) {
        let bounds = layer.bounds
        if let color = layer.backgroundColor {
            context.setFillColor(color)
            context.addPath(CGPath(roundedRect: bounds, cornerWidth: layer.cornerRadius, cornerHeight: layer.cornerRadius, transform: nil))
            context.fillPath()
        }
        if let (image, tint) = picture(layer) {
            context.saveGState()
            if flipped {
                context.translateBy(x: 0, y: bounds.minY + bounds.maxY)
                context.scaleBy(x: 1, y: -1)
            }
            if let tint {
                context.clip(to: bounds, mask: image)
                context.setFillColor(tint)
                context.fill(bounds)
            } else {
                context.draw(image, in: bounds)
            }
            context.restoreGState()
        } else if drawn(layer) {
            context.saveGState()
            let previous = NSGraphicsContext.current
            NSGraphicsContext.current = NSGraphicsContext(cgContext: context, flipped: flipped)
            layer.draw(in: context)
            NSGraphicsContext.current = previous
            context.restoreGState()
        }
        if layer.borderWidth > 0, let color = layer.borderColor {
            context.setStrokeColor(color)
            context.setLineWidth(layer.borderWidth)
            context.addPath(CGPath(roundedRect: bounds.insetBy(dx: layer.borderWidth / 2, dy: layer.borderWidth / 2), cornerWidth: layer.cornerRadius, cornerHeight: layer.cornerRadius, transform: nil))
            context.strokePath()
        }
    }
}
