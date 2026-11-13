// Koi pond: everything on screen, seen from above. The pond bed with stones
// and caustics, the koi and their shadows, the ripples that bend all of it,
// and lily pads on top. The light follows the time of day.

bool hasBit(uint mask, int bit) {
    return ((mask >> uint(bit)) & 1u) != 0u;
}

// ---- koi --------------------------------------------------------------------

// Signed distance to the body. Also returns t along the body, s across it
// (-1 to 1), and the side distance from the spine.
float bodyDist(Pose P, vec2 local, SpinePoint sp, out float t, out float s, out float dv) {
    float u = local.x;
    dv = (local.y - sp.y) / sqrt(1.0 + sp.slope * sp.slope);
    t = u / P.len;
    float w = WIDTH * P.len * halfWidth(clamp(t, 0.0, 1.0));
    s = dv / max(w, 1e-5);
    return max(max(abs(dv) - w, -u), u - P.len);
}

// The forked tail fin behind the body. `ray` goes from -1 to 1 across it.
float tailDist(Pose P, vec2 local, SpinePoint sp, out float ray, out float along) {
    float L = P.len;
    float tt = (local.x - 0.93 * L) / (0.36 * L);
    // Measured across the bent spine, so the fin keeps its shape when the
    // tail swings.
    float dv = (local.y - sp.y) / sqrt(1.0 + sp.slope * sp.slope);
    float hw = L * (0.025 + 0.16 * pow(clamp(tt, 0.0, 1.0), 0.75));
    float ss = dv / hw;
    float edge = 1.0 - 0.3 * (1.0 - min(ss * ss, 1.0));
    ray = ss;
    along = tt;
    return max(abs(dv) - hw, max(-tt, tt - edge) * 0.36 * L);
}

// A paired fin: a round paddle at the edge of the body, swept back from the
// tail direction by an angle given as (cos, sin). `x` goes from 0 at the root
// to 1 at the tip.
float finDist(vec2 local, vec2 root, float side, vec2 sweep, float len, float wid,
              out float x, out float ray) {
    // `ray` is the tangent of the angle from the root, so the rays spread out
    // from the root.
    vec2 axis = vec2(sweep.x, side * sweep.y);
    vec2 d = local - root;
    float a = dot(d, axis);
    float b = dot(d, vec2(-axis.y, axis.x));
    x = clamp(a / len, 0.0, 1.0);
    float hw = wid * 0.8 * sqrt(x * max(1.0 - x * x, 0.0));
    ray = b / max(a, 1e-4);
    return max(abs(b) - hw, max(-a, a - len));
}

// Skin color at (t, s) on the body. `metal` is 1 for metallic varieties.
vec3 koiSkin(Koi k, float t, float s, out float metal) {
    vec3 white = vec3(0.96, 0.94, 0.88);
    vec3 red = vec3(0.86, 0.22, 0.09);
    vec3 sumi = vec3(0.05, 0.05, 0.06);
    metal = 0.0;
    int v = k.variety;
    if (v == 3) { // tancho: a single red circle on the head
        float r = length(vec2((t - 0.11) * 2.6, s * 0.75));
        return mix(white, red, 1.0 - smoothstep(0.17, 0.19, r));
    }
    if (v == 6) { // asagi: blue-gray back, orange flanks, pale head
        vec3 c = mix(vec3(0.52, 0.63, 0.72), vec3(0.94, 0.5, 0.22), smoothstep(0.6, 0.85, abs(s)));
        return mix(c, vec3(0.88, 0.9, 0.89), 1.0 - smoothstep(0.06, 0.2, t));
    }
    if (v == 7) { // chagoi: bronze
        return mix(vec3(0.68, 0.5, 0.32), vec3(0.52, 0.36, 0.22), abs(s));
    }
    if (v >= 4) {
        metal = 1.0;
        if (v == 4) return mix(vec3(0.99, 0.77, 0.24), vec3(0.85, 0.55, 0.12), abs(s) * 0.7);
        if (v == 5) return mix(vec3(0.95, 0.95, 0.92), vec3(0.74, 0.77, 0.78), abs(s) * 0.7);
        return mix(vec3(1.0, 0.55, 0.14), vec3(0.88, 0.36, 0.08), abs(s) * 0.7);
    }
    // Varieties with patches. The patches have sharp edges and stay off the
    // nose and the tail root.
    vec2 q = vec2(t * 4.2, s * 1.3) + k.seed * 23.0;
    float edge = 0.025;
    float body = smoothstep(0.04, 0.1, t) * (1.0 - smoothstep(0.86, 0.96, t));
    float hi = smoothstep(0.5 - edge, 0.5 + edge, noise(q) * 0.65 + noise(q * 2.3 + 5.0) * 0.35) * body;
    if (v == 0) { // kohaku
        return mix(white, red, hi);
    }
    float patchB = noise(q * 1.4 + 41.0) * 0.6 + noise(q * 3.1 + 9.0) * 0.4;
    if (v == 1) { // sanke: kohaku with small black spots
        float spots = smoothstep(0.66 - edge, 0.66 + edge, patchB) * smoothstep(0.25, 0.35, t);
        return mix(mix(white, red, hi), sumi, spots * body);
    }
    // showa: black with white and red
    return mix(mix(sumi, white, smoothstep(0.46 - edge, 0.46 + edge, patchB)), red, hi * 0.95);
}

