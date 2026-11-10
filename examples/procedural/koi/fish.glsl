// Koi behavior. Each koi wanders, keeps away from the other koi, turns away
// from the banks, and swims to the food from a click. common.glsl describes
// the layout of this buffer.

vec2 dirOf(float a) {
    return vec2(cos(a), sin(a));
}

vec2 closestOnSegment(vec2 p, vec2 a, vec2 b) {
    vec2 ba = b - a;
    return a + ba * clamp(dot(p - a, ba) / max(dot(ba, ba), 1e-8), 0.0, 1.0);
}

float wrapAngle(float a) {
    return mod(a + PI, TAU) - PI;
}

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    ivec2 cell = ivec2(fragCoord);
    vec2 pond = vec2(iImageResolution.x / iImageResolution.y, 1.0);
    bool start = iFrame == 0;
    vec4 food = texelFetch(iChannel0, ivec2(CONTROL, 0), 0);

    if (cell.x == CONTROL) {
        // iTime goes back to 0 after a day. Forget food from the future.
        if (start || food.z > iTime) {
            food = vec4(0.0, 0.0, -1e4, 0.0);
        }
        // iMouse.w is positive only on the frame of a click.
        if (iMouse.w > 0.0) {
            food = vec4(iMouse.zw / iImageResolution.y, iTime, 1.0);
        }
        // Row 1: the position of the pressed button in the last frame and in
        // this frame. iMouse.z is positive while the button is pressed.
        vec4 drag = texelFetch(iChannel0, ivec2(CONTROL, 1), 0);
        vec2 now = iMouse.z > 0.0 ? iMouse.xy / iImageResolution.y : vec2(-1.0);
        fragColor = cell.y == 0 ? food : (start ? vec4(-1.0) : vec4(drag.zw, now));
        return;
    }
    if (cell.x >= NUM_FISH) {
        // Columns NUM_FISH and up, row 0: the draw order. Koi indices from the
        // deepest to the shallowest, four in each texel, from the depths of
        // the last frame.
        int slot = (cell.x - NUM_FISH) * 4;
        vec4 order = vec4(0.0);
        if (cell.y == 0 && slot < NUM_FISH) {
            for (int c = 0; c < 4; c++) {
                for (int i = 0; i < NUM_FISH; i++) {
                    float di = texelFetch(iChannel0, ivec2(i, 1), 0).z;
                    int rank = 0;
                    for (int j = 0; j < NUM_FISH; j++) {
                        float dj = texelFetch(iChannel0, ivec2(j, 1), 0).z;
                        if (dj > di || (dj == di && j < i)) {
                            rank++;
                        }
                    }
                    if (rank == slot + c) {
                        order[c] = float(i);
                    }
                }
            }
        }
        fragColor = order;
        return;
    }

    int i = cell.x;
    Koi k = koiOf(i);
    Fish f = loadFish(i);

    if (start || any(isnan(f.pos)) || isnan(f.heading)) {
        f.pos = (0.15 + 0.7 * vec2(hash1(k.seed * 91.0), hash1(k.seed * 57.0 + 3.0))) * pond;
        f.heading = hash1(k.seed * 13.0) * TAU;
        f.speed = k.cruise;
        f.turn = 0.0;
        f.phase = hash1(k.seed * 7.0) * TAU;
        f.depth = 0.2 + 0.6 * hash1(k.seed * 29.0);
        f.effort = 1.0;
        fragColor = cell.y == 0 ? vec4(f.pos, f.heading, f.speed)
                  : cell.y == 1 ? vec4(f.turn, f.phase, f.depth, f.effort)
                                : vec4(0.0);
        return;
    }

    float dt = clamp(iTimeDelta, 0.0, 0.1);
    float t = iTime;
    vec2 fwd = dirOf(f.heading);
    vec2 centre = f.pos - fwd * k.len * 0.5;

    // Wander: a direction that changes slowly.
    float wander = (noise(vec2(t * 0.06 + k.seed * 50.0, k.seed * 11.0)) - 0.5) * 4.0;
    vec2 steer = fwd + dirOf(f.heading + wander) * 0.8;

    // Other koi. A koi steers away early from the nearest part of each body
    // ahead of its nose, so the koi swim around each other and do not push.
    // If two are too close to steer clear, they pass at different depths. The
    // lower koi dives a little, and the other one rises.
    vec2 soon = f.pos + fwd * k.len * 0.35;
    vec2 away = vec2(0.0);
    vec2 align = vec2(0.0);
    vec2 toGroup = vec2(0.0);
    float count = 0.0;
    float depthPush = 0.0;
    for (int j = 0; j < NUM_FISH; j++) {
        if (j == i) {
            continue;
        }
        Fish o = loadFish(j);
        Koi ok = koiOf(j);
        vec2 ofwd = dirOf(o.heading);
        vec2 toOther = (o.pos - ofwd * ok.len * 0.5) - centre;
        if (length(toOther) > 0.3) {
            continue;
        }
        align += ofwd;
        toGroup += toOther;
        count += 1.0;

        vec2 d = soon - closestOnSegment(soon, o.pos, o.pos - ofwd * ok.len);
        float dist = length(d);
        // Two half-widths plus a little room.
        float gap = (k.len + ok.len) * 0.14 + 0.01;
        float sameLayer = 1.0 - smoothstep(0.12, 0.3, abs(o.depth - f.depth));
        // Koi at different depths can cross. They still move apart slowly, so
        // they do not stay on top of each other.
        away += d / max(dist, 1e-4) * (1.0 - smoothstep(gap, gap * 3.0, dist)) * mix(0.3, 1.0, sameLayer);
        if (dist < gap * 1.6) {
            bool lower = f.depth > o.depth || (f.depth == o.depth && i > j);
            depthPush += (lower ? 1.0 : -1.0) * (1.0 - smoothstep(gap * 0.5, gap * 1.6, dist)) * sameLayer;
        }
    }
    if (count > 0.0) {
        steer += normalize(align + 1e-4) * 0.2 + toGroup / count;
    }
    steer += away * 2.5;

    // Lily pads float at the surface. A koi near the top dives to pass under
    // a pad, and does not swim through it.
    float underPad = 0.0;
    for (int p = 0; p < NUM_PADS; p++) {
        float edge = length(soon - padCentre(p, pond, t)) - padDef(p).z;
        underPad = max(underPad, 1.0 - smoothstep(0.0, 0.06, edge));
    }

    // Banks: look ahead and turn back to open water.
    vec2 ahead = f.pos + fwd * 0.12;
    float margin = 0.16;
    steer += ((1.0 - smoothstep(0.0, margin, ahead)) - smoothstep(pond - margin, pond, ahead)) * 4.0;

    // Food: a koi that is farther away sees it later. Each koi goes to its
    // own spot around the food, so the koi do not pile onto one point. Then
    // it circles there.
    bool feeding = false;
    // The golden angle spaces the spots evenly for any number of koi.
    vec2 mySpot = food.xy + 0.06 * dirOf(float(i) * 2.39996 + t * 0.15);
    vec2 toFood = mySpot - f.pos;
    float foodDist = length(toFood);
    float sinceFood = t - food.z;
    if (food.w > 0.5 && sinceFood < FOOD_TIME) {
        float notice = 0.25 + foodDist * 2.0 + hash1(k.seed * 3.0 + food.z) * 0.8;
        if (sinceFood > notice) {
            feeding = true;
            vec2 dir = toFood / max(foodDist, 1e-4);
            steer += dir * (foodDist > 0.08 ? 4.0 : 1.2);
            float spin = k.seed > 0.5 ? 1.0 : -1.0;
            steer += vec2(-dir.y, dir.x) * spin * 2.0 * (1.0 - smoothstep(0.03, 0.12, foodDist));
        }
    }

    // Turn toward the steering direction, with some inertia.
    float err = wrapAngle(atan(steer.y, steer.x) - f.heading);
    f.turn += (err * 5.0 - f.turn * 3.0) * dt;
    f.turn = clamp(f.turn, -1.8, 1.8);
    f.heading = wrapAngle(f.heading + f.turn * dt);

    float mood = noise(vec2(t * 0.09 + k.seed * 20.0, k.seed * 7.0));
    float want = k.cruise * mix(0.45, 1.35, mood);
    if (feeding) {
        want = foodDist > 0.08 ? k.cruise * 2.2 : k.cruise * 0.5;
    }
    want *= 1.0 - 0.35 * clamp(abs(err) / PI, 0.0, 1.0);
    f.speed += (want - f.speed) * (1.0 - exp(-1.3 * dt));
    f.pos = clamp(f.pos + dirOf(f.heading) * f.speed * dt, vec2(0.01), pond - 0.01);

    // Move between the surface and the bottom. Rise for food.
    float wantDepth = mix(0.06, 0.85,
        smoothstep(0.35, 0.75, noise(vec2(t * 0.025 + k.seed * 9.0, k.seed * 31.0))));
    if (feeding) {
        wantDepth = 0.02;
    }
    wantDepth = clamp(wantDepth + depthPush * 0.6, 0.02, 0.95);
    wantDepth = max(wantDepth, 0.45 * underPad);
    // Avoiding another koi or a pad changes depth faster than drifting.
    float depthRate = max(feeding ? 0.9 : 0.25, 1.5 * max(underPad, min(abs(depthPush), 1.0)));
    f.depth += (wantDepth - f.depth) * (1.0 - exp(-depthRate * dt));

    float effort = f.speed / k.cruise * 0.75 + abs(f.turn) * 0.35;
    f.effort += (effort - f.effort) * (1.0 - exp(-4.0 * dt));
    f.phase = mod(f.phase + dt * (2.2 + 4.5 * f.effort), TAU * 8.0);

    // The body follows the curve that the head swam: turn rate divided by
    // speed. The value is smoothed over a quarter second. The turn rate jitters
    // when steering changes, and a slow koi divides by a small speed. Without
    // smoothing, the tail whips from side to side.
    float bend = texelFetch(iChannel0, ivec2(i, 2), 0).x;
    float wantBend = clamp(f.turn / max(f.speed, 0.02), -maxBend(k.len), maxBend(k.len));
    bend += (wantBend - bend) * (1.0 - exp(-4.0 * dt));

    fragColor = cell.y == 0 ? vec4(f.pos, f.heading, f.speed)
              : cell.y == 1 ? vec4(f.turn, f.phase, f.depth, f.effort)
                            : vec4(bend, 0.0, 0.0, 0.0);
}
