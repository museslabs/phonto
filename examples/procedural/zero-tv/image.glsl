// The image pass. Its channels:
//   0: the video
//   1: the tapes buffer with the front faces
//   2: the state
//   3: the back of the shelf

// The screen of the TV: center, half size, and corner radius.
const vec2 TV_CENTER = vec2(948.5, 288.5);
const vec2 TV_HALF = vec2(283.5, 199.5);
const float TV_RADIUS = 40.0;

// The opening of the VCR for tapes: left, top, right, bottom.
const vec4 SLOT = vec4(762.0, 565.0, 1145.0, 613.0);


// The colors of a tape, normal and selected at the low point of the pulse,
// from the video. A selected tape maps each channel through these points.
const vec3 NORMAL[7] = vec3[7](
    vec3(0.0), vec3(54.0, 42.0, 65.0) / 255.0, vec3(70.0, 58.0, 82.0) / 255.0,
    vec3(108.0, 93.0, 116.0) / 255.0, vec3(146.0, 131.0, 149.0) / 255.0,
    vec3(170.0, 149.0, 176.0) / 255.0, vec3(1.0)
);
const vec3 SELECTED[7] = vec3[7](
    vec3(0.0), vec3(50.0, 36.0, 69.0) / 255.0, vec3(87.0, 71.0, 106.0) / 255.0,
    vec3(131.0, 108.0, 140.0) / 255.0, vec3(183.0, 162.0, 189.0) / 255.0,
    vec3(239.0, 214.0, 244.0) / 255.0, vec3(1.0)
);

// The pulse of the selected tape in the video: a period of 0.75 seconds, in
// steps of 1/20 second.
const float PULSE_PERIOD = 0.75;
const float PULSE_GAIN = 0.18;

const vec3 OSD_GREEN = vec3(70.0, 215.0, 135.0) / 255.0;

vec3 video(vec2 s) {
    return texture(iChannel0, vec2(s.x / VIDEO.x, 1.0 - s.y / VIDEO.y)).rgb;
}

vec3 selectedColor(vec3 c, float pulse) {
    vec3 o = SELECTED[0];
    for (int i = 0; i < 6; i++) {
        vec3 t = clamp((c - NORMAL[i]) / (NORMAL[i + 1] - NORMAL[i]), 0.0, 1.0);
        o += (SELECTED[i + 1] - SELECTED[i]) * t;
    }
    return min(o * (1.0 + PULSE_GAIN * pulse), 1.0);
}

float pulse() {
    float t = floor(iTime * 20.0) / 20.0;
    return 0.5 - 0.5 * cos(6.2831853 * t / PULSE_PERIOD);
}

// The size of the tapes buffer: 12 front faces, 62 rows each, with 2
// pixels on each side.
const vec2 FACES_SIZE = vec2(454.0, 744.0);

// The front face of `tape` at `face`, in pixels from its top left corner,
// with alpha. The video is soft, so the tapes are too: four samples between
// the pixels. The color is premultiplied by alpha.
vec4 frontColor(int tape, vec2 face) {
    float height = PLACES[tape - 1].z;
    vec4 c = vec4(0.0);
    for (int i = 0; i < 4; i++) {
        vec2 o = vec2(float(i & 1), float(i >> 1)) * 1.2 - 0.6;
        // Stay in the rows of this face: the faces of the other tapes are
        // above and under it in the buffer.
        vec2 at = vec2(face.x + o.x + 2.0, float((13 - tape) * 62) - clamp(face.y + o.y, 0.5, height - 0.5));
        vec4 t = texture(iChannel1, at / FACES_SIZE);
        c += vec4(t.rgb * t.a, t.a) * 0.25;
    }
    return c;
}

// The top face of a tape, 12 rows from its back edge to its front edge. The
// colors come from the tapes on top of the stacks in the video.
const vec3 TOP_FACE[12] = vec3[12](
    vec3(24.0, 12.0, 42.0) / 255.0, vec3(36.0, 24.0, 54.0) / 255.0,
    vec3(47.0, 37.0, 65.0) / 255.0, vec3(49.0, 39.0, 67.0) / 255.0,
    vec3(53.0, 43.0, 70.0) / 255.0, vec3(61.0, 51.0, 78.0) / 255.0,
    vec3(72.0, 62.0, 88.0) / 255.0, vec3(77.0, 67.0, 93.0) / 255.0,
    vec3(76.0, 66.0, 90.0) / 255.0, vec3(68.0, 58.0, 82.0) / 255.0,
    vec3(55.0, 45.0, 69.0) / 255.0, vec3(41.0, 31.0, 55.0) / 255.0
);
const vec3 TOP_OUTLINE = vec3(30.0, 20.0, 45.0) / 255.0;
const vec3 SHINE = vec3(214.0, 206.0, 228.0) / 255.0;
const vec3 SHINE_DIM = vec3(118.0, 106.0, 138.0) / 255.0;