vec3 finColour(Koi k, float x) {
    int v = k.variety;
    if (v == 2) { // showa: black at the fin root
        return mix(vec3(0.06), vec3(0.94, 0.93, 0.9), smoothstep(0.2, 0.55, x));
    }
    if (v == 4) return vec3(1.0, 0.8, 0.35);
    if (v == 5) return vec3(0.92, 0.93, 0.92);
    if (v == 6) return vec3(0.93, 0.55, 0.28);
    if (v == 7) return vec3(0.7, 0.55, 0.38);
    if (v == 8) return vec3(1.0, 0.6, 0.25);
    return vec3(0.96, 0.95, 0.92);
}

// Overlapping scales: 1 along the edge of each scale.
float scales(float u, float dv, float L) {
    vec2 g = vec2(u, dv) / (L * 0.05);
    g.y += 0.5 * mod(floor(g.x), 2.0);
    vec2 f = fract(g) - 0.5;
    return smoothstep(0.28, 0.5, length(f * vec2(1.25, 1.0)));
}

// Color and coverage of a koi at `local`. The fins and the tail are
// translucent.
vec4 drawKoi(Pose P, Koi k, vec2 local, SpinePoint sp, float px, vec3 ld, Light L, float caustic) {
    // A deeper koi looks softer, through more water.
    float aa = px * 1.2 + P.depth * 0.006;
    vec4 layer = vec4(0.0);

    // Tail fin.
    if (local.x > 0.9 * P.len) {
        float ray;
        float along;
        float d = tailDist(P, local, sp, ray, along);
        float cov = (1.0 - smoothstep(-aa, aa, d)) * 0.62 * (1.0 - 0.45 * clamp(along, 0.0, 1.0));
        if (cov > 0.0) {
            float streak = 0.75 + 0.25 * abs(sin(ray * 9.0));
            layer = vec4(finColour(k, 0.4 + along * 0.6) * streak, cov);
        }
    }

    // Pectoral and pelvic fins: they paddle when the koi is slow, and fold in
    // when it is fast.
    if (local.x > 0.15 * P.len && local.x < 0.82 * P.len) {
        float tuck = clamp((P.effort - 0.6) / 2.0, 0.0, 1.0);
        // Values for all four fins: the spine at the fin roots, and the paddle
        // stroke. The stroke is sin(phase / 2 + side * 0.8), expanded, so each
        // fin needs no more trig.
        float yFront = spineAt(P, 0.24 * P.len).y;
        float yBack = spineAt(P, 0.58 * P.len).y;
        float ps = sin(P.phase * 0.5);
        float pc = cos(P.phase * 0.5);
        // The rest angles, front and back, as (cos, sin).
        float front0 = mix(1.15, 0.5, tuck);
        float back0 = mix(0.8, 0.45, tuck);
        vec2 restFront = vec2(cos(front0), sin(front0));
        vec2 restBack = vec2(cos(back0), sin(back0));
        for (int i = 0; i < 4; i++) {
            bool front = i < 2;
            float side = (i % 2 == 0) ? 1.0 : -1.0;
            vec2 root = front
                ? vec2(0.24 * P.len, yFront + side * WIDTH * P.len * 0.75)
                : vec2(0.58 * P.len, yBack + side * WIDTH * P.len * WIDTH_PELVIC * 0.75);
            float paddle = (ps * 0.6967 + side * pc * 0.7174) * (front ? 0.22 : 0.12);
            // Turn the rest angle by the small paddle angle.
            vec2 rest = front ? restFront : restBack;
            vec2 turn = vec2(1.0 - 0.5 * paddle * paddle, paddle);
            vec2 sweep = vec2(rest.x * turn.x - rest.y * turn.y, rest.y * turn.x + rest.x * turn.y);
            float x;
            float ray;
            float d = front
                ? finDist(local, root, side, sweep, 0.24 * P.len, 0.15 * P.len, x, ray)
                : finDist(local, root, side, sweep, 0.12 * P.len, 0.08 * P.len, x, ray);
            float cov = (1.0 - smoothstep(-aa, aa, d)) * 0.6 * (1.0 - 0.4 * x);
            if (cov > 0.0) {
                vec3 c = finColour(k, x) * (0.78 + 0.22 * abs(sin(ray * 22.0)));
                layer = vec4(mix(layer.rgb, c, cov), max(layer.a, cov));
            }
        }
    }

    // Body.
    if (local.x < 1.01 * P.len) {
        float t;
        float s;
        float dv;
        float d = bodyDist(P, local, sp, t, s, dv);
        float cov = 1.0 - smoothstep(-aa, aa, d);
        if (cov > 0.0) {
            float metal;
            s = clamp(s, -1.0, 1.0);
            vec3 c = koiSkin(k, t, s, metal);
            float sc = scales(local.x, dv, P.len);
            c *= 1.0 - (k.variety == 6 || k.variety == 7 ? 0.16 : 0.05) * sc;

            // A round back, lit on the side of the light.
            vec3 n = normalize(vec3(P.left * s * 0.85, sqrt(max(1.0 - s * s * 0.72, 0.0))));
            float diffuse = 0.62 + 0.45 * max(dot(n, ld), 0.0);
            vec3 h = normalize(ld + vec3(0.0, 0.0, 1.0));
            float spec = pow(max(dot(n, h), 0.0), metal > 0.0 ? 40.0 : 18.0) * (metal > 0.0 ? 0.7 : 0.18);
            // Metallic scales catch the light one at a time.
            vec2 scaleId = floor(vec2(local.x, dv) / (P.len * 0.05));
            spec += metal * 0.35 * pow(hashI(ivec2(scaleId) + ivec2(k.variety * 97)), 12.0) * (1.0 - sc);
            c = c * diffuse * (0.72 + 0.28 * sqrt(1.0 - s * s)) + L.sun * spec * L.direct;

            // Gill covers, and the dorsal fin folded along the back.
            float gill = (1.0 - smoothstep(0.0, 0.012, abs(t - 0.22 - 0.05 * s * s))) * smoothstep(0.35, 0.6, abs(s));
            c *= 1.0 - 0.18 * gill;
            float dorsal = (1.0 - smoothstep(0.03, 0.1, abs(s))) * smoothstep(0.3, 0.38, t) * (1.0 - smoothstep(0.68, 0.76, t));
            c = mix(c, c * 0.72, dorsal * 0.55);

            // Eyes.
            if (t < 0.16) {
                float ue = 0.085 * P.len;
                float ye = spineAt(P, ue).y;
                float off = WIDTH * P.len * WIDTH_EYES * 0.72;
                for (int e = 0; e < 2; e++) {
                    float side = e == 0 ? 1.0 : -1.0;
                    vec2 eye = vec2(ue, ye + side * off);
                    float de = length(local - eye) - 0.022 * P.len;
                    c = mix(c, vec3(0.03, 0.03, 0.04), 1.0 - smoothstep(-px, px, de));
                    float glint = length(local - eye - vec2(-0.006, 0.005 * side) * P.len) - 0.007 * P.len;
                    c = mix(c, vec3(0.9), (1.0 - smoothstep(-px, px, glint)) * 0.7);
                }
            }

            // Light through the ripples moves on koi near the surface.
            c += L.sun * caustic * L.direct * 0.3 * (1.0 - P.depth);
            layer = vec4(mix(layer.rgb, c, cov), max(layer.a, cov));
        }
    }
    return layer;
}

