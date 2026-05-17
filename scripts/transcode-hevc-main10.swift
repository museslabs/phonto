#!/usr/bin/swift
//
// One-shot transcoder: input video → HEVC Main10 (10-bit profile) `.mov` via
// AVFoundation / VideoToolbox.
//
// Usage:
//   swift scripts/transcode-hevc-main10.swift <input> <output.mov>
//
// Why this exists, and not as part of the Rust binary yet:
// We're verifying the hypothesis that the aerials extension only survives
// across lid sleep/wake when its asset is HEVC Main10 (Wallper's claim,
// inferred from Apple's own aerials being `bitsPerComponent=10`). The Rust
// transcoder kept tripping foreign-exception aborts inside AVF on macOS 26
// and the cost of unblocking it was outpacing the cost of just verifying the
// hypothesis. This Swift one-shot drops in next to wallpaper-injector's
// output path so we can swap the 8-bit transcode for a 10-bit one and see
// whether the second-lock-black bug actually goes away. If it does, the
// transcoder gets reimplemented properly (Rust or shelled-out Swift, TBD).
// If it doesn't, we drop the 10-bit hypothesis entirely.

import AVFoundation
import CoreMedia
import Foundation

guard CommandLine.arguments.count == 3 else {
    FileHandle.standardError.write("usage: transcode-hevc-main10.swift <input> <output.mov>\n".data(using: .utf8)!)
    exit(2)
}
let srcPath = CommandLine.arguments[1]
let dstPath = CommandLine.arguments[2]
let src = URL(fileURLWithPath: srcPath)
let dst = URL(fileURLWithPath: dstPath)

try? FileManager.default.removeItem(at: dst)
try? FileManager.default.createDirectory(at: dst.deletingLastPathComponent(),
                                         withIntermediateDirectories: true)

let asset = AVURLAsset(url: src)
let sem = DispatchSemaphore(value: 0)
var exitCode: Int32 = 0

Task {
    defer { sem.signal() }
    do {
        // Async-loaded track + size — synchronous getters are deprecated on
        // macOS 15+ and can hand back proxies that fail later validation.
        guard let videoTrack = try await asset.loadTracks(withMediaType: .video).first else {
            FileHandle.standardError.write("no video track in input\n".data(using: .utf8)!)
            exitCode = 1
            return
        }
        let size = try await videoTrack.load(.naturalSize)

        let reader = try AVAssetReader(asset: asset)
        // Decoded 8-bit 4:2:0 video-range. We deliberately do NOT ask the
        // reader for 10-bit pixel buffers — typical inputs are 8-bit and the
        // reader throws when asked to widen the source. The 10-bitness comes
        // out at the writer's encoder.
        let readerOut = AVAssetReaderTrackOutput(track: videoTrack, outputSettings: [
            kCVPixelBufferPixelFormatTypeKey as String:
                kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
        ])
        guard reader.canAdd(readerOut) else {
            FileHandle.standardError.write("reader rejects 8-bit 4:2:0 output for this track\n".data(using: .utf8)!)
            exitCode = 1
            return
        }
        reader.add(readerOut)

        let writer = try AVAssetWriter(url: dst, fileType: .mov)
        // HEVC Main10 — the VideoToolbox encoder emits a 10-bit Main10
        // bitstream regardless of whether the input pixels were 8-bit.
        //
        // MaxKeyFrameInterval = 24 (one IDR per second at 24fps source) is
        // the critical knob: empirically, without it, AVAssetWriter chose a
        // very large GOP and the aerials extension's player would decode
        // through the first GOP (about 2.3s of frames) then stop cold —
        // every lock cycle after the first played to black because the
        // player couldn't seek to a usable IDR. Wallper's mov has frequent
        // keyframes so the player can always re-engage.
        let writerInput = AVAssetWriterInput(mediaType: .video, outputSettings: [
            AVVideoCodecKey: AVVideoCodecType.hevc,
            AVVideoWidthKey: size.width,
            AVVideoHeightKey: size.height,
            AVVideoCompressionPropertiesKey: [
                AVVideoProfileLevelKey: "HEVC_Main10_AutoLevel" as NSString,
                AVVideoMaxKeyFrameIntervalKey: 24,
                AVVideoMaxKeyFrameIntervalDurationKey: 1.0,
            ],
        ])
        writerInput.expectsMediaDataInRealTime = false

        // We feed the writer via a pixel-buffer adaptor, *not* by passing
        // CMSampleBuffers through directly. The reason: a sample buffer
        // carries a CMFormatDescription that already has the source's
        // `kCMFormatDescriptionExtension_YCbCrMatrix` / `_ColorPrimaries` /
        // `_TransferFunction` baked in. Whatever we strip off the
        // CVPixelBuffer, that format description still tells VideoToolbox
        // "encode this BT.709 stream with VUI colour_description_present=1",
        // and you get `ycbcrMatrix=ITU_R_709_2` on the output.
        //
        // The adaptor only consumes a CVPixelBuffer + presentation time,
        // not a sample buffer, so the format description is taken from the
        // writer's outputSettings (which deliberately omits
        // AVVideoColorPropertiesKey) plus any attachments we choose to
        // leave on the pixel buffer. We strip the color attachments before
        // appending, so the encoder has no source of color info and writes
        // no VUI color description — matching Wallper's working .mov.
        let adaptor = AVAssetWriterInputPixelBufferAdaptor(
            assetWriterInput: writerInput,
            sourcePixelBufferAttributes: [
                kCVPixelBufferPixelFormatTypeKey as String:
                    kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
                kCVPixelBufferWidthKey as String: size.width,
                kCVPixelBufferHeightKey as String: size.height,
            ]
        )

        guard writer.canAdd(writerInput) else {
            FileHandle.standardError.write("writer rejects HEVC Main10 input\n".data(using: .utf8)!)
            exitCode = 1
            return
        }
        writer.add(writerInput)

        reader.startReading()
        writer.startWriting()
        writer.startSession(atSourceTime: .zero)

        var frames = 0
        while true {
            while !writerInput.isReadyForMoreMediaData {
                try await Task.sleep(nanoseconds: 5_000_000)
            }
            if let sample = readerOut.copyNextSampleBuffer() {
                guard let pb = CMSampleBufferGetImageBuffer(sample) else { continue }
                CVBufferRemoveAttachment(pb, kCVImageBufferColorPrimariesKey)
                CVBufferRemoveAttachment(pb, kCVImageBufferTransferFunctionKey)
                CVBufferRemoveAttachment(pb, kCVImageBufferYCbCrMatrixKey)
                let pts = CMSampleBufferGetPresentationTimeStamp(sample)
                adaptor.append(pb, withPresentationTime: pts)
                frames += 1
                if frames % 30 == 0 {
                    print("\r  \(frames) frames", terminator: "")
                    fflush(stdout)
                }
            } else {
                if reader.status == .failed, let e = reader.error {
                    FileHandle.standardError.write("\nreader failed mid-stream: \(e)\n".data(using: .utf8)!)
                    exitCode = 1
                    return
                }
                writerInput.markAsFinished()
                await writer.finishWriting()
                print("\n  done: \(frames) frames -> \(dst.lastPathComponent)")
                if writer.status == .failed, let e = writer.error {
                    FileHandle.standardError.write("writer finalized with error: \(e)\n".data(using: .utf8)!)
                    exitCode = 1
                }
                return
            }
        }
    } catch {
        FileHandle.standardError.write("error: \(error)\n".data(using: .utf8)!)
        exitCode = 1
    }
}

sem.wait()
exit(exitCode)
