// Draws Metla's app icon (a broom on a blue tile) and writes it as PNG.
//
//   swift scripts/make-icon.swift <out.png> [pixels]
//
// The drawing is made of shapes only, on a 1024-unit canvas, so every size
// of the icon set is drawn at its own resolution and nothing is scaled.

import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

let arguments = CommandLine.arguments
guard arguments.count >= 2 else {
    FileHandle.standardError.write(Data("usage: make-icon.swift <out.png> [pixels]\n".utf8))
    exit(2)
}
let pixels = arguments.count > 2 ? Int(arguments[2]) ?? 1024 : 1024

func colour(_ hex: UInt32, _ alpha: CGFloat = 1) -> CGColor {
    CGColor(
        srgbRed: CGFloat((hex >> 16) & 0xFF) / 255,
        green: CGFloat((hex >> 8) & 0xFF) / 255,
        blue: CGFloat(hex & 0xFF) / 255,
        alpha: alpha)
}

let space = CGColorSpace(name: CGColorSpace.sRGB)!
let context = CGContext(
    data: nil, width: pixels, height: pixels, bitsPerComponent: 8, bytesPerRow: 0,
    space: space, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
context.scaleBy(x: CGFloat(pixels) / 1024, y: CGFloat(pixels) / 1024)
context.setShouldAntialias(true)
context.interpolationQuality = .high

func gradient(_ colours: [CGColor], _ stops: [CGFloat]) -> CGGradient {
    CGGradient(colorsSpace: space, colors: colours as CFArray, locations: stops)!
}

// The tile: the shape and margins of a macOS icon (824 of 1024, radius 185).
let tile = CGPath(
    roundedRect: CGRect(x: 100, y: 100, width: 824, height: 824),
    cornerWidth: 185, cornerHeight: 185, transform: nil)

context.saveGState()
context.setShadow(offset: CGSize(width: 0, height: -10), blur: 24, color: colour(0x000000, 0.30))
context.addPath(tile)
context.setFillColor(colour(0x1F6FE0))
context.fillPath()
context.restoreGState()

context.saveGState()
context.addPath(tile)
context.clip()
context.drawLinearGradient(
    gradient([colour(0x4FC3FF), colour(0x2B7BF0), colour(0x2440C8)], [0, 0.55, 1]),
    start: CGPoint(x: 512, y: 924), end: CGPoint(x: 512, y: 100), options: [])
// A soft light from the top left.
context.drawRadialGradient(
    gradient([colour(0xFFFFFF, 0.28), colour(0xFFFFFF, 0)], [0, 1]),
    startCenter: CGPoint(x: 300, y: 860), startRadius: 0,
    endCenter: CGPoint(x: 300, y: 860), endRadius: 620, options: [])
context.restoreGState()

// A four-pointed sparkle: what is left where the broom has been.
func sparkle(at centre: CGPoint, radius: CGFloat, alpha: CGFloat) {
    let path = CGMutablePath()
    let waist = radius * 0.16
    path.move(to: CGPoint(x: centre.x, y: centre.y + radius))
    path.addQuadCurve(
        to: CGPoint(x: centre.x + radius, y: centre.y),
        control: CGPoint(x: centre.x + waist, y: centre.y + waist))
    path.addQuadCurve(
        to: CGPoint(x: centre.x, y: centre.y - radius),
        control: CGPoint(x: centre.x + waist, y: centre.y - waist))
    path.addQuadCurve(
        to: CGPoint(x: centre.x - radius, y: centre.y),
        control: CGPoint(x: centre.x - waist, y: centre.y - waist))
    path.addQuadCurve(
        to: CGPoint(x: centre.x, y: centre.y + radius),
        control: CGPoint(x: centre.x - waist, y: centre.y + waist))
    context.addPath(path)
    context.setFillColor(colour(0xFFFFFF, alpha))
    context.fillPath()
}

context.saveGState()
context.addPath(tile)
context.clip()
sparkle(at: CGPoint(x: 716, y: 372), radius: 62, alpha: 0.95)
sparkle(at: CGPoint(x: 612, y: 246), radius: 34, alpha: 0.80)
sparkle(at: CGPoint(x: 800, y: 250), radius: 24, alpha: 0.65)
context.restoreGState()

// The broom, drawn upright around the origin and turned so the handle
// points to the top right and the bristles sweep the bottom left.
context.saveGState()
context.translateBy(x: 524, y: 530)
context.rotate(by: -.pi / 4)
context.setShadow(offset: CGSize(width: 0, height: -14), blur: 30, color: colour(0x0A1E6E, 0.45))
context.beginTransparencyLayer(auxiliaryInfo: nil)

// Handle.
let handle = CGPath(
    roundedRect: CGRect(x: -24, y: 20, width: 48, height: 372),
    cornerWidth: 24, cornerHeight: 24, transform: nil)
context.saveGState()
context.addPath(handle)
context.clip()
context.drawLinearGradient(
    gradient([colour(0xFFFFFF), colour(0xDCE9FF)], [0, 1]),
    start: CGPoint(x: -24, y: 0), end: CGPoint(x: 24, y: 0), options: [])
context.restoreGState()

// Bristles: a fan that widens downwards, with a softly rounded edge.
let bristles = CGMutablePath()
bristles.move(to: CGPoint(x: -84, y: -8))
bristles.addLine(to: CGPoint(x: 84, y: -8))
bristles.addQuadCurve(to: CGPoint(x: 208, y: -292), control: CGPoint(x: 150, y: -120))
bristles.addQuadCurve(to: CGPoint(x: 176, y: -330), control: CGPoint(x: 214, y: -326))
bristles.addQuadCurve(to: CGPoint(x: -176, y: -330), control: CGPoint(x: 0, y: -356))
bristles.addQuadCurve(to: CGPoint(x: -208, y: -292), control: CGPoint(x: -214, y: -326))
bristles.addQuadCurve(to: CGPoint(x: -84, y: -8), control: CGPoint(x: -150, y: -120))
bristles.closeSubpath()
context.saveGState()
context.addPath(bristles)
context.clip()
context.drawLinearGradient(
    gradient([colour(0xFFE89A), colour(0xFFC53D), colour(0xF59A0C)], [0, 0.55, 1]),
    start: CGPoint(x: 0, y: -8), end: CGPoint(x: 0, y: -340), options: [])
// Strands.
context.setLineCap(.round)
context.setLineWidth(9)
context.setStrokeColor(colour(0xC76A00, 0.38))
for step in -3...3 where step != 0 {
    let top = CGFloat(step) * 22
    let bottom = CGFloat(step) * 54
    context.move(to: CGPoint(x: top, y: -70))
    context.addQuadCurve(
        to: CGPoint(x: bottom, y: -318),
        control: CGPoint(x: top + (bottom - top) * 0.35, y: -180))
    context.strokePath()
}
context.move(to: CGPoint(x: 0, y: -70))
context.addLine(to: CGPoint(x: 0, y: -326))
context.strokePath()
context.restoreGState()

// Binding: the band that holds the bristles on the handle.
let binding = CGPath(
    roundedRect: CGRect(x: -98, y: -34, width: 196, height: 84),
    cornerWidth: 26, cornerHeight: 26, transform: nil)
context.saveGState()
context.addPath(binding)
context.clip()
context.drawLinearGradient(
    gradient([colour(0xFFFFFF), colour(0xD3E2FF)], [0, 1]),
    start: CGPoint(x: 0, y: 50), end: CGPoint(x: 0, y: -34), options: [])
context.setFillColor(colour(0x2B5FD9, 0.55))
context.fill(CGRect(x: -98, y: 2, width: 196, height: 12))
context.restoreGState()

context.endTransparencyLayer()
context.restoreGState()

let url = URL(fileURLWithPath: arguments[1])
guard
    let image = context.makeImage(),
    let destination = CGImageDestinationCreateWithURL(
        url as CFURL, UTType.png.identifier as CFString, 1, nil)
else {
    FileHandle.standardError.write(Data("cannot write \(arguments[1])\n".utf8))
    exit(1)
}
CGImageDestinationAddImage(destination, image, nil)
guard CGImageDestinationFinalize(destination) else {
    FileHandle.standardError.write(Data("cannot write \(arguments[1])\n".utf8))
    exit(1)
}
