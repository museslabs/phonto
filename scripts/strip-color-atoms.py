#!/usr/bin/env python3
"""
Remove `colr` (Color Information) atoms from a QuickTime/ISO-BMFF MOV file
and fix up the parent atom sizes.

Why: our HEVC Main10 transcode comes out with an explicit `nclc` colr atom
(BT.709 primaries/transfer/matrix). Apple's own aerials don't have these
tags exposed in their format-description extensions either, and Wallper's
working .mov also lacks them. We have a strong hypothesis that the
extension's playback path mishandles the explicit color tags across
display-sleep / wake.

Usage:
  scripts/strip-color-atoms.py <input.mov> [output.mov]

If no output is given, modifies the input file in-place (with a `.bak`
backup written alongside).
"""

import os
import struct
import sys
from typing import List, Tuple


# Container atoms that hold other atoms — we recurse into these. Anything
# else is a leaf and we just copy through.
CONTAINER_ATOMS = {
    b"moov", b"trak", b"mdia", b"minf", b"stbl",
    b"stsd", b"hvc1", b"hev1", b"avc1", b"udta",
    b"edts", b"dinf",
}


def read_atom_header(data: bytes, offset: int) -> Tuple[int, bytes, int]:
    """Return (size, type, header_size) at offset."""
    size, atype = struct.unpack(">I4s", data[offset:offset + 8])
    header = 8
    if size == 1:
        # 64-bit size
        size = struct.unpack(">Q", data[offset + 8:offset + 16])[0]
        header = 16
    elif size == 0:
        # extends to EOF
        size = len(data) - offset
    return size, atype, header


def strip_colr(data: bytes) -> Tuple[bytes, int]:
    """Walk the MOV atom tree and produce a new bytes object with all `colr`
    atoms removed and parent atom sizes fixed. Returns (new_data, removed).
    """
    removed = [0]

    def rebuild(slice_data: bytes, slice_off: int, slice_end: int, inside_container: bool) -> bytes:
        out = bytearray()
        i = slice_off
        while i < slice_end:
            size, atype, header_size = read_atom_header(slice_data, i)
            atom_end = i + size

            if inside_container and atype == b"colr":
                # Drop this atom entirely.
                removed[0] += size
                i = atom_end
                continue

            if atype in CONTAINER_ATOMS:
                # Recurse: rebuild the children, then re-emit a header with
                # the new total size.
                inner = rebuild(slice_data, i + header_size, atom_end, True)
                new_size = header_size + len(inner)
                if header_size == 8:
                    out.extend(struct.pack(">I4s", new_size, atype))
                else:
                    # 64-bit header — re-emit with size==1 + 64-bit size
                    out.extend(struct.pack(">I4sQ", 1, atype, new_size))
                out.extend(inner)
            else:
                # Leaf atom — copy verbatim.
                out.extend(slice_data[i:atom_end])
            i = atom_end
        return bytes(out)

    new_data = rebuild(data, 0, len(data), False)
    return new_data, removed[0]


def main() -> int:
    if len(sys.argv) not in (2, 3):
        print(f"usage: {sys.argv[0]} <input.mov> [output.mov]", file=sys.stderr)
        return 2

    in_path = sys.argv[1]
    out_path = sys.argv[2] if len(sys.argv) == 3 else in_path

    with open(in_path, "rb") as f:
        data = f.read()
    original = len(data)

    new_data, removed = strip_colr(data)

    if removed == 0:
        print(f"no `colr` atoms found in {in_path} — nothing to do")
        return 0

    if out_path == in_path:
        backup = in_path + ".bak"
        if not os.path.exists(backup):
            os.replace(in_path, backup)
            print(f"backed up original -> {backup}")
        else:
            os.remove(in_path)

    with open(out_path, "wb") as f:
        f.write(new_data)

    print(
        f"stripped colr atom(s): removed {removed} bytes "
        f"({original} -> {len(new_data)}) -> {out_path}"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
