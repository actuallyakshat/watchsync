// Draws the WatchSync app icon and writes assets/WatchSync.icns.
// Usage: swift scripts/make-icon.swift
import AppKit

let size: CGFloat = 1024

func drawIcon(_ ctx: CGContext) {
    // macOS icon grid: 824pt body inset 100pt, continuous-ish corners.
    let body = CGRect(x: 100, y: 100, width: 824, height: 824)
    let shape = CGPath(roundedRect: body, cornerWidth: 186, cornerHeight: 186, transform: nil)

    // Soft drop shadow
    ctx.saveGState()
    ctx.setShadow(offset: CGSize(width: 0, height: -12), blur: 28,
                  color: CGColor(gray: 0, alpha: 0.35))
    ctx.addPath(shape)
    ctx.setFillColor(CGColor(red: 0.08, green: 0.08, blue: 0.11, alpha: 1))
    ctx.fillPath()
    ctx.restoreGState()

    // Background gradient (deep night blue → near black)
    ctx.saveGState()
    ctx.addPath(shape)
    ctx.clip()
    let space = CGColorSpaceCreateDeviceRGB()
    let bg = CGGradient(colorsSpace: space, colors: [
        CGColor(red: 0.17, green: 0.16, blue: 0.27, alpha: 1),
        CGColor(red: 0.07, green: 0.07, blue: 0.10, alpha: 1),
    ] as CFArray, locations: [0, 1])!
    ctx.drawLinearGradient(bg, start: CGPoint(x: 512, y: 924), end: CGPoint(x: 512, y: 100), options: [])

    // Warm glow behind the play buttons
    let glow = CGGradient(colorsSpace: space, colors: [
        CGColor(red: 0.88, green: 0.64, blue: 0.23, alpha: 0.35),
        CGColor(red: 0.88, green: 0.64, blue: 0.23, alpha: 0),
    ] as CFArray, locations: [0, 1])!
    ctx.drawRadialGradient(glow, startCenter: CGPoint(x: 540, y: 500), startRadius: 0,
                           endCenter: CGPoint(x: 540, y: 500), endRadius: 420, options: [])
    ctx.restoreGState()

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
    ctx.addPath(play(center: CGPoint(x: 430, y: 512), height: 400))
    ctx.setFillColor(CGColor(red: 1, green: 1, blue: 1, alpha: 0.22))
    ctx.fillPath()

    ctx.saveGState()
    ctx.setShadow(offset: CGSize(width: 0, height: -8), blur: 24,
                  color: CGColor(red: 0, green: 0, blue: 0, alpha: 0.45))
    ctx.addPath(play(center: CGPoint(x: 560, y: 512), height: 400))
    let amber = CGGradient(colorsSpace: space, colors: [
        CGColor(red: 0.98, green: 0.76, blue: 0.33, alpha: 1),
        CGColor(red: 0.88, green: 0.56, blue: 0.16, alpha: 1),
    ] as CFArray, locations: [0, 1])!
    ctx.clip()
    ctx.drawLinearGradient(amber, start: CGPoint(x: 512, y: 712), end: CGPoint(x: 512, y: 312), options: [])
    ctx.restoreGState()
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
