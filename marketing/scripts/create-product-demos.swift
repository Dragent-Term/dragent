// Encode continuous native-window recordings. No still-image transitions or zooms.
// Input: NAME/frame-NNNNNN-MILLISECONDS.png and optional NAME/actions.json.
// Run: swift scripts/create-product-demos.swift /path/to/recordings public/demos
import Foundation
import AVFoundation
import AppKit

let input = URL(fileURLWithPath: CommandLine.arguments[1])
let output = URL(fileURLWithPath: CommandLine.arguments[2])
try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)
struct Frame { let url: URL; let ms: Double }
struct Action: Decodable { let label: String; let before: Double; let after: Double }
struct Chapter: Codable { let label: String; let start: Double; let end: Double }
struct Recording: Codable { let duration: Double; let frames: Int; let kind: String; let chapters: [Chapter] }
let descriptions: [String: [(String, String)]] = [
    "reviews": [("files", "Inspect files"), ("select-line", "Draft a comment"), ("checks", "Check CI"), ("activity", "Read the discussion")],
    "coordination": [("activity", "Read handoffs"), ("plan", "Follow the plan"), ("draft-direction", "Draft direction")],
    "remote": [("agents", "Check installed agents"), ("overview", "Inspect host resources")],
    "workspace": [("describe-task", "Describe the task"), ("refine-scope", "Refine the scope"), ("verification", "Add verification")],
    "agent": [("approval", "Review the approval")]
]
var manifest = (try? JSONDecoder().decode([String: Recording].self, from: Data(contentsOf: output.appendingPathComponent("manifest.json")))) ?? [:]
func vttTime(_ value: Double) -> String { let ms = Int(value * 1000); return String(format: "%02d:%02d:%02d.%03d", ms / 3600000, ms / 60000 % 60, ms / 1000 % 60, ms % 1000) }
let selected = CommandLine.arguments.count > 3 ? Array(CommandLine.arguments.dropFirst(3)) : ["reviews", "coordination", "remote", "workspace", "agent"]
for name in selected {
    let folder = input.appendingPathComponent(name)
    var frames = try FileManager.default.contentsOfDirectory(at: folder, includingPropertiesForKeys: nil).compactMap { url -> Frame? in
        let parts = url.deletingPathExtension().lastPathComponent.split(separator: "-")
        guard parts.count == 3, parts[0] == "frame", let ms = Double(parts[2]) else { return nil }
        return Frame(url: url, ms: ms)
    }.sorted { $0.ms < $1.ms }
    // A recorder stopped by SIGTERM may leave its final encoder job incomplete.
    while let last = frames.last, (CGImageSourceCreateWithURL(last.url as CFURL, nil).flatMap { CGImageSourceCreateImageAtIndex($0, 0, nil) }) == nil { frames.removeLast() }
    guard frames.count > 5 else { throw NSError(domain: "Insufficient native recording frames: \(name)", code: 1) }
    let actions = (try? JSONDecoder().decode([Action].self, from: Data(contentsOf: folder.appendingPathComponent("actions.json")))) ?? []
    let first = max(frames[0].ms, (actions.first?.before ?? frames[0].ms) - 700)
    let end = min(frames.last!.ms, (actions.last?.after ?? frames.last!.ms) + 700)
    let duration = (end - first) / 1000
    let fps = 15, width = 1440, height = 810
    let destination = output.appendingPathComponent(name + "-native.mp4")
    try? FileManager.default.removeItem(at: destination)
    let writer = try AVAssetWriter(outputURL: destination, fileType: .mp4)
    let track = AVAssetWriterInput(mediaType: .video, outputSettings: [AVVideoCodecKey: AVVideoCodecType.h264, AVVideoWidthKey: width, AVVideoHeightKey: height, AVVideoCompressionPropertiesKey: [AVVideoAverageBitRateKey: 2600000, AVVideoProfileLevelKey: AVVideoProfileLevelH264HighAutoLevel]])
    track.expectsMediaDataInRealTime = false
    let adapter = AVAssetWriterInputPixelBufferAdaptor(assetWriterInput: track, sourcePixelBufferAttributes: [kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32ARGB, kCVPixelBufferWidthKey as String: width, kCVPixelBufferHeightKey as String: height, kCVPixelBufferCGImageCompatibilityKey as String: true, kCVPixelBufferCGBitmapContextCompatibilityKey as String: true])
    writer.add(track)
    guard writer.startWriting() else { throw writer.error! }
    writer.startSession(atSourceTime: .zero)
    var sourceIndex = 0, cachedIndex = -1
    var image: CGImage?
    for frame in 0..<Int(duration * Double(fps)) {
        while !track.isReadyForMoreMediaData { Thread.sleep(forTimeInterval: 0.002) }
        try autoreleasepool {
            let moment = first + Double(frame) / Double(fps) * 1000
            while sourceIndex + 1 < frames.count && frames[sourceIndex + 1].ms <= moment { sourceIndex += 1 }
            if cachedIndex != sourceIndex {
                if let source = CGImageSourceCreateWithURL(frames[sourceIndex].url as CFURL, nil), let next = CGImageSourceCreateImageAtIndex(source, 0, nil) { image = next; cachedIndex = sourceIndex }
            }
            guard let image else { throw NSError(domain: "Cannot decode recorded frame", code: 2) }
            var buffer: CVPixelBuffer?
            guard CVPixelBufferPoolCreatePixelBuffer(nil, adapter.pixelBufferPool!, &buffer) == kCVReturnSuccess, let buffer else { throw NSError(domain: "Pixel buffer", code: 3) }
            CVPixelBufferLockBaseAddress(buffer, [])
            let context = CGContext(data: CVPixelBufferGetBaseAddress(buffer), width: width, height: height, bitsPerComponent: 8, bytesPerRow: CVPixelBufferGetBytesPerRow(buffer), space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.noneSkipFirst.rawValue)!
            context.interpolationQuality = .high
            context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
            CVPixelBufferUnlockBaseAddress(buffer, [])
            if !adapter.append(buffer, withPresentationTime: CMTime(value: Int64(frame), timescale: Int32(fps))) { throw writer.error ?? NSError(domain: "Append", code: 4) }
        }
    }
    track.markAsFinished()
    let done = DispatchSemaphore(value: 0)
    writer.finishWriting { done.signal() }
    done.wait()
    guard writer.status == .completed else { throw writer.error! }
    let markers = (descriptions[name] ?? []).compactMap { key, label -> (String, Double)? in
        guard let action = actions.first(where: { $0.label == key }) else { return nil }
        return (label, max(0, (action.before - first) / 1000))
    }
    let chapters = markers.enumerated().map { index, marker in Chapter(label: marker.0, start: index == 0 ? 0 : marker.1, end: index + 1 < markers.count ? markers[index + 1].1 : duration) }
    manifest[name] = Recording(duration: duration, frames: frames.count, kind: "native-window-recording", chapters: chapters)
    let captions = "WEBVTT\n\n" + chapters.enumerated().map { index, chapter in "\(index+1)\n\(vttTime(chapter.start)) --> \(vttTime(chapter.end))\n\(chapter.label) in the native Dragent interface. Sample project.\n" }.joined(separator: "\n")
    try captions.write(to: output.appendingPathComponent(name + "-native.vtt"), atomically: true, encoding: .utf8)
    print("Encoded \(name): \(String(format: "%.1f", duration))s, \(frames.count) captured native frames")
}
let encoder = JSONEncoder(); encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
try encoder.encode(manifest).write(to: output.appendingPathComponent("manifest.json"))