// Soft shadow of a koi on the pond bed at `local` (already shifted).
float koiShadow(Pose P, vec2 local, SpinePoint sp) {
    float t;
    float s;
    float dv;
    float d = bodyDist(P, local, sp, t, s, dv);
    if (local.x > 0.9 * P.len) {
        float ray;
        float along;
        d = min(d, tailDist(P, local, sp, ray, along) + 0.004);
    }
    float blur = 0.005 + 0.012 * (1.0 - P.depth);
    return 1.0 - smoothstep(-blur, blur, d);
}

// ---- lily pads --------------------------------------------------------------

// A water lily. It closes at night.
vec4 waterLily(vec2 d, float R, float open) {
    float rad = length(d);
    if (rad > R * 1.2) {
        return vec4(0.0);
    }
    float a = atan(d.y, d.x);
    float sector = TAU / 8.0;
    vec4 col = vec4(0.0);
    // Outer petals, then inner petals turned by half a petal.
    for (int ring = 0; ring < 2; ring++) {
        float offset = ring == 0 ? 0.0 : sector * 0.5;
        float la = mod(a + offset + sector * 0.5, sector) - sector * 0.5;
        vec2 q = vec2(cos(la), sin(la)) * rad;
        float reach = (ring == 0 ? 0.58 : 0.36) * R * open;
        vec2 r = (ring == 0 ? vec2(0.46, 0.2) : vec2(0.3, 0.15)) * R * vec2(open, 1.0);
        vec2 e = (q - vec2(reach, 0.0)) / r;
        float dist = (length(e) - 1.0) * min(r.x, r.y);
        float cov = 1.0 - smoothstep(-0.0015, 0.0015, dist);
        float along = clamp(q.x / (reach + r.x), 0.0, 1.0);
        vec3 c = mix(vec3(1.0, 0.97, 0.96), vec3(0.97, 0.56, 0.7), smoothstep(0.35, 1.0, along));
        c *= 0.82 + 0.18 * (1.0 - abs(q.y) / r.y);
        c *= ring == 0 ? 0.92 : 1.0;
        col = vec4(mix(col.rgb, c, cov), max(col.a, cov));
    }
    float centre = 1.0 - smoothstep(0.15 * R - 0.0015, 0.15 * R + 0.0015, rad);
    vec3 gold = vec3(1.0, 0.78, 0.25) * (0.8 + 0.2 * noise(d / R * 12.0));
    return vec4(mix(col.rgb, gold, centre), max(col.a, centre));
}

