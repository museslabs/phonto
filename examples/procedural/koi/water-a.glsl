// Water surface, the first of two simulation steps for each frame. This step
// also adds the disturbances of the frame: clicks, drags with the button
// pressed, and koi near the surface. Each texel holds (height now, height one
// step ago).

float gauss(vec2 d, float r) {
    return exp(-dot(d, d) / (r * r));
}

float segmentDistance(vec2 p, vec2 a, vec2 b) {
    vec2 pa = p - a;
    vec2 ba = b - a;
    float h = clamp(dot(pa, ba) / max(dot(ba, ba), 1e-8), 0.0, 1.0);
    return length(pa - ba * h);
}

// True on the one frame where `rate * time` passes a whole number.
bool onBeat(float rate, float offset) {
    return floor((iTime + offset) * rate) != floor((iTime - iTimeDelta + offset) * rate);
}

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    if (iFrame == 0) {
        fragColor = vec4(0.0);
        return;
    }
    vec2 h = waterStep(fragCoord);
    // This grid has square texels and the aspect ratio of the image. Dividing
    // by its height gives pond units.
    vec2 p = fragCoord / iResolution.y;
    float push = 0.0;

    // A click: food lands on the water. One small drop, and a few crumbs next
    // to it.
    if (iMouse.w > 0.0) {
        vec2 c = abs(iMouse.zw) / iImageResolution.y;
        push -= 0.6 * gauss(p - c, 0.008);
        for (int k = 0; k < 3; k++) {
            float fk = float(k);
            float a = hash1(c.x * 37.0 + fk) * TAU;
            float r = 0.015 + 0.025 * hash1(c.y * 53.0 + fk);
            push -= (0.08 + 0.1 * hash1(c.y + fk * 9.0)) * gauss(p - c - vec2(cos(a), sin(a)) * r, 0.004);
        }
    }

    // A drag with the button pressed pulls a wake through the water. A faster
    // drag makes a stronger wake.
    vec4 drag = texelFetch(iChannel0, ivec2(CONTROL, 1), 0);
    if (drag.x >= 0.0 && drag.z >= 0.0) {
        float moved = length(drag.zw - drag.xy);
        float strength = clamp(moved / max(iTimeDelta, 1e-3) * 0.08, 0.0, 0.07);
        float d = segmentDistance(p, drag.xy, drag.zw);
        // The wake is wider than a few water texels. A fast drag then leaves a
        // smooth wake, not one mark for each frame.
        push -= strength * exp(-d * d / (0.009 * 0.009));
    }

    // Koi near the surface push water to the side, and sometimes gulp at it.
    for (int i = 0; i < NUM_FISH; i++) {
        Fish f = loadFish(i);
        float shallow = 1.0 - smoothstep(0.05, 0.3, f.depth);
        if (shallow <= 0.0) {
            continue;
        }
        Koi k = koiOf(i);
        vec2 fwd = vec2(cos(f.heading), sin(f.heading));
        push -= shallow * f.speed * 0.12 * gauss(p - (f.pos - fwd * k.len * 0.3), k.len * 0.14);
        // About one gulp in five seconds for each koi at the surface.
        float gulpRate = 0.4;
        float beat = floor((iTime + k.seed * 5.0) * gulpRate);
        if (f.depth < 0.08 && onBeat(gulpRate, k.seed * 5.0) && hash1(beat + k.seed) > 0.5) {
            push -= 0.2 * gauss(p - f.pos, 0.005);
        }
    }

    // Something lands on the water every few seconds.
    if (onBeat(1.0 / 7.0, 0.0)) {
        float beat = floor(iTime / 7.0);
        vec2 at = vec2(hash1(beat * 3.1 + 0.5), hash1(beat * 7.7 + 0.25)) * vec2(iResolution.x / iResolution.y, 1.0);
        push -= 0.3 * gauss(p - at, 0.006);
    }

    fragColor = vec4(h.x + push, h.y, 0.0, 0.0);
}
