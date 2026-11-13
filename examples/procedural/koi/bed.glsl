// The pond bed: silt, sand, and stones. It does not change, so the manifest
// draws this pass once. The image pass samples it, moved by the ripples.
// Alpha holds how close each point is to the banks.

// How close to the banks: 0 in the deep middle, 1 at the edges.
float bankOf(vec2 uv) {
    return smoothstep(0.28, 0.72, length((uv - 0.5) * vec2(1.0, 1.2)));
}

// Round stones, mostly near the banks.
vec3 stones(vec2 p, vec3 col, float bank) {
    vec2 g = p * 12.0;
    // The nearest point is in the 2x2 cells around the corner of the pixel.
    ivec2 base = ivec2(floor(g - 0.5));
    float best = 9.0;
    ivec2 bestCell = base;
    vec2 bestOffset = vec2(0.0);
    for (int y = 0; y <= 1; y++) {
        for (int x = 0; x <= 1; x++) {
            ivec2 c = base + ivec2(x, y);
            vec2 d = vec2(c) + 0.25 + 0.5 * hash2(c) - g;
            float dist = dot(d, d);
            if (dist < best) {
                best = dist;
                bestCell = c;
                bestOffset = d;
            }
        }
    }
    vec2 h = hash2(bestCell + ivec2(91, 17));
    if (h.x > bank * 0.55) {
        return col;
    }
    // Slightly oval, at a random angle.
    float a = h.y * TAU;
    vec2 o = mat2(cos(a), -sin(a), sin(a), cos(a)) * bestOffset * vec2(1.0, 1.35);
    float size = mix(0.14, 0.34, hashI(bestCell + ivec2(7, 3)));
    float r = length(o);
    float stone = 1.0 - smoothstep(size * 0.82, size, r);
    vec3 tint = mix(vec3(0.27, 0.28, 0.24), vec3(0.42, 0.39, 0.32), hashI(bestCell + ivec2(3, 41)));
    // Soft light from the upper left. The stone is darker where it goes into
    // the silt.
    float shade = 0.9 + 0.3 * dot(-bestOffset / size, vec2(-0.6, 0.8));
    shade *= mix(1.0, 0.75, smoothstep(size * 0.45, size, r));
    return mix(col, tint * shade, stone * 0.8);
}

vec3 pondBed(vec2 p, vec2 uv) {
    float bank = bankOf(uv);
    // Two octaves. The second is rotated, so the noise grid does not show.
    float silt = noise(p * 3.5) * 0.65 + noise(mat2(0.8, -0.6, 0.6, 0.8) * p * 10.0 + 3.7) * 0.35;
    vec3 deep = vec3(0.07, 0.21, 0.19);
    vec3 shallow = vec3(0.27, 0.36, 0.27);
    vec3 col = mix(deep, shallow, clamp(bank * 0.75 + (silt - 0.5) * 0.45, 0.0, 1.0));
    // Pale patches of sand.
    col = mix(col, vec3(0.36, 0.38, 0.29), smoothstep(0.62, 0.8, silt) * 0.45);
    // Fine grain of sand.
    col *= 0.96 + 0.08 * hashI(ivec2(floor(p * 420.0)));
    return stones(p, col, bank);
}

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    vec2 p = fragCoord / iResolution.y;
    fragColor = vec4(pondBed(p, uv), bankOf(uv));
}
