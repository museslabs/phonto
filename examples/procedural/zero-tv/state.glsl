// The menu state, 13x1 texels (see common.glsl). iChannel0 is this buffer.
//
// The tape under the pointer becomes the selected tape, and it stays
// selected after the pointer goes. A click selects a tape and sends it to
// the VCR.

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    int texel = int(fragCoord.x);
    vec4 state = iFrame == 0 ? START_STATE : texelFetch(iChannel0, ivec2(0), 0);
#if DEMO_TAPE > 0
    state = vec4(float(DEMO_TAPE), -100.0, float(DEMO_TAPE), DEMO_CLICK < 0.0 ? -100.0 : DEMO_CLICK);
#else
    if (iPointer.z > 0.5 && !isBusy(state)) {
        int tape = hitTape(iChannel0, toScene(iPointer.xy, iImageResolution.xy));
        if (tape > 0 && tape != int(state.x + 0.5)) {
            state.xy = vec2(float(tape), iTime);
        }
    }
    // A short click can end before this frame, and then iMouse.z is
    // negative. abs() gives the place of the click in both cases.
    if (iMouse.w > 0.0 && !isBusy(state)) {
        int tape = hitTape(iChannel0, toScene(abs(iMouse.zw), iImageResolution.xy));
        if (tape > 0) {
            state.xy = vec2(float(tape), iTime);
            state.zw = vec2(float(tape), iTime);
        }
    }
#endif
    if (texel == 0) {
        fragColor = state;
        return;
    }
    // The selected tape comes out fast, and the others go back.
    // pullOffset() rounds the place of a tape to whole art pixels.
    float amount = iFrame == 0 ? (texel == 1 ? 1.0 : 0.0) : texelFetch(iChannel0, ivec2(texel, 0), 0).x;
    float target = texel == int(state.x + 0.5) ? 1.0 : 0.0;
    amount = mix(target, amount, exp(-iTimeDelta * 28.0));
#if DEMO_TAPE > 0
    amount = target * DEMO_OUT;
#endif
    fragColor = vec4(amount, 0.0, 0.0, 0.0);
}
