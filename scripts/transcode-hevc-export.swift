#!/usr/bin/swift
//
// Alternate transcoder: use AVAssetExportSession with HEVCHighestQuality.
// Different code path inside AVFoundation than AVAssetWriter — its
// bitstream shaping is set by Apple via the preset rather than knob-by-knob
// from us, so it may pick saner defaults (more frequent IDRs, the same
// HRD/VUI parameters Apple's own aerial pipeline produces) than what we've
// been able to coax out of AVAssetWriter.
//
// Usage:
//   swift scripts/transcode-hevc-export.swift <input> <output.mov>

import AVFoundation
import Foundation

guard CommandLine.arguments.count == 3 else {
    FileHandle.standardError.write("usage: transcode-hevc-export.swift <input> <output.mov>\n".data(using: .utf8)!)
    exit(2)
}
let src = URL(fileURLWithPath: CommandLine.arguments[1])
let dst = URL(fileURLWithPath: CommandLine.arguments[2])
try? FileManager.default.removeItem(at: dst)

let asset = AVURLAsset(url: src)
guard let session = AVAssetExportSession(asset: asset,
                                         presetName: AVAssetExportPresetHEVCHighestQuality) else {
    FileHandle.standardError.write("could not create export session\n".data(using: .utf8)!)
    exit(1)
}
session.outputURL = dst
session.outputFileType = .mov

let sem = DispatchSemaphore(value: 0)
session.exportAsynchronously {
    sem.signal()
}
// Progress while we wait
while session.status == .waiting || session.status == .exporting {
    print("\r  exporting... \(Int(session.progress * 100))%", terminator: "")
    fflush(stdout)
    Thread.sleep(forTimeInterval: 0.2)
}
sem.wait()
print()

switch session.status {
case .completed:
    print("done -> \(dst.lastPathComponent)")
case .failed:
    FileHandle.standardError.write("export failed: \(session.error?.localizedDescription ?? "unknown")\n".data(using: .utf8)!)
    exit(1)
case .cancelled:
    FileHandle.standardError.write("export cancelled\n".data(using: .utf8)!)
    exit(1)
default:
    FileHandle.standardError.write("export ended in unexpected status: \(session.status.rawValue)\n".data(using: .utf8)!)
    exit(1)
}
