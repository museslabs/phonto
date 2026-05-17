# Phonto — Lock-Screen Video Handoff

## Goal

Make a user-supplied video play on the macOS 26 (Tahoe) lock screen across
*multiple* lock/unlock cycles — including lid sleep → wake. Wallper.app
achieves this; we have not. The user wants to keep going. You are picking
this up cold; this doc has everything you need to skip the dead ends.

## What works today vs. what doesn't

| Surface | Status |
|---|---|
| Screen saver (`Phonto.saver`, hot corner / idle timer) | ✅ works, loops cleanly |
| Desktop wallpaper static image | ✅ trivial via `NSWorkspace.setDesktopImageURL` |
| **Animated lock-screen wallpaper** | ❌ this is the unsolved problem |

### The exact failure mode

With our transcoded `.mov` injected via the aerials catalog:

- The video plays on the **first** lock cycle.
- After the first cycle, the player decodes ~2.3 seconds of frames then stops.
- Subsequent locks render black. The aerials extension never re-engages the
  player on later lock cycles; only the *first* lock fires
  `Play Called → StartReadingItem → videodecoder` events.

With Wallper's `.mov` (`aerials/videos/ED68F689-…`) byte-substituted under
**our** injection entry, multi-cycle lock plays correctly. So:

> **The bug is in the bytes of the `.mov` file**, not in the injection
> wiring, not in entries.json shape, not in xattrs, not in Index.plist.
> This has been verified by direct substitution.

## The mystery

We have achieved scalar `hvcC` field parity with Wallper's working `.mov`:

```
profile_idc=2 (Main10)   level_idc=150 (5.0)   tier_flag=0 (Main tier)
constraints=b00000000000  compat=20000000
bit_depth_luma=10  bit_depth_chroma=10
chroma_fmt=1  temporal_id_nested=0  num_temporal_layers=1
```

Best result was via:

```
HandBrakeCLI \
  --input <src> --output <dst> \
  --format av_mp4 --encoder vt_h265_10bit --encoder-preset quality \
  --vfr --audio none
```

Yet even with every documented `hvcC` field matching Wallper's, the
file still fails the multi-lock test. The remaining diff is **inside
the NAL bodies themselves**: VPS body 24 vs 26 bytes, SPS body 37 vs 38
bytes, PPS body 8 vs 7 bytes. The HandBrake-VT output is ~2 bytes off
in each NAL.

```
Wallper VPS: 40010c02 ffff 02 2000000300b000000300000300960000170240   (26)
HandBrake VPS: 40010c01 ffff 02 2000000300b0000003000003009615c090       (24)

Wallper SPS: 420102022000000300b000000300000300960000a001e020021c4d8817b91655370202020080  (38)
HandBrake SPS: 420101022000000300b00000030000030096a001e020021c4d8815ee4595602d4040404020  (37)

Wallper PPS: 4401c02cbc14c9                                              (7)
HandBrake PPS: 4401c0287860a648                                          (8)
```

Note specifically:
- Wallper has byte 3 of VPS = `0x02` (vps_max_sub_layers_minus1 = 1, two
  temporal sub-layers). Ours = `0x01` (single layer). This was the
  hypothesis I had VTCompressionSession could fix but
  `HierarchicalFrameTuning` is rejected by AVAssetWriter for HEVC.
- The post-`ptl` bytes differ too — likely VUI/HRD optional fields, num
  reference frames, or other SPS-tail signaling.

## Run-time playback divergence (smoking gun)

`scripts/capture-lock-cycle-logs.sh` captures the os_log stream around lock
cycles. Diffing the working vs broken pass:

