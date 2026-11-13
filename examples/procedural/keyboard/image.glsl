// Space toggles the colour. The strip at the bottom lights up the keys that
// are held, one cell for each JavaScript key code.
float key(int code, int row) { return texelFetch(iChannel1, ivec2(code, row), 0).x; }

void mainImage(out vec4 c, in vec2 p) {
    vec2 pos = texelFetch(iChannel0, ivec2(0), 0).xy;
    vec3 col = mix(vec3(0.05, 0.06, 0.09), vec3(0.10, 0.07, 0.14), p.y / iResolution.y);
    vec3 tint = mix(vec3(0.95, 0.55, 0.20), vec3(0.30, 0.80, 1.00), key(32, 2));
    float d = length(p - pos);
    col += tint * (smoothstep(40.0, 36.0, d) + 0.25 * exp(-d / 120.0));
    if (p.y < 14.0) col = mix(col, vec3(1.0), key(int(p.x / iResolution.x * 256.0), 0));
    c = vec4(col, 1.0);
}
