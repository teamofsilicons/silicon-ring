// Readies simulator screenshots for App Store Connect, in place: drops the alpha channel and
// hides the Dynamic Island, which the iPhone simulator sometimes draws into its screenshots (a
// screenshot taken on an iPhone never shows it). The island is painted over with the colour
// beside it, row by row; nothing else in the image changes.
//
//   xcrun swift deploy/appstore/screenshots/flatten.swift <png>...
import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

for path in CommandLine.arguments.dropFirst() {
    let url = URL(fileURLWithPath: path)
    guard let source = CGImageSourceCreateWithURL(url as CFURL, nil),
          let image = CGImageSourceCreateImageAtIndex(source, 0, nil) else {
        print("\(path): not an image")
        exit(1)
    }
    let width = image.width, height = image.height
    let space = image.colorSpace ?? CGColorSpace(name: CGColorSpace.sRGB)!
    guard let context = CGContext(
        data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: width * 4, space: space,
        bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue), let data = context.data else { exit(1) }
    context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
    let pixels = data.bindMemory(to: UInt8.self, capacity: width * height * 4)
    func isBlack(_ x: Int, _ y: Int) -> Bool {
        let i = (y * width + x) * 4
        return pixels[i] < 4 && pixels[i + 1] < 4 && pixels[i + 2] < 4
    }

    // The island sits centred near the top of a portrait iPhone; its bounding box of pure black.
    var box: (minX: Int, maxX: Int, minY: Int, maxY: Int)?
    var count = 0
    if width < height, width < 1500 {
        for y in 20..<min(260, height) {
            for x in (width / 2 - 220)..<(width / 2 + 220) where isBlack(x, y) {
                count += 1
                box = (min(box?.minX ?? x, x), max(box?.maxX ?? x, x), min(box?.minY ?? y, y), max(box?.maxY ?? y, y))
            }
        }
    }
    var hidden = false
    if let box, count > 10_000, box.maxX - box.minX > 250 {
        let (left, right) = (box.minX - 4, box.maxX + 4)
        for y in max(0, box.minY - 4)...min(height - 1, box.maxY + 4) {
            let sample = (y * width + left - 6) * 4
            for x in left...right {
                let i = (y * width + x) * 4
                pixels[i] = pixels[sample]
                pixels[i + 1] = pixels[sample + 1]
                pixels[i + 2] = pixels[sample + 2]
            }
        }
        hidden = true
    }

    // The iPhone simulator also leaves a few black pixels in the top-left corner of light screens.
    var corner = false
    if isBlack(0, 0), !isBlack(6, 6) {
        let sample = (6 * width + 6) * 4
        for y in 0..<4 {
            for x in 0..<4 {
                let i = (y * width + x) * 4
                pixels[i] = pixels[sample]
                pixels[i + 1] = pixels[sample + 1]
                pixels[i + 2] = pixels[sample + 2]
            }
        }
        corner = true
    }

    guard let flattened = context.makeImage(),
          let destination = CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 1, nil) else { exit(1) }
    CGImageDestinationAddImage(destination, flattened, nil)
    guard CGImageDestinationFinalize(destination) else { exit(1) }
    print("\(path): \(width) x \(height), no alpha\(hidden ? ", Dynamic Island hidden" : "")\(corner ? ", corner cleaned" : "")")
}
