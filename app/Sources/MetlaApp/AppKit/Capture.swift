import AppKit
import CoreImage
import IOSurface
import QuartzCore
import MetlaCoreFFI
import MetlaKit
import SwiftUI

/// `--capture <dir>`: renders each main screen of the real window, light
/// and dark, to PNG files, from a fixture engine, then every M6 screen
/// (`CaptureExtras`), then the app quits.
///
/// The debug build takes each picture from the window server: the
/// image of one of the app's own windows, which needs no screen-recording
/// permission (the app asks for none and never touches it), with the
/// Liquid Glass panes, materials and accent colours as they are on
/// screen, at the scale of the screen the window is on (the capture moves
/// its windows to the screen with the most pixels per point). Where the
/// window server gives no image, or a blank one, and in the release
/// build, the app's own layers are painted in-process (`LayerPainter`),
/// without what the window server composes itself.
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
    static func run(
        _ model: AppModel, into directory: String, settings: @MainActor (SettingsTab) async -> NSWindow?
    ) async -> Int {
        try? FileManager.default.createDirectory(atPath: directory, withIntermediateDirectories: true)
        await settle(model)
        guard let window = NSApp.windows.first(where: { $0.isVisible }) else {
            return 0
        }
        var written = 0
        // The toolbar's items come up a moment after the first picture,
        // and the first painting of the window leaves them out: paint it
        // once and throw that away.
        sharpest(window)
        window.makeKeyAndOrderFront(nil)
        await model.show(.overview)
        try? await Task.sleep(for: .seconds(2))
        _ = image(of: window)
        let source = composed(window) == nil ? "painted in-process" : "the window server's images of its own windows"
        print("capture: \(source), \(window.backingScaleFactor)x")
        for (appearance, suffix) in [(NSAppearance.Name.aqua, "light"), (.darkAqua, "dark")] {
            await appear(appearance, window)
            for (name, screen) in screens {
                await model.show(screen)
                await prepare(model, screen)
                window.setContentSize(size)
                try? await Task.sleep(for: .milliseconds(900))
                window.setContentSize(size)
                window.displayIfNeeded()
                let file = (directory as NSString).appendingPathComponent("\(name)-\(suffix).png")
                if let png = await png(of: window, appearance), (try? png.write(to: URL(fileURLWithPath: file))) != nil {
                    written += 1
                }
            }
        }
        NSApp.appearance = nil
        return written + (await CaptureExtras.run(model, into: directory, settings: settings))
    }

    /// Switches the app to `name` and waits until the window has it.
    /// AppKit hands a new appearance to the views asynchronously (100 ms
    /// after the switch the frame view still had the old one), and the
    /// painter paints whatever the layers hold: a picture taken too soon
    /// shows the old appearance, as the M6 light Overview's dark title
    /// bar did. The app itself never sets an appearance.
    static func appear(_ name: NSAppearance.Name, _ window: NSWindow) async {
        NSApp.appearance = NSAppearance(named: name)
        let deadline = Date().addingTimeInterval(10)
        while !has(name, window), Date() < deadline {
            try? await Task.sleep(for: .milliseconds(50))
        }
        window.display()
        try? await Task.sleep(for: .milliseconds(500))
    }

    private static func has(_ name: NSAppearance.Name, _ window: NSWindow) -> Bool {
        let frame = window.contentView?.superview ?? window.contentView
        return frame?.effectiveAppearance.bestMatch(from: [.aqua, .darkAqua]) == name
    }

    /// One window, painted as `run` paints the main one, once its title
    /// bar shows `appearance`: a picture whose top band is light in the
    /// dark appearance (or dark in the light one) is painted again after
    /// a moment, up to ten times, and reported if it never matches.
    static func png(of window: NSWindow, _ appearance: NSAppearance.Name) async -> Data? {
        var picture = image(of: window)
        for _ in 0..<10 {
            guard let shown = picture, looksLight(shown, of: window) != (appearance == .aqua) else {
                break
            }
            try? await Task.sleep(for: .milliseconds(300))
            window.display()
            picture = image(of: window)
        }
        if let shown = picture, looksLight(shown, of: window) != (appearance == .aqua) {
            FileHandle.standardError.write(
                Data("capture: a picture's title bar does not show \(appearance.rawValue)\n".utf8))
        }
        return picture.flatMap { NSBitmapImageRep(cgImage: $0).representation(using: .png, properties: [:]) }
    }

    /// Whether the picture's top band (its title bar, 28 points) is light:
    /// its mean brightness above one half.
    static func looksLight(_ picture: CGImage, of window: NSWindow) -> Bool {
        let scale = CGFloat(picture.width) / max(1, window.frame.width)
        let band = CGRect(x: 0, y: 0, width: CGFloat(picture.width), height: min(28 * scale, CGFloat(picture.height)))
        guard let gray = sample(picture, band) else {
            return true
        }
        return gray.reduce(0) { $0 + Int($1) } > 128 * 128
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

    /// The window as it is on screen, else painted.
    private static func image(of window: NSWindow) -> CGImage? {
        composed(window) ?? painted(window)
    }

    /// The window server's image of one of the app's own windows, without
    /// its shadow; `nil` when it gives none or a blank one. Debug builds
    /// only. `CGWindowListCreateImage` is marked obsoleted in the macOS 26
    /// SDK, so it is looked up by name; for a process's own windows it
    /// needs no screen-recording permission.
    private static func composed(_ window: NSWindow) -> CGImage? {
        #if DEBUG
            typealias Create = @convention(c) (CGRect, UInt32, UInt32, UInt32) -> Unmanaged<CGImage>?
            guard let symbol = dlsym(dlopen(nil, RTLD_NOW), "CGWindowListCreateImage") else {
                return nil
            }
            let create = unsafeBitCast(symbol, to: Create.self)
            let includingWindow: UInt32 = 1 << 3
            let ignoreFraming: UInt32 = 1 << 0, bestResolution: UInt32 = 1 << 3
            let picture = create(.null, includingWindow, UInt32(window.windowNumber), ignoreFraming | bestResolution)?
                .takeRetainedValue()
            return picture.flatMap { blank($0) ? nil : $0 }
        #else
            return nil
        #endif
    }

    /// Whether the picture is one flat colour (what a refused capture
    /// returns).
    static func blank(_ picture: CGImage) -> Bool {
        guard let gray = sample(picture, CGRect(x: 0, y: 0, width: picture.width, height: picture.height)) else {
            return true
        }
        return gray.max()! - gray.min()! < 8
    }

    /// The region of the picture shrunk to 32 x 4 grey pixels.
    static func sample(_ picture: CGImage, _ region: CGRect) -> [UInt8]? {
        guard let part = picture.cropping(to: region),
            let context = CGContext(
                data: nil, width: 32, height: 4, bitsPerComponent: 8, bytesPerRow: 32,
                space: CGColorSpaceCreateDeviceGray(), bitmapInfo: CGImageAlphaInfo.none.rawValue),
            let bytes = context.data
        else {
            return nil
        }
        context.draw(part, in: CGRect(x: 0, y: 0, width: 32, height: 4))
        return Array(UnsafeBufferPointer(start: bytes.assumingMemoryBound(to: UInt8.self), count: 128))
    }

    /// Puts the window on the screen with the most pixels per point, so
    /// that the window server's image is as sharp as the app can be.
    static func sharpest(_ window: NSWindow) {
        guard let screen = NSScreen.screens.max(by: { $0.backingScaleFactor < $1.backingScaleFactor }),
            window.screen != screen
        else {
            return
        }
        let visible = screen.visibleFrame
        window.setFrameTopLeftPoint(NSPoint(x: visible.minX + 40, y: visible.maxY - 40))
    }

    /// The whole window, title bar and toolbar included, drawn layer by
    /// layer (`LayerPainter`): SwiftUI draws into layers that neither
    /// `cacheDisplay` nor `CALayer.render(in:)` can draw.
    private static func painted(_ window: NSWindow) -> CGImage? {
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
        return context.makeImage()
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
