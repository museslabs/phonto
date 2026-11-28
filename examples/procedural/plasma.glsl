// A procedural wallpaper in one file: `phonto examples/procedural/plasma.glsl`
//
// Slow bands of color. A glow follows the pointer while it is over the
// desktop.

vec3 palette(float t) {
    return 0.5 + 0.5 * cos(6.28318 * (t + vec3(0.0, 0.33, 0.67)));
}

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = (fragCoord - 0.5 * iResolution.xy) / iResolution.y;
    float t = iTime * 0.15;
    float v = sin(uv.x * 3.0 + t * 2.0)
            + sin(uv.y * 4.0 - t * 3.0)
            + sin((uv.x + uv.y) * 2.5 + t)
            + sin(length(uv * 3.0) - t * 4.0);
    vec3 col = palette(v * 0.25 + t) * 0.6;

    float d = length(fragCoord - iPointer.xy) / iResolution.y;
    col += iPointer.z * 0.35 * exp(-d * d * 60.0) * palette(t + 0.5);

    fragColor = vec4(col, 1.0);
}
