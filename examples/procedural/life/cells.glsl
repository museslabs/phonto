float alive(vec2 coord) {
    return texture(iChannel0, coord / iResolution.xy).x;
}

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    if (iFrame == 0) {
        float a = step(0.8, hash(fragCoord));
        fragColor = vec4(a, a, 0.0, 0.0);
        return;
    }

    vec4 prev = texture(iChannel0, fragCoord / iResolution.xy);
    float n = 0.0;
    for (int y = -1; y <= 1; y++) {
        for (int x = -1; x <= 1; x++) {
            if (x != 0 || y != 0) {
                n += alive(fragCoord + vec2(x, y));
            }
        }
    }
    float next = (n == 3.0 || (prev.x > 0.5 && n == 2.0)) ? 1.0 : 0.0;

    // A few random births stop the board from going still.
    if (hash(fragCoord + fract(iTime * 7.13)) > 0.99997) {
        next = 1.0;
    }

    // Add cells around the pointer: a few while it hovers, many while the
    // left button is held.
    bool held = iPointer.w > 0.5;
    float radius = held ? 6.0 : 3.0;
    float density = held ? 0.3 : 0.85;
    if (iPointer.z > 0.5 && length(fragCoord - iPointer.xy) < radius
        && hash(fragCoord + iTime) > density) {
        next = 1.0;
    }

    float glow = max(next, prev.y * 0.93);
    fragColor = vec4(next, glow, 0.0, 0.0);
}
