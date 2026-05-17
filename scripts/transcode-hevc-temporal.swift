#!/usr/bin/swift
//
// Transcode a video to HEVC Main10 with 2 temporal sub-layers, mirroring
// Wallper.app's VTCompressionSession recipe. This is what unlocks
// multi-cycle lock-screen playback — AVAssetWriter doesn't expose the
// temporal-sub-layer knobs, so we drive VideoToolbox directly and feed
// the resulting NAL units into a `.mov` in pass-through mode.
//
// Property dict source: memory/project_wallper_encoder_config.md (lldb
// capture of Wallper, 2026-05-17).
//
// Usage: swift scripts/transcode-hevc-temporal.swift <input> <output.mov>

import AVFoundation
import CoreMedia
import Foundation
import VideoToolbox

guard CommandLine.arguments.count == 3 else {
    FileHandle.standardError.write("usage: transcode-hevc-temporal.swift <input> <output.mov>\n".data(using: .utf8)!)
    exit(2)
}
let srcURL = URL(fileURLWithPath: CommandLine.arguments[1])
let dstURL = URL(fileURLWithPath: CommandLine.arguments[2])
try? FileManager.default.removeItem(at: dstURL)
try? FileManager.default.createDirectory(at: dstURL.deletingLastPathComponent(), withIntermediateDirectories: true)

// Encoder output sink. The VT callback fires on its own thread; we drain
// after VTCompressionSessionCompleteFrames returns.
final class SampleSink {
    let lock = NSLock()
    var samples: [CMSampleBuffer] = []
    var firstError: OSStatus = noErr
}
let sink = SampleSink()

let outputCallback: VTCompressionOutputCallback = { refcon, _, status, _, sample in
    let sink = Unmanaged<SampleSink>.fromOpaque(refcon!).takeUnretainedValue()
    sink.lock.lock(); defer { sink.lock.unlock() }
    if status != noErr {
        if sink.firstError == noErr { sink.firstError = status }
        return
    }
    if let s = sample { sink.samples.append(s) }
}

struct TranscodeError: Error { let message: String }

let sem = DispatchSemaphore(value: 0)
var exitCode: Int32 = 0