// The right side of a box faces away from the light, so it is darker than the
// front. It has a lit edge where it meets the front, and an outline at the
// back. The left side faces the light, so it is lighter.
const vec3 SIDE = vec3(46.0, 36.0, 62.0) / 255.0;
const vec3 SIDE_LIGHT = vec3(64.0, 53.0, 82.0) / 255.0;
const vec3 SIDE_EDGE = vec3(96.0, 84.0, 112.0) / 255.0;
const vec3 SIDE_DARK = vec3(30.0, 20.0, 45.0) / 255.0;
const vec3 BEVEL_LIGHT = vec3(104.0, 92.0, 120.0) / 255.0;

// The color of the top face at `face`: the rows back from the front edge,
// and the fraction along the row. The shine on it is a line one art pixel
// tall, brightest in its middle, as on the tapes of the video.
vec3 topColor(vec2 face) {
    float back = face.x;
    float x = face.y;
    vec3 c = TOP_FACE[clamp(int(12.0 - back), 0, 11)];
    if (x < 0.008 || x > 0.992) {
        return TOP_OUTLINE;
    }
    float row = 12.0 - back;
    if (row >= 6.5 && row < 8.5) {
        float a = smoothstep(0.03, 0.08, x) * (1.0 - smoothstep(0.2, 0.27, x));
        float b = smoothstep(0.7, 0.74, x) * (1.0 - smoothstep(0.86, 0.9, x));
        c = mix(c, SHINE, a * 0.85);
        c = mix(c, SHINE_DIM, b * 0.7);
    }
    return c;
}

// The color of a side at `face`: the pixels back from the front edge, and the
// row from the top edge. The side is `height` rows tall and `width` pixels
// wide on the screen.
vec3 sideColor(vec2 face, float height, float width, bool left) {
    float back = face.x;
    float row = face.y;
    if (back >= width - 3.0 || row >= height - 3.0) {
        return SIDE_DARK;
    }
    if (row < 3.0 || back < 3.0) {
        return left ? BEVEL_LIGHT : SIDE_EDGE;
    }
    // Lighter near the front, and a groove around the middle, as on the
    // end of a sleeve.
    vec3 c = mix(SIDE_LIGHT, SIDE, smoothstep(0.0, width, back));
    if (left) {
        c = mix(c, BEVEL_LIGHT, 0.35);
    }
    if (abs(row - height * 0.5) < 1.5) {
        c *= 0.75;
    }
    return c;
}

// `tape`, moved by `offset` and `amount` out (0 to 1), at scene pixel `p`:
// its color and its cover. A tape that is out has the colors and the pulse
// of a selected tape.
vec4 tapeColor(int tape, vec2 p, vec2 offset, float amount) {
    Box b = boxOf(tape, offset);
    vec2 face;
    float edge;
    int part = boxCover(b, p, face, edge);
    vec4 result = vec4(0.0);
    if (part == 1) {
        vec4 c = frontColor(tape, face);
        if (c.a > 0.0) {
            result = vec4(c.rgb / c.a, c.a);
        }
    } else if (part == 2) {
        // Rows back from the front edge, as at no turn, and the fraction
        // along the width.
        result = vec4(topColor(vec2(face.y * 12.0, face.x / TAPE_W)), clamp(edge + 0.5, 0.0, 1.0));
    } else if (part >= 3) {
        float width = abs(b.depth.x);
        result = vec4(sideColor(vec2(face.x * width, face.y), b.height, width, part == 4),
                      clamp(edge + 0.5, 0.0, 1.0));
    }
    if (result.a > 0.0 && amount > 0.0) {
        result.rgb = mix(result.rgb, selectedColor(result.rgb, pulse()), amount);
    }
    return result;
}

// 1 inside a rounded rectangle of half size `extent` around `center`, with
// soft edges.
float roundedBox(vec2 s, vec2 center, vec2 extent, float radius) {
    vec2 d = abs(s - center) - extent + radius;
    float dist = length(max(d, 0.0)) + min(max(d.x, d.y), 0.0) - radius;
    return 1.0 - smoothstep(-2.0, 1.0, dist);
}

