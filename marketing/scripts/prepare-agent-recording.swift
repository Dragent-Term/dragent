// Crop a real screen recording to the full Dragent window, preserving playback.
// No app is launched. Arguments: input.mp4 output.mp4 [start=12] [duration=43].
import Foundation
import AVFoundation
import AppKit
import QuartzCore
import CoreText
let source=URL(fileURLWithPath:CommandLine.arguments[1])
let destination=URL(fileURLWithPath:CommandLine.arguments[2])
let start=Double(CommandLine.arguments.count>3 ? CommandLine.arguments[3] : "12")!
let duration=Double(CommandLine.arguments.count>4 ? CommandLine.arguments[4] : "43")!
let asset=AVURLAsset(url:source)
let composition=AVMutableComposition()
let track=asset.tracks(withMediaType:.video).first!
let video=composition.addMutableTrack(withMediaType:.video,preferredTrackID:kCMPersistentTrackID_Invalid)!
try video.insertTimeRange(CMTimeRange(start:CMTime(seconds:start,preferredTimescale:600),duration:CMTime(seconds:duration,preferredTimescale:600)),of:track,at:.zero)
let layer=AVMutableVideoCompositionLayerInstruction(assetTrack:video)
// The original main display is 1710×1108; the exact app window is 0,34,1710,984.
let scale=1440.0/1710.0
layer.setCropRectangle(CGRect(x:0,y:34,width:1710,height:984),at:.zero)
layer.setTransform(CGAffineTransform(translationX:0,y:-34).concatenating(CGAffineTransform(scaleX:scale,y:scale)),at:.zero)
let instruction=AVMutableVideoCompositionInstruction()
instruction.timeRange=CMTimeRange(start:.zero,duration:video.timeRange.duration)
instruction.layerInstructions=[layer]
let render=AVMutableVideoComposition()
render.instructions=[instruction]
render.renderSize=CGSize(width:1440,height:828)
render.frameDuration=CMTime(value:1,timescale:30)
// Opaque editorial panel is baked into every frame, including downloads.
// It conceals the explorer rather than relying on a website-only overlay.
let stage=CALayer()
stage.frame=CGRect(x:0,y:0,width:1440,height:828)
let footage=CALayer()
footage.frame=stage.bounds
stage.addSublayer(footage)
let cover=CALayer()
cover.frame=CGRect(x:1210,y:0,width:230,height:828)
cover.backgroundColor=NSColor(srgbRed:0.055,green:0.075,blue:0.095,alpha:1).cgColor
stage.addSublayer(cover)
let rule=CALayer()
rule.frame=CGRect(x:0,y:0,width:1,height:828)
rule.backgroundColor=NSColor(srgbRed:0.19,green:0.24,blue:0.28,alpha:1).cgColor
cover.addSublayer(rule)
let panel=CGContext(data:nil,width:460,height:1656,bitsPerComponent:8,bytesPerRow:0,space:CGColorSpaceCreateDeviceRGB(),bitmapInfo:CGImageAlphaInfo.premultipliedLast.rawValue)!
panel.scaleBy(x:2,y:2)
func label(_ text:String,_ top:CGFloat,_ height:CGFloat,_ size:CGFloat,_ color:NSColor,_ bold:Bool=false) {
 let font=CTFontCreateWithName((bold ? "HelveticaNeue-Medium" : "HelveticaNeue") as CFString,size,nil)
 let string=NSAttributedString(string:text,attributes:[NSAttributedString.Key(kCTFontAttributeName as String):font,NSAttributedString.Key(kCTForegroundColorAttributeName as String):color.cgColor])
 let setter=CTFramesetterCreateWithAttributedString(string)
 let path=CGPath(rect:CGRect(x:24,y:828-top-height,width:184,height:height),transform:nil)
 let frame=CTFramesetterCreateFrame(setter,CFRange(location:0,length:0),path,nil)
 CTFrameDraw(frame,panel)
}
let cyan=NSColor(srgbRed:0.33,green:0.79,blue:0.94,alpha:1)
let white=NSColor(srgbRed:0.89,green:0.92,blue:0.94,alpha:1)
let muted=NSColor(srgbRed:0.58,green:0.64,blue:0.69,alpha:1)
label("DRAGENT",36,24,13,cyan,true)
label("Your agent.\nYour flow.",106,100,29,white,true)
label("From the first prompt to the final review.",226,70,16,muted)
let accent=CALayer()
accent.frame=CGRect(x:24,y:484,width:34,height:2)
accent.backgroundColor=cyan.cgColor
cover.addSublayer(accent)
label("Stay in context",382,26,16,white,true)
label("Agents, workspaces, and reviews. One place to work.",422,100,15,muted)
label("REAL SESSION",730,20,10,cyan,true)
label("Codex in Dragent",758,24,14,white)
let lettering=CALayer()
lettering.frame=cover.bounds
lettering.contents=panel.makeImage()!
cover.addSublayer(lettering)
render.animationTool=AVVideoCompositionCoreAnimationTool(postProcessingAsVideoLayer:footage,in:stage)

try? FileManager.default.removeItem(at:destination)
let export=AVAssetExportSession(asset:composition,presetName:AVAssetExportPreset1920x1080)!
export.outputURL=destination
export.outputFileType = .mp4
export.shouldOptimizeForNetworkUse=true
export.videoComposition=render
let done=DispatchSemaphore(value:0)
export.exportAsynchronously {done.signal()}
done.wait()
guard export.status == .completed else {throw export.error ?? NSError(domain:"Video export failed",code:1)}
let final=AVURLAsset(url:destination)
let generator=AVAssetImageGenerator(asset:final)
generator.appliesPreferredTrackTransform=true
let evidence=FileManager.default.temporaryDirectory.appendingPathComponent("dragent-video-qa")
try FileManager.default.createDirectory(at:evidence,withIntermediateDirectories:true)
for seconds in [2.0,23.0,40.0] {
 let frame=try generator.copyCGImage(at:CMTime(seconds:seconds,preferredTimescale:600),actualTime:nil)
 let data=NSBitmapImageRep(cgImage:frame).representation(using:.png,properties:[:])!
 try data.write(to:evidence.appendingPathComponent("codex-\(Int(seconds)).png"))
}
print("Exported real Codex session:",CMTimeGetSeconds(final.duration),"seconds")