Task {
    defer { sem.signal() }
    do {
        let asset = AVURLAsset(url: srcURL)
        guard let track = try await asset.loadTracks(withMediaType: .video).first else {
            throw TranscodeError(message: "no video track in input")
        }
        let size = try await track.load(.naturalSize)
        let width = Int32(size.width), height = Int32(size.height)
        print("source: \(width)x\(height)")

        // Reader emits 8-bit NV12. VT converts internally; output profile
        // is determined by ProfileLevel, not input pixel format.
        let reader = try AVAssetReader(asset: asset)
        let readerOutput = AVAssetReaderTrackOutput(track: track, outputSettings: [
            kCVPixelBufferPixelFormatTypeKey as String:
                kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
        ])
        reader.add(readerOutput)

        var sessionOut: VTCompressionSession?
        let createStatus = VTCompressionSessionCreate(
            allocator: nil,
            width: width, height: height,
            codecType: kCMVideoCodecType_HEVC,
            encoderSpecification: nil,
            imageBufferAttributes: nil,
            compressedDataAllocator: nil,
            outputCallback: outputCallback,
            refcon: Unmanaged.passUnretained(sink).toOpaque(),
            compressionSessionOut: &sessionOut)
        guard createStatus == noErr, let session = sessionOut else {
            throw TranscodeError(message: "VTCompressionSessionCreate failed: \(createStatus)")
        }

        func set(_ key: CFString, _ value: CFTypeRef, _ name: String) throws {
            let st = VTSessionSetProperty(session, key: key, value: value)
            if st != noErr { throw TranscodeError(message: "set \(name) failed: \(st)") }
        }

        // Properties in Wallper's exact order. Public constants where
        // they exist; raw CFStrings for the two temporal-sub-layer keys
        // that aren't in the SDK headers.
        try set(kVTCompressionPropertyKey_ProfileLevel, kVTProfileLevel_HEVC_Main10_AutoLevel, "ProfileLevel")
        try set(kVTCompressionPropertyKey_AverageBitRate, NSNumber(value: 9_500_000), "AverageBitRate")
        try set(kVTCompressionPropertyKey_DataRateLimits,
                [NSNumber(value: 1_500_000), NSNumber(value: 1)] as CFArray, "DataRateLimits")
        try set(kVTCompressionPropertyKey_ExpectedFrameRate, NSNumber(value: 60), "ExpectedFrameRate")
        try set(kVTCompressionPropertyKey_MaxKeyFrameInterval, NSNumber(value: 60), "MaxKeyFrameInterval")
        try set(kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration, NSNumber(value: 1.0), "MaxKeyFrameIntervalDuration")
        try set(kVTCompressionPropertyKey_RealTime, kCFBooleanFalse, "RealTime")
        try set(kVTCompressionPropertyKey_AllowFrameReordering, kCFBooleanFalse, "AllowFrameReordering")

        // Try the modern key first; fall back to the explicit pair on
        // -12900 (kVTPropertyNotSupportedErr). Wallper does the same
        // probe; both paths produce the same VPS.
        let tslaStatus = VTSessionSetProperty(session, key: "HEVCTemporalSubLayerAccess" as CFString, value: kCFBooleanTrue)
        if tslaStatus == noErr {
            print("temporal sub-layers: HEVCTemporalSubLayerAccess")
        } else {
            print("temporal sub-layers: NumberOfTemporalLayers + BaseLayerFrameRate (TSLA returned \(tslaStatus))")
            try set("NumberOfTemporalLayers" as CFString, NSNumber(value: 2), "NumberOfTemporalLayers")
            try set(kVTCompressionPropertyKey_BaseLayerFrameRate, NSNumber(value: 30.0), "BaseLayerFrameRate")
        }

        reader.startReading()
        var framesIn = 0
        while reader.status == .reading {
            guard let sample = readerOutput.copyNextSampleBuffer() else { break }
            guard let pb = CMSampleBufferGetImageBuffer(sample) else { continue }
            // Strip color attachments so the encoder writes no VUI color
            // description. Matches Wallper's .mov.
            CVBufferRemoveAttachment(pb, kCVImageBufferColorPrimariesKey)
            CVBufferRemoveAttachment(pb, kCVImageBufferTransferFunctionKey)
            CVBufferRemoveAttachment(pb, kCVImageBufferYCbCrMatrixKey)
            var info: VTEncodeInfoFlags = []
            let encStatus = VTCompressionSessionEncodeFrame(
                session,
                imageBuffer: pb,
                presentationTimeStamp: CMSampleBufferGetPresentationTimeStamp(sample),
                duration: CMSampleBufferGetDuration(sample),
                frameProperties: nil,
                sourceFrameRefcon: nil,
                infoFlagsOut: &info)
            if encStatus != noErr {
                throw TranscodeError(message: "EncodeFrame failed at frame \(framesIn): \(encStatus)")
            }
            framesIn += 1
            if framesIn % 30 == 0 {
                print("\r  encoding... \(framesIn) frames", terminator: "")
                fflush(stdout)
            }
        }
        if reader.status == .failed, let e = reader.error {
            throw TranscodeError(message: "reader failed: \(e)")
        }
        VTCompressionSessionCompleteFrames(session, untilPresentationTimeStamp: .invalid)
        VTCompressionSessionInvalidate(session)
        if sink.firstError != noErr {
            throw TranscodeError(message: "encoder error: \(sink.firstError)")
        }
        guard let firstSample = sink.samples.first,
              let formatDesc = CMSampleBufferGetFormatDescription(firstSample) else {
            throw TranscodeError(message: "no encoded samples produced")
        }
        print("\r  encoded \(framesIn) frames           ")

        // Writer in pass-through mode preserves the encoder's exact NAL
        // bytes — without this, AVAssetWriter would re-encode and we'd
        // lose the temporal-sub-layer signaling.
        let writer = try AVAssetWriter(url: dstURL, fileType: .mov)
        let writerInput = AVAssetWriterInput(mediaType: .video,
                                             outputSettings: nil,
                                             sourceFormatHint: formatDesc)
        writerInput.expectsMediaDataInRealTime = false
        writer.add(writerInput)
        guard writer.startWriting() else {
            throw TranscodeError(message: "startWriting failed: \(String(describing: writer.error))")
        }
        writer.startSession(atSourceTime: CMSampleBufferGetPresentationTimeStamp(firstSample))

        var idx = 0
        while idx < sink.samples.count {
            if writerInput.isReadyForMoreMediaData {
                if !writerInput.append(sink.samples[idx]) {
                    throw TranscodeError(message: "append failed at sample \(idx): \(String(describing: writer.error))")
                }
                idx += 1
            } else {
                try await Task.sleep(nanoseconds: 5_000_000)
            }
        }
        writerInput.markAsFinished()
        await writer.finishWriting()
        if writer.status == .failed, let e = writer.error {
            throw TranscodeError(message: "writer finalized with error: \(e)")
        }
        print("  wrote \(idx) samples -> \(dstURL.path)")
    } catch let e as TranscodeError {
        FileHandle.standardError.write("error: \(e.message)\n".data(using: .utf8)!)
        exitCode = 1
    } catch {
        FileHandle.standardError.write("error: \(error)\n".data(using: .utf8)!)
        exitCode = 1
    }
}

sem.wait()
exit(exitCode)