float hash(vec2 p) {
    p = fract(p * vec2(123.34, 456.21));
    p += dot(p, p + 45.32);
    return fract(p.x * p.y);
}

// The rows of the 5x7 glyphs of the VCR text, 5 bits each, with the left pixel
// in bit 4. The glyphs are P, L, A, Y, a play triangle, and a rewind triangle.
int osdRow(int glyph, int row) {
    const int ROWS[42] = int[42](
        30, 17, 17, 30, 16, 16, 16,
        16, 16, 16, 16, 16, 16, 31,
        14, 17, 17, 31, 17, 17, 17,
        17, 17, 10, 4, 4, 4, 4,
        16, 24, 28, 30, 28, 24, 16,
        1, 3, 7, 15, 7, 3, 1
    );
    return ROWS[glyph * 7 + row];
}

// 1 on the pixels of the VCR text at the top left of the TV: "PLAY" and a play
// triangle, or two rewind triangles. The text is bold, as the "AV 1" text of
// the video is.
float osd(vec2 s, bool rewind) {
    const float CELL = 8.5;
    const vec2 ORIGIN = vec2(686.0, 162.0);
    vec2 c = (s - ORIGIN) / CELL;
    if (c.y < 0.0 || c.y >= 7.0 || c.x < 0.0) {
        return 0.0;
    }
    int x = int(c.x);
    int glyph;
    int col;
    if (rewind) {
        // Two rewind triangles that touch, as on a VCR.
        glyph = x < 12 ? 5 : -1;
        col = x - (x / 6) * 6;
    } else {
        // Each letter takes 7 cells: 6 for the bold glyph and 1 for the
        // space. The triangle starts 1 cell after the Y.
        glyph = x < 28 ? x / 7 : 4;
        col = x < 28 ? x - glyph * 7 : x - 28;
    }
    if (glyph < 0 || col < 0 || col > 5) {
        return 0.0;
    }
    // A bold pixel lights its own cell and the cell to its right.
    int bits = osdRow(glyph, int(c.y));
    int here = col <= 4 ? (bits >> (4 - col)) & 1 : 0;
    int left = col >= 1 ? (bits >> (5 - col)) & 1 : 0;
    return float(here | left);
}

