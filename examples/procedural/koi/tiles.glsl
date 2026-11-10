// Culling for the image pass. Each texel covers a small square of the screen.
// Its bit masks tell what can show in that square:
//   x: koi, bit r = the koi drawn in place r (deepest first)
//   y: koi shadows, bit i = koi i
//   z: lily pads (bits 0-7) and their shadows (bits 8-15)
//   w: fireflies (only at night)
// The image pass then skips all other koi, pads, and fireflies. The format is
// rgba32f, so the masks are exact.

// True if the koi can be within `margin` of p. This is the test that the
// image pass makes for each pixel, made wider by the margin. The margin is
// larger where the spine is steep.
bool koiNear(Pose P, vec2 p, float margin) {
    vec2 local = localOf(P, p);
    SpinePoint sp = spineAt(P, local.x);
    return local.x > -0.02 * P.len - margin && local.x < 1.32 * P.len + margin
        && abs(local.y - sp.y) < 0.42 * P.len + margin * (1.0 + abs(sp.slope));
}

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 pond = vec2(iImageResolution.x / iImageResolution.y, 1.0);
    // The texels are square, with the aspect ratio of the image, so this is in
    // pond units.
    vec2 centre = fragCoord / iResolution.y;
    // Half the diagonal of a tile, plus the largest shift from refraction.
    float margin = 0.75 / iResolution.y + MAX_REFRACTION;
    float t = iTime;

    float hour = hourOfDay();
    Light L = lightAt(hour);
    vec2 shadowDir = shadowDirection(lightDirection(hour, L.night));

    uint koi = 0u;
    uint shadows = 0u;
    for (int r = 0; r < NUM_FISH; r++) {
        int i = int(texelFetch(iChannel0, ivec2(NUM_FISH + r / 4, 0), 0)[r % 4] + 0.5);
        Pose P = loadPose(i);
        if (koiNear(P, centre, margin)) {
            koi |= 1u << uint(r);
        }
        // A shadow is blurred by up to 0.017 more.
        if (koiNear(P, centre - shadowDir * shadowReach(P.depth), margin + 0.02)) {
            shadows |= 1u << uint(i);
        }
    }

    uint pads = 0u;
    for (int i = 0; i < NUM_PADS; i++) {
        vec2 c = padCentre(i, pond, t);
        float r = padDef(i).z * 1.25 + margin;
        if (length(centre - c) < r) {
            pads |= 1u << uint(i);
        }
        // Shadow bit: the shade of the pad on the bed, or on a koi part of the
        // way down.
        if (length(centre - shadowDir * 0.08 - c) < r + 0.02
            || length(centre - shadowDir * 0.04 - c) < r + 0.06) {
            pads |= 1u << uint(NUM_PADS + i);
        }
    }

    uint flies = 0u;
    if (L.night > 0.01) {
        for (int k = 0; k < NUM_FIREFLIES; k++) {
            if (length(centre - fireflyPos(k, pond, t)) < 0.07 + margin) {
                flies |= 1u << uint(k);
            }
        }
    }

    fragColor = vec4(float(koi), float(shadows), float(pads), float(flies));
}
