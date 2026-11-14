// Draws the WatchSync app icon and writes assets/WatchSync.icns.
// Usage: swift scripts/make-icon.swift
import AppKit

let size: CGFloat = 1024

// Monochrome, like Tars: ink (#18181b) on Tars's page colour (#fafaf8).
let ink = CGColor(red: 0x18 / 255, green: 0x18 / 255, blue: 0x1b / 255, alpha: 1)
let page = CGColor(red: 0xfa / 255, green: 0xfa / 255, blue: 0xf8 / 255, alpha: 1)
let line = CGColor(red: 0xe6 / 255, green: 0xe6 / 255, blue: 0xe9 / 255, alpha: 1)

func drawIcon(_ ctx: CGContext) {
    // macOS icon grid: 824pt body inset 100pt.
    let body = CGRect(x: 100, y: 100, width: 824, height: 824)
    let shape = CGPath(roundedRect: body, cornerWidth: 186, cornerHeight: 186, transform: nil)

    // Soft drop shadow, then the off-white tile with a hairline edge.
    ctx.saveGState()
    ctx.setShadow(offset: CGSize(width: 0, height: -10), blur: 24, color: CGColor(gray: 0, alpha: 0.22))
    ctx.addPath(shape)
    ctx.setFillColor(page)
    ctx.fillPath()
    ctx.restoreGState()
    ctx.addPath(shape)
    ctx.setStrokeColor(line)
    ctx.setLineWidth(4)
    ctx.strokePath()

    // Rounded play triangle
    func play(center: CGPoint, height h: CGFloat) -> CGPath {
        let w = h * 0.9
        let a = CGPoint(x: center.x - w * 0.4, y: center.y + h / 2)
        let b = CGPoint(x: center.x + w * 0.6, y: center.y)
        let c = CGPoint(x: center.x - w * 0.4, y: center.y - h / 2)
        let p = CGMutablePath()
        let r = h * 0.09
        p.move(to: CGPoint(x: (a.x + c.x) / 2, y: (a.y + c.y) / 2))
        p.addArc(tangent1End: a, tangent2End: b, radius: r)
        p.addArc(tangent1End: b, tangent2End: c, radius: r)
        p.addArc(tangent1End: c, tangent2End: a, radius: r)
        p.closeSubpath()
        return p
    }

    // Two play buttons, one behind the other: "watching together, in sync".
    // Same shapes as the in-app mark; the one behind is a tint of the ink.
    ctx.addPath(play(center: CGPoint(x: 438, y: 512), height: 380))
    ctx.setFillColor(ink.copy(alpha: 0.28)!)
    ctx.fillPath()
    ctx.addPath(play(center: CGPoint(x: 560, y: 512), height: 380))
    ctx.setFillColor(ink)
    ctx.fillPath()
}

func png(pixels: Int) -> Data {
    let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: pixels, pixelsHigh: pixels,
                               bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
                               colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    let ctx = NSGraphicsContext(bitmapImageRep: rep)!.cgContext
    let scale = CGFloat(pixels) / size
    ctx.scaleBy(x: scale, y: scale)
    drawIcon(ctx)
    return rep.representation(using: .png, properties: [:])!
}

let fm = FileManager.default
let iconset = URL(fileURLWithPath: NSTemporaryDirectory()).appendingPathComponent("WatchSync.iconset")
try? fm.removeItem(at: iconset)
try! fm.createDirectory(at: iconset, withIntermediateDirectories: true)
for base in [16, 32, 128, 256, 512] {
    try! png(pixels: base).write(to: iconset.appendingPathComponent("icon_\(base)x\(base).png"))
    try! png(pixels: base * 2).write(to: iconset.appendingPathComponent("icon_\(base)x\(base)@2x.png"))
}
try! png(pixels: 1024).write(to: URL(fileURLWithPath: "assets/icon-1024.png"))

let task = Process()
task.executableURL = URL(fileURLWithPath: "/usr/bin/iconutil")
task.arguments = ["-c", "icns", iconset.path, "-o", "assets/WatchSync.icns"]
try! task.run()
task.waitUntilExit()
print(task.terminationStatus == 0 ? "Wrote assets/WatchSync.icns" : "iconutil failed")