float ease(float t) {
    t = clamp(t, 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

// The clicked tape on its way to the VCR, at time `t` of the click. `offset`
// moves its middle from its place in the stack, and `scale` sizes it. Returns
// false when it is in the VCR.
bool flight(int tape, float t, out vec2 offset, out float scale) {
    Box b = boxOf(tape, pullOffset(1.0));
    vec2 home = b.origin + b.along * TAPE_W * 0.5 + vec2(0.0, b.height * 0.5);
    vec2 slot = (SLOT.xy + SLOT.zw) * 0.5;
    float slotScale = (SLOT.z - SLOT.x - 6.0) / TAPE_W;
    vec2 up = vec2(0.0, -42.0);
    if (t < CLICK_OUT) {
        float u = ease(t / CLICK_OUT);
        offset = up * u;
        scale = mix(1.0, 1.05, u);
    } else if (t < CLICK_ARRIVE) {
        float u = ease((t - CLICK_OUT) / (CLICK_ARRIVE - CLICK_OUT));
        offset = mix(home + up, slot, u) - home - vec2(0.0, sin(3.14159 * u) * 60.0);
        scale = mix(1.05, slotScale, u);
    } else if (t < CLICK_IN) {
        float u = ease((t - CLICK_ARRIVE) / (CLICK_IN - CLICK_ARRIVE));
        offset = slot - home + vec2(0.0, 3.0 * u);
        scale = slotScale * mix(1.0, 0.9, u);
    } else {
        return false;
    }
    offset = floor(offset / 3.0 + 0.5) * 3.0;
    return true;
}

// The scene at `s`: the video, the shelf in layers, the tape that flies,
// and the TV.
vec3 compose(vec2 s, vec4 state) {
    vec3 col = inShelf(s) ? texelFetch(iChannel3, shelfTexel(s), 0).rgb : video(s);
    int gone = clickedTape(state);
    float t = clickTime(state);

    // The stacks from left to right, and each stack from its bottom tape up,
    // so a tape is in front of the tapes under it. Where a tape slid out,
    // the back of the shelf shows.
    if (nearShelf(s)) {
        for (int stack = 0; stack < 3; stack++) {
            if (!stackNear(stack, s)) {
                continue;
            }
            for (int k = stack * 4 + 4; k > stack * 4; k--) {
                if (k == gone) {
                    continue;
                }
                float amount = texelFetch(iChannel2, ivec2(k, 0), 0).x;
                vec2 out1 = pullOffset(amount);
                vec2 offset = out1 + vec2(0.0, fallOf(state, k));
                vec4 c = tapeColor(k, s, offset, out1 == vec2(0.0) ? 0.0 : amount);
                col = mix(col, c.rgb, c.a);
            }
        }
    }

    // The clicked tape, in front of everything until it is in the VCR.
    vec2 offset;
    float scale;
    if (gone > 0) {
        bool inSlot = s.x >= SLOT.x && s.x < SLOT.z && s.y >= SLOT.y && s.y < SLOT.w;
        float open = smoothstep(CLICK_ARRIVE - 0.05, CLICK_ARRIVE + 0.05, t)
            * (1.0 - smoothstep(CLICK_IN, CLICK_IN + 0.12, t));
        if (inSlot) {
            col = mix(col, vec3(0.03, 0.02, 0.05), open * 0.85);
        }
        if (flight(gone, t, offset, scale)) {
            vec2 out1 = pullOffset(1.0);
            Box b = boxOf(gone, out1);
            vec2 home = b.origin + b.along * TAPE_W * 0.5 + vec2(0.0, b.height * 0.5);
            vec2 q = home + (s - home - offset) / scale;
            bool entering = t >= CLICK_ARRIVE;
            if (!entering || inSlot) {
                vec4 c = tapeColor(gone, q, out1, 1.0);
                if (c.a > 0.5) {
                    float flash = (1.0 - smoothstep(0.0, 0.3, t)) * 0.5;
                    vec3 tc = mix(c.rgb, vec3(1.0), flash);
                    if (entering) {
                        tc *= 1.0 - 0.8 * ease((t - CLICK_ARRIVE) / (CLICK_IN - CLICK_ARRIVE));
                    }
                    col = tc;
                }
            }
        }
    }

    // When the tape is in, the TV shows static, and then the text of a VCR
    // that plays, or that rewinds.
    float play = t - CLICK_IN;
    if (gone > 0 && play > 0.0) {
        float tv = roundedBox(s, TV_CENTER, TV_HALF, TV_RADIUS);
        float frame = floor(iTime * 30.0);
        float grain = hash(floor(s / 3.0) + frame * 17.0);
        float band = hash(vec2(floor(s.y / 3.0), frame));
        vec3 snow = vec3(mix(grain, band, 0.35)) * vec3(0.92, 0.88, 1.0);
        float noise = 1.0 - smoothstep(0.2, 0.5, play);
        col = mix(col, snow, noise * tv);
        bool rewind = since(state.w) >= REWIND_START;
        float text = osd(s, rewind) * step(0.25, play);
        float scan = 0.65 + 0.35 * step(fract(s.y / 4.5), 0.55);
        col = mix(col, OSD_GREEN * scan, text * tv);
    }
    return col;
}

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 s = toScene(fragCoord, iResolution.xy);
    vec4 state = texelFetch(iChannel2, ivec2(0), 0);

    // While the tape rewinds, the picture has the noise of a VHS tape that
    // rewinds. Bands of rows shift sideways, a tracking band rolls down, and
    // white lines and faded colors show.
    float r = 0.0;
    float rw = since(state.w) - REWIND_START;
    if (clickedTape(state) > 0 && rw > 0.0) {
        r = sin(3.14159 * clamp(rw / (REWIND_END - REWIND_START), 0.0, 1.0));
    }
    float frame = floor(iTime * 30.0);
    if (r > 0.0) {
        float band = floor(s.y / 9.0);
        s.x += (hash(vec2(band, frame)) - 0.5) * 28.0 * r;
        float roll = fract(rw * 1.7) * VIDEO.y;
        float tear = 1.0 - smoothstep(0.0, 40.0, abs(s.y - roll));
        s.x += tear * 60.0 * r * (hash(vec2(floor(s.y / 3.0), frame)) - 0.3);
    }

    vec3 col = compose(s, state);

    if (r > 0.0) {
        float line = step(0.992, hash(vec2(floor(s.y / 3.0), frame + 7.0)));
        float luma = dot(col, vec3(0.299, 0.587, 0.114));
        col = mix(col, vec3(luma) * vec3(0.92, 0.88, 1.08), 0.35 * r);
        col += line * 0.6 * r;
    }
    fragColor = vec4(col, 1.0);
}