// Color and coverage of the lily pads at point q.
vec4 lilyPads(vec2 q, vec2 pond, float t, float px, vec3 ld, Light L, uint mask) {
    vec4 col = vec4(0.0);
    for (int i = 0; i < NUM_PADS; i++) {
        if (!hasBit(mask, i)) {
            continue;
        }
        vec4 def = padDef(i);
        float r = def.z;
        vec2 d = q - padCentre(i, pond, t);
        if (dot(d, d) > r * r * 1.5) {
            continue;
        }
        float ang = def.w + 0.08 * sin(t * 0.03 + float(i));
        vec2 local = mat2(cos(ang), -sin(ang), sin(ang), cos(ang)) * d;
        float a = atan(local.y, local.x);
        float rad = length(local);
        float edge = rad - r * (1.0 + 0.025 * sin(a * 7.0 + float(i)));
        // The notch: a narrow wedge from the center to the rim.
        float dist = max(edge, -(abs(a) - 0.17) * rad);
        float cov = 1.0 - smoothstep(-px, px, dist);
        if (cov <= 0.0) {
            continue;
        }
        float x = rad / r;
        vec3 c = mix(vec3(0.33, 0.56, 0.25), vec3(0.2, 0.42, 0.18), x);
        float veins = smoothstep(0.82, 1.0, cos(a * 19.0)) * smoothstep(0.12, 0.35, x);
        c = mix(c, vec3(0.45, 0.66, 0.33), veins * 0.5);
        c = mix(c, vec3(0.46, 0.48, 0.2), smoothstep(0.86, 1.0, x) * 0.6);
        c = mix(c, vec3(0.42, 0.33, 0.15), smoothstep(0.68, 0.8, noise(local / r * 5.0 + float(i) * 7.0)) * 0.5);
        // The rim curls up a little and catches the light.
        vec3 n = normalize(vec3(local / max(rad, 1e-4) * smoothstep(0.75, 1.0, x) * 0.5, 1.0));
        float spec = pow(max(dot(n, normalize(ld + vec3(0.0, 0.0, 1.0))), 0.0), 30.0);
        c = c * L.ambient * (0.75 + 0.35 * max(dot(n, ld), 0.0)) + L.sun * spec * 0.15 * L.direct;
        col = vec4(mix(col.rgb, c, cov), max(col.a, cov));

        if (i == 0 || i == 3) {
            vec4 lily = waterLily(local - vec2(-0.25, 0.2) * r, r * 0.55, mix(1.0, 0.55, L.night));
            lily.rgb *= L.ambient * 1.05 + L.sun * 0.1 * L.direct;
            col = vec4(mix(col.rgb, lily.rgb, lily.a), max(col.a, lily.a));
        }
    }
    return col;
}

