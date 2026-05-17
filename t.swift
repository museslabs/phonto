  import AVFoundation
  let asset = AVURLAsset(url: URL(fileURLWithPath: "$DIR/$LATEST"))
  let sem = DispatchSemaphore(value: 0)
  Task {
      let t = try? await asset.loadTracks(withMediaType: .video).first
      if let d = try? await t?.load(.formatDescriptions).first,
         let ext = CMFormatDescriptionGetExtensions(d) as? [String: Any],
         let bpc = ext["BitsPerComponent"] {
          print("bitsPerComponent = \(bpc)")
      }
      sem.signal()
  }
  sem.wait()
