#!/usr/bin/env python3
"""
Force `general_level_idc` in an HEVC `.mov` from 153 (Level 5.1) to 150
(Level 5.0). Patches it in three places:
  - the hvcC config record's scalar `general_level_idc` field
  - the VPS NAL embedded in hvcC (profile_tier_level appears once)
  - the SPS NAL embedded in hvcC (same profile_tier_level signaling)

Rationale: empirically, Wallper's working .mov has `level_idc = 150` while
our AVAssetWriter output (HEVC_Main10_AutoLevel) chose 153. Both should
decode our 4K@25fps content fine, but the aerials extension's playback
state apparently behaves differently between the two levels on macOS 26.

In-place by default; pass an explicit output path as the 2nd arg to write
elsewhere. A `.bak` is left next to the input on in-place writes.

Usage:
  scripts/patch-level-idc.py <input.mov> [output.mov]
"""

import os
import struct
import sys


CONTAINER = {b"moov", b"trak", b"mdia", b"minf", b"stbl", b"stsd",
             b"hvc1", b"hev1"}


def walk(data, off, end):
    while off < end:
        size, atype = struct.unpack(">I4s", data[off:off + 8])
        hdr = 8
        if size == 1:
            size = struct.unpack(">Q", data[off + 8:off + 16])[0]
            hdr = 16
        elif size == 0:
            size = end - off
        body_start = off + hdr
        body_end = off + size
        yield atype, body_start, body_end
        if atype in CONTAINER:
            child_start = body_start + 8 if atype == b"stsd" else body_start
            if atype in (b"hvc1", b"hev1"):
                child_start = body_start + 78
            yield from walk(data, child_start, body_end)
        off = body_end


def patch_hvcc(mut, hvcc_start, hvcc_end, old_level=153, new_level=150):
    """Patch the hvcC body in-place.

    Layout (ISO/IEC 14496-15 §8.3.3.1):
      0  configurationVersion           1
      1  profile_space/tier_flag/idc    1
      2  general_profile_compat_flags   4
      6  general_constraint_indicator   6
      12 general_level_idc              1   ← target #1
      13 ...
      22 numOfArrays                    1
      23 arrays[]: per-type header + NALs

    Then we scan the NAL bytes for the VPS/SPS and patch level_idc in
    profile_tier_level() too. The level_idc byte appears at a known offset
    inside each NAL — easy to find without a full bitstream parser because
    both NALs start with a fixed header followed by the same 12-byte
    profile_tier_level prefix.
    """
    # 1. Scalar level_idc in hvcC body.
    pos = hvcc_start + 12
    if mut[pos] != old_level:
        print(f"  hvcC scalar level_idc = {mut[pos]} (expected {old_level}) — skipping scalar patch")
    else:
        mut[pos] = new_level
        print(f"  patched hvcC scalar: {old_level} -> {new_level}")

    # 2. Walk the NAL arrays after the hvcC fixed header (23 bytes).
    arr_off = hvcc_start + 22
    num_arrays = mut[arr_off]
    o = arr_off + 1
    patched_in_nals = 0
    for _ in range(num_arrays):
        nal_type = mut[o] & 0x3f
        o += 1
        num_nalus = struct.unpack(">H", bytes(mut[o:o + 2]))[0]
        o += 2
        for _ in range(num_nalus):
            l = struct.unpack(">H", bytes(mut[o:o + 2]))[0]
            o += 2
            nal_start = o
            nal_end = o + l
            o = nal_end

            if nal_type not in (32, 33):  # 32=VPS, 33=SPS
                continue

            # NAL header is 2 bytes. profile_tier_level() then follows
            # immediately after a small fixed-size prefix that's the same
            # for VPS and SPS at this profile/tier configuration:
            #   - VPS body: 4 bits vps_video_parameter_set_id +
            #     2 bits reserved + ... up to ptl
            #   - SPS body: 4 bits vps_id + 3 bits sub_layers + 1 bit
            #     temporal nesting + ... up to ptl
            # general_level_idc is at byte offset 12 inside ptl, where ptl
            # starts at byte 4 of the VPS body (after the 2-byte NAL header
            # the encoder gives us 'ffff' filler then ptl) and byte 3 of SPS
            # body. We've inspected the actual NAL bytes from Wallper vs
            # ours; the level_idc byte sits at the same fixed offset within
            # each NAL in practice, so we just scan for `old_level` after a
            # specific known prefix.
            #
            # Specifically: the 12-byte ptl-prefix is the same across both
            # VPS and SPS — bytes 0–10 are profile/constraint bits, byte 11
            # is general_level_idc.
            nal = mut[nal_start:nal_end]
            # Find ptl prefix `2000000300b0000003000003 00 ??` — the `??`
            # right after `0000030000030000` is level_idc.
            anchor = b"\x20\x00\x00\x03\x00\xb0\x00\x00\x03\x00\x00\x03\x00"
            idx = nal.find(anchor)
            if idx < 0:
                print(f"  NAL type {nal_type}: anchor not found, skipping")
                continue
            level_byte_idx = idx + len(anchor)  # position in nal[]
            if nal[level_byte_idx] != old_level:
                print(f"  NAL type {nal_type}: level_idc = {nal[level_byte_idx]} "
                      f"(expected {old_level}), skipping")
                continue
            mut[nal_start + level_byte_idx] = new_level
            patched_in_nals += 1
            print(f"  patched NAL type {nal_type}: level_idc {old_level} -> {new_level}")

    return patched_in_nals


def main():
    if len(sys.argv) not in (2, 3):
        print(f"usage: {sys.argv[0]} <input.mov> [output.mov]", file=sys.stderr)
        return 2

    in_path = sys.argv[1]
    out_path = sys.argv[2] if len(sys.argv) == 3 else in_path

    with open(in_path, "rb") as f:
        data = bytearray(f.read())

    found = False
    for atype, s, e in walk(data, 0, len(data)):
        if atype != b"hvcC":
            continue
        found = True
        print(f"hvcC at {s}-{e} ({e - s} bytes)")
        patch_hvcc(data, s, e)

    if not found:
        print(f"no hvcC atom in {in_path}", file=sys.stderr)
        return 1

    if out_path == in_path:
        backup = in_path + ".bak"
        if not os.path.exists(backup):
            with open(backup, "wb") as f:
                f.write(open(in_path, "rb").read())
            print(f"backed up original -> {backup}")

    with open(out_path, "wb") as f:
        f.write(bytes(data))
    print(f"wrote patched mov -> {out_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
