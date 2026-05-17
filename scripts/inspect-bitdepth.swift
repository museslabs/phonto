#!/usr/bin/swift
//
// Diagnostic: print bit depth and a few other format-description bits for a
// video file. Used to verify our transcoder produced an HEVC Main10
// (`bitsPerComponent = 10`) `.mov`, the way Apple's own aerials are encoded.
//
// Usage:
//   swift scripts/inspect-bitdepth.swift <path-to-video>

import AVFoundation
import CoreMedia
import Foundation

guard CommandLine.arguments.count == 2 else {
    FileHandle.standardError.write("usage: inspect-bitdepth.swift <video>\n".data(using: .utf8)!)
    exit(2)
}
let path = CommandLine.arguments[1]

let asset = AVURLAsset(url: URL(fileURLWithPath: path))
let sem = DispatchSemaphore(value: 0)
var exitCode: Int32 = 0

Task {
    defer { sem.signal() }
    do {
        guard let track = try await asset.loadTracks(withMediaType: .video).first else {
            FileHandle.standardError.write("no video track\n".data(using: .utf8)!)
            exitCode = 1
            return
        }
        guard let desc = try await track.load(.formatDescriptions).first else {
            FileHandle.standardError.write("no format descriptions\n".data(using: .utf8)!)
            exitCode = 1
            return
        }

        let mediaSub = CMFormatDescriptionGetMediaSubType(desc)
        let codecChars = String(format: "%c%c%c%c",
            (mediaSub >> 24) & 0xff, (mediaSub >> 16) & 0xff,
            (mediaSub >>  8) & 0xff, (mediaSub      ) & 0xff)
        let dims = CMVideoFormatDescriptionGetDimensions(desc)

        print("file:              \(path)")
        print("codec:             \(codecChars)  (\(dims.width)x\(dims.height))")

        if let ext = CMFormatDescriptionGetExtensions(desc) as? [String: Any] {
            if let v = ext["BitsPerComponent"] { print("bitsPerComponent:  \(v)") }
            if let v = ext["Depth"]            { print("depth:             \(v)") }
            if let v = ext["CVImageBufferYCbCrMatrix"] { print("ycbcrMatrix:       \(v)") }
            if let v = ext["CVImageBufferColorPrimaries"] { print("colorPrimaries:    \(v)") }
            if let v = ext["CVImageBufferTransferFunction"] { print("transferFunction:  \(v)") }
        }

        let audioCount = try await asset.loadTracks(withMediaType: .audio).count
        print("audio tracks:      \(audioCount)")
    } catch {
        FileHandle.standardError.write("error: \(error)\n".data(using: .utf8)!)
        exitCode = 1
    }
}

sem.wait()
exit(exitCode)
