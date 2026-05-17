#!/usr/bin/env python3
"""
Locate the hvcC (HEVC Configuration) box inside two MOV files and print a
side-by-side diff of their parameter sets (VPS, SPS, PPS).

The hvcC box layout (ISO/IEC 14496-15 §8.3.3.1) is:
  configurationVersion (1)
  general_profile_space + tier_flag + profile_idc (1)
  general_profile_compatibility_flags (4)
  general_constraint_indicator_flags (6)
  general_level_idc (1)
  min_spatial_segmentation_idc (2)
  parallelismType (1)
  chromaFormat (1)
  bitDepthLumaMinus8 (1)
  bitDepthChromaMinus8 (1)
  avgFrameRate (2)
  constantFrameRate + numTemporalLayers + temporalIdNested + lengthSizeMinusOne (1)
  numOfArrays (1)
  arrays[] each: nal_unit_type (1) + numNalus (2) + nalus[] each (length + bytes)

Usage:
  scripts/diff-hvcc.py <mov-a> <mov-b>
"""

import struct
import sys


CONTAINER = {b"moov", b"trak", b"mdia", b"minf", b"stbl", b"stsd",
             b"hvc1", b"hev1"}


def walk(data, off, end, path=()):
    """Yield (path, atom_type, body_start, body_end)."""
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
        yield path, atype, body_start, body_end
        if atype in CONTAINER:
            # stsd has a 8-byte header (version+flags+entry_count) before children
            child_start = body_start + 8 if atype == b"stsd" else body_start
            # hvc1/hev1 sample entry has a 78-byte SampleEntry/VisualSampleEntry preamble
            if atype in (b"hvc1", b"hev1"):
                child_start = body_start + 78
            yield from walk(data, child_start, body_end, path + (atype,))
        off = body_end


def find_hvcc(path):
    with open(path, "rb") as f:
        data = f.read()
    for p, t, s, e in walk(data, 0, len(data)):
        if t == b"hvcC":
            return data[s:e]
    return None


PARAM_TYPES = {
    32: "VPS", 33: "SPS", 34: "PPS",
}


def parse_hvcc(buf):
    """Return a dict of parsed-out hvcC fields + extracted NALs."""
    if buf is None or len(buf) < 23:
        return None
    o = 0
    cfg_ver = buf[o]; o += 1
    b = buf[o]; o += 1
    profile_space = (b >> 6) & 0x3
    tier_flag     = (b >> 5) & 0x1
    profile_idc   = b & 0x1f
    compat = buf[o:o + 4]; o += 4
    constraints = buf[o:o + 6]; o += 6
    level_idc = buf[o]; o += 1
    o += 2  # min_spatial_segmentation_idc
    o += 1  # parallelismType
    chroma_fmt = buf[o] & 0x3; o += 1
    bd_luma = (buf[o] & 0x7); o += 1
    bd_chroma = (buf[o] & 0x7); o += 1
    avg_fr = struct.unpack(">H", buf[o:o + 2])[0]; o += 2
    flags = buf[o]; o += 1
    constant_fr   = (flags >> 6) & 0x3
    num_temp      = (flags >> 3) & 0x7
    temp_nested   = (flags >> 2) & 0x1
    length_size   = (flags & 0x3) + 1
    num_arrays = buf[o]; o += 1
    arrays = []
    for _ in range(num_arrays):
        nal_type = buf[o] & 0x3f; o += 1
        num_nalus = struct.unpack(">H", buf[o:o + 2])[0]; o += 2
        nalus = []
        for _ in range(num_nalus):
            l = struct.unpack(">H", buf[o:o + 2])[0]; o += 2
            nalus.append(buf[o:o + l])
            o += l
        arrays.append((nal_type, nalus))
    return {
        "cfg_ver": cfg_ver,
        "profile_space": profile_space,
        "tier_flag": tier_flag,
        "profile_idc": profile_idc,
        "compat": compat.hex(),
        "constraints": constraints.hex(),
        "level_idc": level_idc,
        "chroma_fmt": chroma_fmt,
        "bit_depth_luma": 8 + bd_luma,
        "bit_depth_chroma": 8 + bd_chroma,
        "avg_frame_rate": avg_fr,
        "constant_frame_rate": constant_fr,
        "num_temporal_layers": num_temp,
        "temporal_id_nested": temp_nested,
        "length_size": length_size,
        "arrays": arrays,
    }


def dump(label, parsed):
    print(f"=== {label} ===")
    if parsed is None:
        print("  (no hvcC found)")
        return
    for k, v in parsed.items():
        if k == "arrays":
            continue
        print(f"  {k}: {v}")
    for nal_type, nalus in parsed["arrays"]:
        name = PARAM_TYPES.get(nal_type, f"type={nal_type}")
        for i, nal in enumerate(nalus):
            print(f"  {name}[{i}] ({len(nal)} bytes): {nal.hex()}")


def main():
    if len(sys.argv) != 3:
        print(f"usage: {sys.argv[0]} <mov-a> <mov-b>", file=sys.stderr)
        return 2
    a = parse_hvcc(find_hvcc(sys.argv[1]))
    b = parse_hvcc(find_hvcc(sys.argv[2]))
    dump(sys.argv[1], a)
    print()
    dump(sys.argv[2], b)
    print()
    print("=== quick diff of scalar fields ===")
    if a is None or b is None:
        print("  cannot diff (one side missing)")
        return 1
    for k in a:
        if k == "arrays":
            continue
        if a[k] != b[k]:
            print(f"  {k}: A={a[k]!r}  B={b[k]!r}")
    print("=== NAL-by-NAL byte diff ===")
    seen = {nt: ([], []) for nt in set(t for t, _ in a["arrays"]) | set(t for t, _ in b["arrays"])}
    for nt, ns in a["arrays"]:
        seen[nt][0].extend(ns)
    for nt, ns in b["arrays"]:
        seen[nt][1].extend(ns)
    for nt, (as_, bs_) in seen.items():
        name = PARAM_TYPES.get(nt, f"type={nt}")
        for i in range(max(len(as_), len(bs_))):
            x = as_[i] if i < len(as_) else b""
            y = bs_[i] if i < len(bs_) else b""
            if x == y:
                print(f"  {name}[{i}]: identical ({len(x)} bytes)")
            else:
                print(f"  {name}[{i}]: DIFFER  A({len(x)}b)={x.hex()}")
                print(f"  {name}[{i}]:         B({len(y)}b)={y.hex()}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
