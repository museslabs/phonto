"""
lldb tracer for VideoToolbox VTCompressionSession calls in Wallper.

Hooks VTCompressionSessionCreate / VTSessionSetProperty / VTSessionSetProperties
and prints args (codec, dimensions, encoder spec dict, property key/value) by
invoking po (CFCopyDescription) on each CFTypeRef. Returns False from every
callback so the process auto-continues — no manual `c` needed.
"""

import lldb


def _po(debugger, addr):
    if not addr:
        return "<null>"
    ci = debugger.GetCommandInterpreter()
    res = lldb.SBCommandReturnObject()
    ci.HandleCommand("po (id){:#x}".format(addr), res)
    out = res.GetOutput() or ""
    return out.strip() or res.GetError().strip() or "<no description>"


def vt_create(frame, bp_loc, internal_dict):
    debugger = frame.GetThread().GetProcess().GetTarget().GetDebugger()
    width = frame.FindRegister("x1").GetValueAsSigned()
    height = frame.FindRegister("x2").GetValueAsSigned()
    codec = frame.FindRegister("x3").GetValueAsUnsigned() & 0xFFFFFFFF
    enc_spec = frame.FindRegister("x4").GetValueAsUnsigned()
    src_attrs = frame.FindRegister("x5").GetValueAsUnsigned()

    fourcc = bytes([
        (codec >> 24) & 0xFF,
        (codec >> 16) & 0xFF,
        (codec >> 8) & 0xFF,
        codec & 0xFF,
    ]).decode("ascii", errors="replace")

    print("\n=== VTCompressionSessionCreate ===")
    print("  width  = {}".format(width))
    print("  height = {}".format(height))
    print("  codec  = 0x{:08x} ('{}')".format(codec, fourcc))
    print("  encoderSpecification = {}".format(_po(debugger, enc_spec)))
    print("  sourceImageBufferAttributes = {}".format(_po(debugger, src_attrs)))

    ci = debugger.GetCommandInterpreter()
    res = lldb.SBCommandReturnObject()
    ci.HandleCommand("breakpoint enable 2", res)
    ci.HandleCommand("breakpoint enable 3", res)
    print("  [enabled VTSessionSetProperty / VTSessionSetProperties tracing]")
    return False


def vt_set_property(frame, bp_loc, internal_dict):
    debugger = frame.GetThread().GetProcess().GetTarget().GetDebugger()
    key = frame.FindRegister("x1").GetValueAsUnsigned()
    val = frame.FindRegister("x2").GetValueAsUnsigned()
    print("[VTSessionSetProperty] {} = {}".format(_po(debugger, key), _po(debugger, val)))
    return False


def vt_set_properties(frame, bp_loc, internal_dict):
    debugger = frame.GetThread().GetProcess().GetTarget().GetDebugger()
    props = frame.FindRegister("x1").GetValueAsUnsigned()
    print("\n=== VTSessionSetProperties ===")
    print(_po(debugger, props))
    return False


def __lldb_init_module(debugger, internal_dict):
    pass
