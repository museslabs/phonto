// Caustics: the light that the ripples focus on the pond bed. The light is
// soft, so this pass draws it at half height. The image pass samples it,
// moved by the ripples like all things under the surface.

// Bright lines that move. They are the edges between cells. Slow waves bend
// the pattern, so it wobbles and drifts.
float caustics(vec2 p, float t) {
    vec2 g = p * 13.0;
    g += 0.45 * vec2(sin(g.y * 1.3 + t * 0.6) + sin(g.y * 0.6 - t * 0.4),
                     sin(g.x * 1.1 - t * 0.5) + sin(g.x * 0.7 + t * 0.35));
    g += t * vec2(0.05, 0.03);
    ivec2 base = ivec2(floor(g));
    float f1 = 9.0;
    float f2 = 9.0;
    for (int y = -1; y <= 1; y++) {
        for (int x = -1; x <= 1; x++) {
            ivec2 c = base + ivec2(x, y);
            vec2 d = vec2(c) + 0.15 + 0.7 * hash2(c) - g;
            float dist = dot(d, d);
            if (dist < f1) {
                f2 = f1;
                f1 = dist;
            } else if (dist < f2) {
                f2 = dist;
            }
        }
    }
    float line = 1.0 - smoothstep(0.0, 0.07, sqrt(f2) - sqrt(f1));
    // The brightness changes along the lines. The light comes in patches, not
    // as an even net.
    float flicker = 0.35 + 0.65 * smoothstep(-0.3, 1.0, sin(g.x * 0.9 + t * 0.8) * sin(g.y * 1.1 - t * 0.6));
    float patches = 0.5 + 0.5 * sin(p.x * 2.1 + t * 0.07) * sin(p.y * 2.7 - t * 0.05);
    return line * line * flicker * smoothstep(0.3, 0.95, patches);
}

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    fragColor = vec4(caustics(fragCoord / iResolution.y, iTime), 0.0, 0.0, 1.0);
}
