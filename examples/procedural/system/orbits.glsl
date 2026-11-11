// The planets of the orrery. Each frame reads the last one.
// Texel i: x = the angle of planet i, y = its load for each core, smoothed.

void mainImage(out vec4 state, in vec2 coord) {
    int i = int(coord.x);
    state = texelFetch(iChannel0, ivec2(i, 0), 0);
    float load = i == 0 ? iInput.load.x : i == 1 ? iInput.load.y : iInput.load.z;
    float perCore = clamp(load / max(iInput.cores, 1.0), 0.0, 2.0);
    // The commands give new values every few seconds. The planets ease to
    // them, and add up their angle, so they never jump.
    state.y += (perCore - state.y) * (1.0 - exp(-iTimeDelta / 2.0));
    float speed = (0.05 + 1.2 * state.y) * (1.0 - 0.25 * float(i));
    state.x = mod(state.x + iTimeDelta * speed, 6.2831853);
}
