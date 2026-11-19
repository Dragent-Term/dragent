// Lay a still of the current sidebar over a demo clip's sidebar, so a real
// recording shows today's design there. No app is launched.
// Arguments: input.mp4 overlay.png output.mp4 x y width height
// (the rectangle in the clip's own pixels, top-left origin).
import Foundation
import AVFoundation
import AppKit
import QuartzCore
let a=CommandLine.arguments
let source=URL(fileURLWithPath:a[1])
let image=NSImage(contentsOfFile:a[2])!
let destination=URL(fileURLWithPath:a[3])
let (x,y,w,h)=(Double(a[4])!,Double(a[5])!,Double(a[6])!,Double(a[7])!)
let asset=AVURLAsset(url:source)
let track=asset.tracks(withMediaType:.video).first!
let size=track.naturalSize
let composition=AVMutableComposition()
let video=composition.addMutableTrack(withMediaType:.video,preferredTrackID:kCMPersistentTrackID_Invalid)!
try video.insertTimeRange(CMTimeRange(start:.zero,duration:asset.duration),of:track,at:.zero)
let instruction=AVMutableVideoCompositionInstruction()
instruction.timeRange=CMTimeRange(start:.zero,duration:asset.duration)
instruction.layerInstructions=[AVMutableVideoCompositionLayerInstruction(assetTrack:video)]
let render=AVMutableVideoComposition()
render.instructions=[instruction]
render.renderSize=size
render.frameDuration=CMTime(value:1,timescale:30)
let stage=CALayer()
stage.frame=CGRect(origin:.zero,size:size)
let footage=CALayer()
footage.frame=stage.bounds
stage.addSublayer(footage)
let sidebar=CALayer()
// Core Animation's origin is bottom-left here.
sidebar.frame=CGRect(x:x,y:Double(size.height)-y-h,width:w,height:h)
sidebar.contents=image.cgImage(forProposedRect:nil,context:nil,hints:nil)
sidebar.contentsGravity = .resize
stage.addSublayer(sidebar)
render.animationTool=AVVideoCompositionCoreAnimationTool(postProcessingAsVideoLayer:footage,in:stage)
try? FileManager.default.removeItem(at:destination)
let export=AVAssetExportSession(asset:composition,presetName:AVAssetExportPresetHighestQuality)!
export.outputURL=destination
export.outputFileType = .mp4
export.shouldOptimizeForNetworkUse=true
export.videoComposition=render
let done=DispatchSemaphore(value:0)
export.exportAsynchronously {done.signal()}
done.wait()
guard export.status == .completed else {throw export.error ?? NSError(domain:"Video export failed",code:1)}
print("Exported",CMTimeGetSeconds(AVURLAsset(url:destination).duration),"seconds")
