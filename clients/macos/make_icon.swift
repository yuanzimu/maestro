// make_icon.swift — 生成 App 图标（渐变 + "M"），iconutil 转 .icns
// 运行：swift make_icon.swift
import Foundation
import CoreGraphics
import CoreText
import ImageIO

func render(size: Int) -> CGImage? {
    let w = size, h = size
    guard let ctx = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8,
                              bytesPerRow: 0, space: CGColorSpace(name: CGColorSpace.sRGB)!,
                              bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return nil }
    let rect = CGRect(x: 0, y: 0, width: w, height: h)
    // macOS squircle：圆角 ≈ 边长 22.37%
    let radius = CGFloat(w) * 0.2237
    let path = CGPath(roundedRect: rect, cornerWidth: radius, cornerHeight: radius, transform: nil)

    ctx.saveGState()
    ctx.addPath(path)
    ctx.clip()
    let colors = [CGColor(red: 0.30, green: 0.18, blue: 0.78, alpha: 1),
                  CGColor(red: 0.08, green: 0.48, blue: 0.95, alpha: 1)] as CFArray
    if let grad = CGGradient(colorsSpace: CGColorSpace(name: CGColorSpace.sRGB)!,
                             colors: colors, locations: [0, 1]) {
        ctx.drawLinearGradient(grad, start: .zero, end: CGPoint(x: 0, y: CGFloat(h)), options: [])
    }
    ctx.restoreGState()

    // "M"
    let font = CTFontCreateWithName("Helvetica-Bold" as CFString, CGFloat(w) * 0.50, nil)
    let attrs: [CFString: Any] = [kCTFontAttributeName: font,
                                  kCTForegroundColorAttributeName: CGColor(red: 1, green: 1, blue: 1, alpha: 1)]
    guard let str = CFAttributedStringCreate(nil, "M" as CFString, attrs as CFDictionary) else { return nil }
    let line = CTLineCreateWithAttributedString(str)
    let bounds = CTLineGetBoundsWithOptions(line, .useOpticalBounds)
    ctx.textPosition = CGPoint(x: (CGFloat(w) - bounds.width) / 2 - bounds.minX,
                               y: (CGFloat(h) - bounds.height) / 2 - bounds.minY)
    CTLineDraw(line, ctx)

    return ctx.makeImage()
}

func writePNG(_ image: CGImage, to path: String) {
    let url = URL(fileURLWithPath: path) as CFURL
    guard let dest = CGImageDestinationCreateWithURL(url, "public.png" as CFString, 1, nil) else { return }
    CGImageDestinationAddImage(dest, image, nil)
    CGImageDestinationFinalize(dest)
}

let fm = FileManager.default
let iconset = "icon.iconset"
try? fm.createDirectory(atPath: iconset, withIntermediateDirectories: true)

let sizes: [(name: String, px: Int)] = [
    ("icon_16x16.png", 16), ("icon_16x16@2x.png", 32),
    ("icon_32x32.png", 32), ("icon_32x32@2x.png", 64),
    ("icon_128x128.png", 128), ("icon_128x128@2x.png", 256),
    ("icon_256x256.png", 256), ("icon_256x256@2x.png", 512),
    ("icon_512x512.png", 512), ("icon_512x512@2x.png", 1024),
]
for (name, px) in sizes {
    if let img = render(size: px) {
        writePNG(img, to: iconset + "/" + name)
    }
}

let p = Process()
p.executableURL = URL(fileURLWithPath: "/usr/bin/iconutil")
p.arguments = ["-c", "icns", iconset, "-o", "AppIcon.icns"]
try? p.run()
p.waitUntilExit()
print(p.terminationStatus == 0 ? "✅ AppIcon.icns 已生成" : "❌ iconutil 失败")