**Working** (Wallper's bytes):
- Each lock cycle fires `Play Called`, `StartReadingItem`,
  `videodecoder` XPC activate.
- PTS enqueue stream is continuous over the entire 35-second capture.
- Player resumes from the last-played timestamp across each cycle.

**Broken** (any of our transcodes):
- *Only* the first lock cycle fires `Play Called` / `StartReadingItem`.
- PTS enqueue stream stops cold after ~2.3 seconds of frames.
- Subsequent lock cycles produce no player-state log entries at all.
- `FigVideoQueueGMStats: 0 frames enqueued in the last 6 seconds`
  appears in idle periods.

So: the system decodes our first GOP, then the player enters a state
from which the extension never re-arms it on later locks. With Wallper's
file the extension keeps arming it. The 2-byte NAL diff is presumably the
cause.

## What we've ruled out (do NOT redo these)

1. **Entries.json shape** — `subcategories`, `includeInShuffle`,
   `pointsOfInterest`, custom Phonto category UUID. All match Wallper's
   shape now. `wallpaper-injector` and `scripts/inject-entries.py` produce
   the right JSON. Confirmed: Wallper's bytes under our injection entry
   plays multi-cycle.
2. **URL scheme** — `file://` works. Wallper uses it too.
3. **xattrs** — `LastETag` and `com.apple.quarantine` made it worse;
   `com.apple.provenance` (empty) is what Wallper has and what we should
   stay with. The current injector leaves it alone.
4. **HEVC 10-bit** — necessary, not sufficient. 8-bit fails immediately;
   10-bit gets us to the "first lock works" stage.
5. **Color tags** — our transcoder strips them via the pixel-buffer
   adaptor path. Wallper's `.mov` doesn't have them either. Inspector
   confirms parity.
6. **Container `colr` atom** — neither file has one. `scripts/strip-color-atoms.py`
   confirmed there's no atom-level color info to strip.
7. **Container atom hierarchy** — both have `ftyp / wide / mdat / moov`.
   Same brand `qt  `.
8. **MaxKeyFrameInterval = 24** in AVAssetWriter — didn't fix multi-cycle.
9. **Patching `level_idc` post-hoc** (`scripts/patch-level-idc.py`) —
   matches the byte but doesn't fix playback.
10. **Wallper-style separate `LockScreenCache` dir** — this is purely
    Wallper's internal re-encode cache. The system reads from
    `aerials/videos/<UUID>.mov` via `entries.json`, same as us.
11. **NSWorkspace overlay window at desktop level** — hidden by
    `loginwindow` during lock; lock-screen video via overlay is not
    reachable from a non-Apple-signed window.
12. **Writing our own wallpaper extension** — requires the
    `com.apple.private.wallpaper.extension` entitlement which is gated to
    Apple-signed binaries.

## Unexplored avenues (where to go next, ordered by likely payoff)

### 1. VTCompressionSession directly (highest payoff, biggest cost)

We've only driven the HEVC encoder through `AVAssetWriter` and `AVAssetExportSession`
abstractions. They reject `kVTCompressionPropertyKey_HierarchicalFrameTuning`,
`BaseLayerFrameRate`, and likely other VT-level keys we need.

Drop to `VTCompressionSession` directly:

```swift
var sessionOut: VTCompressionSession?
VTCompressionSessionCreate(
    allocator: nil,
    width: 3840, height: 2160,
    codecType: kCMVideoCodecType_HEVC,
    encoderSpecification: nil,
    imageBufferAttributes: nil,
    compressedDataAllocator: nil,
    outputCallback: callback,
    refcon: nil,
    compressionSessionOut: &sessionOut)

VTSessionSetProperty(session, key: kVTCompressionPropertyKey_HierarchicalFrameTuning,
                    value: kVTHierarchicalFrameTuning_AdaptiveTier as CFTypeRef)
VTSessionSetProperty(session, key: kVTCompressionPropertyKey_BaseLayerFrameRate,
                    value: 12.0 as CFNumber)
VTSessionSetProperty(session, key: kVTCompressionPropertyKey_ProfileLevel,
                    value: kVTProfileLevel_HEVC_Main10_AutoLevel)
// ... feed pixel buffers via VTCompressionSessionEncodeFrame ...
```

Then wrap the resulting NAL units in a MOV container via `AVAssetWriter`
in pass-through mode (`outputSettings: nil`) or write the container by
hand.

This is the *only* path that exposes the encoder properties Wallper is
almost certainly using to get `vps_max_sub_layers_minus1=1` and whatever
the SPS-tail differences encode.

Cost estimate: ~300 lines of careful Swift. Should be done in a focused
sitting, not iteratively. Build the prototype, encode the test video,
diff against Wallper. If the VPS now has 26 bytes and ends in
`0000170240`, you're on the right track.

### 2. Reverse-engineer Wallper's encoder calls directly

Wallper.app's main binary is at `/Applications/Wallper.app/Contents/MacOS/Wallper`.
We've grepped strings; `VTCompressionSession`, `HierarchicalFrameTuning`,
`BaseLayerFrameRate` references are present.

Run Wallper under `dtruss` or `fs_usage` while it transcodes a new video:

```bash
sudo dtruss -f -t open /Applications/Wallper.app/Contents/MacOS/Wallper 2>&1 | tee /tmp/wallper.dtruss
```

While that runs, in the Wallper UI: pick a new local video → set as
wallpaper. Watch what it touches and writes. The dtruss will show
exactly which encoder framework it loads and roughly when.

Better — attach with lldb and break on `VTSessionSetProperty`:

```bash
lldb -p $(pgrep Wallper)
(lldb) br set -n VTSessionSetProperty
(lldb) c
# now trigger a transcode in Wallper, each VTSessionSetProperty call
# will show you the property key and value being passed.
```

This will dump Wallper's exact property dict. Match it.

### 3. HEVC bitstream patching to insert sub-layer signaling

Even without re-encoding, you can rewrite the VPS/SPS NALs in our existing
`.mov` to declare `vps_max_sub_layers_minus1 = 1` and add the matching
sub-layer parameters. Match Wallper byte-for-byte.

This is ~bitstream surgery — flip the right bits in the VPS/SPS, recompute
RBSP emulation prevention bytes, recompute parent atom sizes.
Self-contained, no encoder dependency. Mid-effort (~200 lines of Python).

### 4. Re-run the lldb capture against the running aerials extension

`/System/Library/ExtensionKit/Extensions/WallpaperAerialsExtension.appex` —
when our broken `.mov` is loaded vs Wallper's, attach to the running
extension and see what diverges. Specifically: where does it bail out
after ~2.3 seconds of decoded frames? Where does it decide not to
re-arm on later locks?

```bash
lldb -p $(pgrep WallpaperAerialsExtension)
```

Without symbols this is hard but the existing logs we have show *what*
events fire (`Play Called`, `StartReadingItem`, decoder XPC activate);
lldb could show *why* the second lock doesn't fire them.

## Critical files and scripts

### What's on disk now

| Path | What it is |
|---|---|
| `~/Library/Application Support/com.apple.wallpaper/aerials/manifest/entries.json` | The aerial catalog. Currently injected with a `Phonto` category + a `17A6A998-…` asset, AND Wallper's `ED68F689-…` asset injected by Wallper itself. Backup at `entries.json.phonto-backup`. |
| `~/.../aerials/videos/17A6A998-….mov` | Our injected asset. Currently whatever the last transcode produced. |
| `~/.../aerials/videos/ED68F689-….mov` | **Wallper's working `.mov` — use as the gold reference for bitstream comparison.** |
| `~/Library/Application Support/Wallper/` | Wallper's own state (irrelevant to the playback path; just an internal cache). |
| `/Applications/Wallper.app` | Wallper binary — reverse-engineering target. |
| `/Applications/Wallspace.app` | Another working tool, similar mechanism. |

### Scripts in `scripts/` you'll want

| Script | Purpose |
|---|---|
| `transcode-hevc-main10.swift` | Our current AVAssetWriter+pixel-adaptor transcoder. Strips color attachments, sets MaxKeyFrameInterval=24. Best AVAssetWriter output we got. |
| `transcode-hevc-export.swift` | Alternate via AVAssetExportSession + HEVCHighestQuality preset. Produces 8-bit Main, not Main10. Probably skip. |
| `inspect-bitdepth.swift` | Reads a `.mov` and prints codec / bit depth / color tags / audio tracks. The "first-pass parity check". |
| `diff-hvcc.py` | Parses `hvcC` atom from two `.mov` files and prints scalar field diff + per-NAL byte diff. **This is your primary debugging tool**. |
| `inject-entries.py` | Stand-alone entries.json injector — bypasses the Rust injector when you want to test a `.mov` quickly. Use `phonto.<asset-uuid>` <display-name> form. |
| `strip-color-atoms.py` | MOV surgeon that removes `colr` atoms (turns out unnecessary; neither file has one). Kept for future MOV-container edits. |
| `patch-level-idc.py` | Post-encode patcher that flips `level_idc` 153→150 in hvcC + VPS + SPS NAL. Worked mechanically but didn't fix playback. Useful pattern for future bitstream patching. |
| `capture-lock-cycle-logs.sh` | os_log capture during a lock cycle. Filter for `WallpaperAerialsExtension`, `coremedia`, `videotoolbox`, etc. Crucial for "is the decoder even running?" diagnosis. |
| `probe-wallper-lockcache.sh` | Inspects Wallper's app-support state. Useful when comparing Wallper installs across versions. |
| `diff-wallper-vs-ours.sh` | High-level comparison: xattrs, mov header bytes, atoms, system state. Run this when you've changed something and want a quick sanity check. |
| `test-wallper-shape.sh` | Patches our entries.json to match Wallper's shape (includeInShuffle=true, pointsOfInterest filled). Once-only fix; future injector runs already produce this shape. |

### Rust injector (`wallpaper-injector/src/main.rs`)

Mid-rewrite. Currently has a `VTDecompressionSession`-fight legacy from a
prior pivot. The intended end-state:

1. Take a `.mov` as input (no transcoding inside Rust).
2. Copy to `aerials/videos/<UUID>.mov` (UUID = stable UUIDv5 of canonical
   source path).
3. Generate thumbnail via `qlmanage`.
4. Update `entries.json` (Phonto category, asset entry — shape is correct).
5. `killall WallpaperAerialsExtension Wallpaper WallpaperAgent`.

If you decide the transcoder belongs *outside* Phonto (which I now think it
does), the Rust CLI's job is just steps 1–5. Then the question is what
produces the input `.mov`. A few choices:

- Document "bring your own HEVC Main10 mov". HandBrake recipe in README.
- Bundle a wrapper script that shells out to `HandBrakeCLI` if present.
- Pull the file from Wallper's `LockScreenCache/` if Wallper is installed.

## How to verify a "candidate fix" works

The user is tired of false positives. Use this exact protocol:

```bash
# 1. Place your candidate .mov at our injected entry's path.
DST="$HOME/Library/Application Support/com.apple.wallpaper/aerials/videos/17A6A998-4049-5A53-A08D-FD553BE57044.mov"
cp /path/to/candidate.mov "$DST"

# 2. Strip any inherited xattrs.
xattr -c "$DST"

# 3. Kick the three daemons.
killall WallpaperAerialsExtension Wallpaper WallpaperAgent 2>/dev/null
sleep 1

# 4. Re-pick the Phonto wallpaper in System Settings → Wallpaper, manually.
#    (The choice config must reload; the daemon kick alone isn't enough.)

# 5. Confirm playback on first lock cycle (Apple menu → Lock Screen).

# 6. Unlock. Close lid (or Cmd+Ctrl+Q again). Reopen. Confirm second
#    cycle plays. Repeat 3+ times.

# 7. If steps 5-6 succeed, capture a log to confirm continuous player
#    activity, not just visual confirmation:
./scripts/capture-lock-cycle-logs.sh /tmp/candidate.log 35
grep 'enqueue PTS' /tmp/candidate.log | wc -l
# Expect: >>30 lines (continuous decoding throughout 35s).
# If <30, decoder stopped early — same bug we've been chasing.
```

## Important reference data

### Wallper's hvcC parameter sets (the target)

```
VPS[0] (26 bytes):
  40010c02ffff022000000300b000000300000300960000170240

SPS[0] (38 bytes):
  420102022000000300b000000300000300960000a001e020021c4d8817b91655370202020080

PPS[0] (7 bytes):
  4401c02cbc14c9
```

### Our HandBrake-VT hvcC parameter sets (closest miss)

```
VPS[0] (24 bytes):
  40010c01ffff022000000300b0000003000003009615c090

SPS[0] (37 bytes):
  420101022000000300b00000030000030096a001e020021c4d8815ee4595602d4040404020

PPS[0] (8 bytes):
  4401c0287860a648
```

### Bit-level interpretation of the VPS diff

```
Wallper byte 3:  0x02  → vps_max_sub_layers_minus1 = 1, temporal_id_nesting = 0
Ours byte 3:     0x01  → vps_max_sub_layers_minus1 = 0, temporal_id_nesting = 1
```

`vps_max_sub_layers_minus1 = 1` triggers extra sub-layer info encoding in
both VPS and SPS — explains the 2-byte / 3-byte / 1-byte size differences
across all three NALs. **This is the single most actionable target.**

## Constraints / user preferences

- **No Swift app extension.** User wants to keep it Rust-first. A small
  Swift helper script (like the transcoders) is acceptable.
- **No private entitlements.** No `com.apple.private.*` of any kind.
- **Don't ship private API hacks.** Public API surface only.
- **Don't run `cargo build` to "verify".** User drives their own build
  loop. Just write the code and let them run it.
- **No cosmetic edits.** Functional changes only.

## My read on the path most likely to land

Order I'd try:

1. **lldb on running Wallper while transcoding** to dump exact
   `VTSessionSetProperty` calls. This is the cheapest way to skip
   guesswork and land on Wallper's actual property dict in one shot.
2. **Implement those properties via direct `VTCompressionSession`** in a
   small Swift transcoder, write NALs into a mov via AVAssetWriter
   pass-through.
3. **Test against the protocol above.**

If lldb shows Wallper is using a property we *can't* set as a third party
(e.g. requires an entitlement), we're done and pivot to "pre-transcoded
mov" as input.

If lldb shows Wallper is using properties we *can* set, we can match them
and ship. My honest estimate: 4-8 hours of focused work once we have the
lldb capture.

Good luck. The bitstream diff is small enough that this is solvable.
