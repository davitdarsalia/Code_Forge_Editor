// Renders the Forge app icon (1024x1024 PNG) with AppKit.
// Usage: swift macos/make-icon.swift <output.png>

import AppKit

let output = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "icon.png"
let size: CGFloat = 1024

let image = NSImage(size: NSSize(width: size, height: size))
image.lockFocus()

// macOS icon grid: ~824pt rounded square centered on a 1024 canvas.
let inset: CGFloat = 100
let rect = NSRect(x: inset, y: inset, width: size - inset * 2, height: size - inset * 2)
let shape = NSBezierPath(roundedRect: rect, xRadius: 185, yRadius: 185)

NSGraphicsContext.current?.saveGraphicsState()
let shadow = NSShadow()
shadow.shadowColor = NSColor.black.withAlphaComponent(0.35)
shadow.shadowBlurRadius = 28
shadow.shadowOffset = NSSize(width: 0, height: -12)
shadow.set()
NSColor.black.setFill()
shape.fill()
NSGraphicsContext.current?.restoreGraphicsState()

let gradient = NSGradient(colors: [
    NSColor(srgbRed: 0.24, green: 0.55, blue: 1.0, alpha: 1),
    NSColor(srgbRed: 0.36, green: 0.25, blue: 0.85, alpha: 1),
])!
gradient.draw(in: shape, angle: -70)

// Soft top highlight.
let highlight = NSGradient(colors: [NSColor.white.withAlphaComponent(0.18), NSColor.white.withAlphaComponent(0)])!
highlight.draw(in: shape, angle: -90)

if let symbol = NSImage(systemSymbolName: "chevron.left.forwardslash.chevron.right", accessibilityDescription: nil) {
    let config = NSImage.SymbolConfiguration(pointSize: 380, weight: .semibold)
        .applying(NSImage.SymbolConfiguration(paletteColors: [.white]))
    if let glyph = symbol.withSymbolConfiguration(config) {
        let glyphSize = glyph.size
        let origin = NSPoint(x: (size - glyphSize.width) / 2, y: (size - glyphSize.height) / 2)
        glyph.draw(in: NSRect(origin: origin, size: glyphSize))
    }
}

image.unlockFocus()

guard let tiff = image.tiffRepresentation,
      let bitmap = NSBitmapImageRep(data: tiff),
      let png = bitmap.representation(using: .png, properties: [:]) else {
    fatalError("failed to encode icon")
}
try png.write(to: URL(fileURLWithPath: output))