// Soft shadow of the pads on the bed.
float padShadow(vec2 q, vec2 pond, float t, uint mask) {
    float shade = 0.0;
    for (int i = 0; i < NUM_PADS; i++) {
        if (!hasBit(mask, NUM_PADS + i)) {
            continue;
        }
        float d = length(q - padCentre(i, pond, t)) - padDef(i).z;
        shade = max(shade, 1.0 - smoothstep(-0.02, 0.02, d));
    }
    return shade;
}

// ---- night ------------------------------------------------------------------

vec3 fireflies(vec2 p, vec2 pond, float t, uint mask) {
    float glow = 0.0;
    for (int k = 0; k < NUM_FIREFLIES; k++) {
        if (!hasBit(mask, k)) {
            continue;
        }
        float fk = float(k);
        float blink = pow(0.5 + 0.5 * sin(t * (0.6 + 0.4 * hash1(fk + 9.0)) + fk * 4.0), 6.0);
        vec2 d = p - fireflyPos(k, pond, t);
        float r2 = dot(d, d);
        glow += blink * (exp(-r2 / 0.00003) * 1.2 + exp(-r2 / 0.0008) * 0.22);
    }
    return vec3(1.0, 0.86, 0.42) * glow;
}

// ---- compose ----------------------------------------------------------------

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 pond = vec2(iResolution.x / iResolution.y, 1.0);
    vec2 p = fragCoord / iResolution.y;
    vec2 uv = fragCoord / iResolution.xy;
    float px = 1.0 / iResolution.y;
    float t = iTime;

    float hour = hourOfDay();
    Light L = lightAt(hour);
    // The koi, shadows, pads, and fireflies that can touch this part of the
    // screen.
    vec4 tile = texelFetch(iChannel2, ivec2(uv * iChannelResolution[2].xy), 0);
    uint koiMask = uint(tile.x);
    uint shadowMask = uint(tile.y);
    uint padMask = uint(tile.z);
    vec3 ld = lightDirection(hour, L.night);

    // Surface normal from the ripples, plus a faint breeze.
    vec2 texel = 1.0 / iChannelResolution[1].xy;
    float hl = texture(iChannel1, uv - vec2(texel.x, 0.0)).x;
    float hr = texture(iChannel1, uv + vec2(texel.x, 0.0)).x;
    float hd = texture(iChannel1, uv - vec2(0.0, texel.y)).x;
    float hu = texture(iChannel1, uv + vec2(0.0, texel.y)).x;
    vec2 slope = vec2(hr - hl, hu - hd) * 1.6;
    vec2 breeze = 0.25 * vec2(sin(dot(p, vec2(23.0, 9.0)) + t * 1.3) + sin(dot(p, vec2(-11.0, 27.0)) - t * 1.1),
                              sin(dot(p, vec2(17.0, -21.0)) + t * 0.9) + sin(dot(p, vec2(29.0, 13.0)) - t * 1.5));
    vec3 n = normalize(vec3(-slope + breeze * 0.05, 1.0));

    // Everything under the surface looks bent by it.
    vec2 bend = n.xy * 0.04;
    vec2 q = p + bend * min(1.0, MAX_REFRACTION / max(length(bend), 1e-6));
    // Caustics are soft light, so caustics.glsl draws them at half height.
    // Sample them where the ripples bend.
    float caustic = texture(iChannel4, q / pond).r;

    vec2 shadowDir = shadowDirection(ld);
    float shade = padShadow(q - shadowDir * 0.08, pond, t, padMask) * 0.55;
    // Pads that can shade a koi at any depth between the surface and the bed.
    uint padShadowMask = padMask | ((padMask & 0xffu) << uint(NUM_PADS));
    for (int i = 0; i < NUM_FISH; i++) {
        if (!hasBit(shadowMask, i)) {
            continue;
        }
        Koi k = koiOf(i);
        Pose P = loadPose(i);
        vec2 local = localOf(P, q - shadowDir * shadowReach(P.depth));
        SpinePoint sp = spineAt(P, local.x);
        if (nearKoi(P, local, sp)) {
            shade = max(shade, koiShadow(P, local, sp) * mix(0.55, 0.3, P.depth));
        }
    }

    // The pond bed does not change, so bed.glsl drew it once. RGB is the
    // color, and alpha is how close the point is to the banks.
    vec4 bed = texture(iChannel3, q / pond);
    float bank = bed.a;
    vec3 col = bed.rgb * L.ambient * 0.9;
    // Caustics fade with depth. They are strongest in the shallow water near
    // the banks.
    col += L.sun * caustic * L.direct * mix(0.1, 0.24, bank) * (1.0 - shade);
    col *= 1.0 - shade * mix(0.4, 0.75, L.direct);

    // Koi, from the deepest to the shallowest.
    vec3 water = vec3(0.07, 0.2, 0.19) * L.ambient;
    // Bit r of the koi mask is the koi drawn in place r, deepest first.
    for (int r = 0; r < NUM_FISH; r++) {
        if (!hasBit(koiMask, r)) {
            continue;
        }
        int i = int(texelFetch(iChannel0, ivec2(NUM_FISH + r / 4, 0), 0)[r % 4] + 0.5);
        Koi k = koiOf(i);
        Pose P = loadPose(i);
        vec2 local = localOf(P, q);
        SpinePoint sp = spineAt(P, local.x);
        if (!nearKoi(P, local, sp)) {
            continue;
        }
        vec4 layer = drawKoi(P, k, local, sp, px, ld, L, caustic);
        vec3 kc = mix(layer.rgb * L.ambient, water, P.depth * 0.5) * (1.0 - 0.2 * P.depth);
        // A koi under a lily pad swims through its shade.
        float padShade = padShadow(q - shadowDir * 0.08 * P.depth, pond, t, padShadowMask);
        kc *= 1.0 - padShade * mix(0.25, 0.5, L.direct);
        col = mix(col, kc, layer.a);
    }

    // The water: a green-teal tint, stronger where the pond is deep.
    col = mix(col, water, 0.1 + 0.1 * (1.0 - bank));

    // The surface reflects the sky, more where ripples tilt it.
    float tilt = 1.0 - n.z;
    col = mix(col, L.sky * 0.9, clamp(0.09 + tilt * 3.5, 0.0, 0.25));
    vec3 h = normalize(ld + vec3(0.0, 0.0, 1.0));
    col += L.sun * min(pow(max(dot(n, h), 0.0), 320.0), 0.3) * L.direct;
    // Ripples also catch a soft sheen where they tilt toward the light. Flat
    // water gets none, so the sheen shows only the ripples. It is strongest at
    // night, when moonlight alone shows the ripples.
    float sheen = max(pow(max(dot(n, h), 0.0), 60.0) - pow(h.z, 60.0), 0.0);
    col += L.sun * sheen * mix(0.05, 0.4, L.night);

    if (L.night > 0.01) {
        // The moon shows in the water, broken by the ripples.
        vec4 sky = rightDisplay();
        vec2 m = (p + n.xy * 0.25 - (sky.xy + sky.zw * vec2(0.76, 0.78)) / iResolution.y)
            * vec2(1.0, 0.6);
        float moon = exp(-dot(m, m) / 0.0015) * 0.35 + exp(-dot(m, m) / 0.00015) * 0.6;
        col += vec3(0.8, 0.88, 1.0) * moon * L.night;
    }

    // Lily pads float on top and rock with the ripples.
    vec4 pads = lilyPads(p + n.xy * 0.006, pond, t, px, ld, L, padMask);
    col = mix(col, pads.rgb, pads.a);

    if (L.night > 0.01) {
        col += fireflies(p, pond, t, uint(tile.w)) * L.night;
    }

    // Vignette, and a little noise against bands.
    // Each display gets its own vignette, also when the pond spans them.
    vec4 display = displayAt(fragCoord);
    vec2 inDisplay = (fragCoord - display.xy) / display.zw;
    col *= 1.0 - 0.25 * pow(length((inDisplay - 0.5) * vec2(1.1, 1.0)), 2.2);
    col += (hashI(ivec2(fragCoord)) - 0.5) / 255.0;
    fragColor = vec4(col, 1.0);
}
