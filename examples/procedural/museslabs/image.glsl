// The Muses Labs lyre on the paper of museslabs.org. The strings vibrate
// with the system audio, and each pluck writes the chord that plays on the
// staff.
//
// iChannel0 = lyre.png, the logo without its strings.
// iChannel1 = the strings buffer (see strings.glsl).
// iChannel2 = wordmark.png. Red is "Muses" and green is "Μοῦσαι".
// iChannel3 = clef.png, a treble clef.

// The colors of museslabs.org, converted from OKLCH to sRGB. On a full
// screen, the paper of the site, oklch(96.5% .012 85), looks almost white.
// So the light paper here is a deeper cream, oklch(94.5% .03 84).
const vec3 LIGHT_PAPER = vec3(0.966, 0.924, 0.842);
const vec3 LIGHT_PAPER_EDGE = vec3(0.938, 0.882, 0.793);
const vec3 LIGHT_INK = vec3(0.140, 0.105, 0.082);
const vec3 LIGHT_INK_SOFT = vec3(0.426, 0.378, 0.347);
const vec3 LIGHT_TINT = vec3(0.980, 0.856, 0.775);
const vec3 DARK_PAPER = vec3(0.104, 0.081, 0.066);
const vec3 DARK_PAPER_EDGE = vec3(0.077, 0.055, 0.041);
const vec3 DARK_INK = vec3(0.936, 0.920, 0.888);
const vec3 DARK_INK_SOFT = vec3(0.760, 0.742, 0.705);
const vec3 DARK_TINT = vec3(0.360, 0.203, 0.131);
const vec3 TERRA = vec3(0.785, 0.356, 0.197);

// The colors of this frame, between the light and the dark colors.
// mainImage sets them.
vec3 paper;
vec3 paperEdge;
vec3 ink;
vec3 inkSoft;
vec3 tint;

// 0 for the paper colors and 1 for the dark colors. A change takes 3
// seconds. A slow fade loses the logo, because the paper and the ink swap,
// so halfway they are the same gray.
float darkness() {
#if THEME == 1
    return 0.0;
#elif THEME == 2
    return 1.0;
#else
    const float FADE = 3.0;   // seconds
    float light = smoothstep(LIGHT_FROM * 3600.0, LIGHT_FROM * 3600.0 + FADE, iDate.w)
        * (1.0 - smoothstep(DARK_FROM * 3600.0, DARK_FROM * 3600.0 + FADE, iDate.w));
    return 1.0 - light;
#endif
}
// The site draws its rules in ink at 24% opacity.
const float RULE = 0.24;

// The height of the 400-unit logo square, as a fraction of the screen.
const float LOGO_SIZE = 0.62;
// The logo point at the center of the screen. It centers the lyre and the
// wordmark together.
const vec2 FOCUS = vec2(200.0, 218.0);
// The space between the screen edges and the ends of the staff.
const float STAFF_MARGIN = 55.0;

const float NOTE_LIFE = 6.0;     // seconds
const float NOTE_SPEED = 110.0;  // units per second
// Notes come out of the lyre at this distance from its center.
const float NOTE_START = 105.0;

// The coverage of a line of `width` units at a distance of `d` units.
float line(float d, float width, float px) {
    return clamp((0.5 * width - abs(d)) / px + 0.5, 0.0, 1.0);
}

float segment(vec2 p, vec2 a, vec2 b) {
    vec2 pa = p - a;
    vec2 ba = b - a;
    return length(pa - ba * clamp(dot(pa, ba) / dot(ba, ba), 0.0, 1.0));
}

// The distance to a note head at p = 0.
float head(vec2 p) {
    float c = cos(0.35);
    float s = sin(0.35);
    vec2 h = mat2(c, -s, s, c) * p;
    return (length(h / vec2(6.6, 5.0)) - 1.0) * 5.0;
}

// The distance to a sharp sign at p = 0.
float sharp(vec2 p) {
    float d = min(segment(p, vec2(-2.2, -11.0), vec2(-2.2, 13.0)),
        segment(p, vec2(2.2, -13.0), vec2(2.2, 11.0))) - 0.6;
    d = min(d, segment(p, vec2(-5.0, 2.4), vec2(5.0, 5.6)) - 1.3);
    return min(d, segment(p, vec2(-5.0, -5.6), vec2(5.0, -2.4)) - 1.3);
}

// The coverage of a chord. p is in units from the bottom staff line at the
// x of the chord, with y up. All heads share one stem. The flag makes an
// eighth note.
float chord(vec2 p, int names[3], bool eighth, float px) {
    // The staff heights of the notes, from low to high.
    float y[3];
    bool sharps[3];
    int count = 0;
    for (int i = 0; i < 3; i++) {
        if (names[i] != NO_NOTE) {
            y[count] = float(NOTE_POSITION[names[i]]) * 0.5 * SPACE;
            sharps[count] = NOTE_SHARP[names[i]];
            count++;
        }
    }
    if (count == 0) {
        return 0.0;
    }
    for (int i = 1; i < count; i++) {
        for (int j = i; j > 0 && y[j] < y[j - 1]; j--) {
            float h = y[j];
            y[j] = y[j - 1];
            y[j - 1] = h;
            bool b = sharps[j];
            sharps[j] = sharps[j - 1];
            sharps[j - 1] = b;
        }
    }
    float low = y[0];
    float high = y[count - 1];
    // The stem goes down when the chord is above the middle line.
    bool up = low + high < 4.0 * SPACE;
    float side = up ? 1.0 : -1.0;

    // Two notes a step apart cannot share a column. The second head moves
    // to the other side of the stem.
    bool moved[3] = bool[3](false, false, false);
    bool anyMoved = false;
    for (int k = 1; k < count; k++) {
        int i = up ? k : count - 1 - k;
        int previous = up ? i - 1 : i + 1;
        moved[i] = abs(y[i] - y[previous]) < 0.6 * SPACE && !moved[previous];
        anyMoved = anyMoved || moved[i];
    }

    float d = 1e3;
    for (int i = 0; i < count; i++) {
        d = min(d, head(p - vec2(moved[i] ? 11.8 * side : 0.0, y[i])));
        if (sharps[i]) {
            float x = -15.0 - (i % 2 == 1 ? 9.0 : 0.0) - (!up && anyMoved ? 11.8 : 0.0);
            d = min(d, sharp(p - vec2(x, y[i])));
        }
    }
    vec2 base = vec2(5.9 * side, up ? low : high);
    vec2 tip = vec2(5.9 * side, up ? high + 3.4 * SPACE : low - 3.4 * SPACE);
    d = min(d, segment(p, base, tip) - 0.65);
    if (eighth) {
        // The flag points right. It bulges out and curves back toward the
        // heads, and it gets thinner at its end.
        vec2 a = tip;
        for (int i = 1; i <= 6; i++) {
            float t = float(i) / 6.0;
            vec2 b = tip + vec2(8.5 * sin(2.8 * t) * (1.0 - 0.3 * t), -22.0 * t * side);
            d = min(d, segment(p, a, b) - mix(1.8, 0.5, t));
            a = b;
        }
    }
    return clamp(0.5 - d / px, 0.0, 1.0);
}

// The sideways move of string s at u, from 0 at the crossbar to 1 at the
// bowl. Higher strings move faster. The rates are slow enough to see.
float vibration(int s, float u, float amplitude) {
    float w = (6.0 + 2.0 * float(s)) * 2.0 * PI;
    float t = iTime;
    return amplitude * (sin(PI * u) * sin(w * t)
        + 0.35 * sin(2.0 * PI * u) * sin(1.9 * w * t + 1.3)
        + 0.2 * sin(3.0 * PI * u) * sin(3.1 * w * t + 0.4));
}

vec3 drawStaff(vec3 col, vec2 q, float left, float right, float px) {
    if (q.x < left || q.x > right || q.y < staffY(8.0) - 3.0 * SPACE
        || q.y > staffY(0.0) + 3.0 * SPACE) {
        return col;
    }
    vec3 rule = mix(paper, ink, RULE);
    float width = max(px, 0.75);
    // The staff stops at the lyre.
    float open = smoothstep(98.0, 108.0, abs(q.x - 200.0));
    float lines = 0.0;
    for (int i = 0; i < 5; i++) {
        lines = max(lines, line(q.y - staffY(float(2 * i)), width, px));
    }
    col = mix(col, rule, lines * open);

    // A treble clef at the start. Its curl is on the second line.
    vec2 clefSize = vec2(28.96, 78.8);
    vec2 uv = vec2((q.x - left - 0.5 * SPACE) / clefSize.x,
        0.3775 + (staffY(2.0) - q.y) / clefSize.y);
    if (all(greaterThanEqual(uv, vec2(0.0))) && all(lessThanEqual(uv, vec2(1.0)))) {
        col = mix(col, mix(paper, ink, 0.45), texture(iChannel3, uv).a);
    }

    // A final barline at the end: a thin line and a thick line.
    if (q.y > staffY(8.0) && q.y < staffY(0.0)) {
        float bar = max(line(q.x - (right - 1.15 * SPACE), width, px),
            line(q.x - (right - 0.25 * SPACE), 0.5 * SPACE, px));
        col = mix(col, mix(paper, ink, 0.45), bar);
    }
    return col;
}

vec3 drawNotes(vec3 col, vec2 q, float left, float right, float px) {
    if (q.y < staffY(8.0) - 4.5 * SPACE || q.y > staffY(0.0) + 4.5 * SPACE) {
        return col;
    }
    for (int k = 0; k < NOTES; k++) {
        vec4 n = texelFetch(iChannel1, ivec2(k, 1), 0);
        float age = iTime - n.x;
        if (n.z <= 0.0 || age < 0.0 || age > NOTE_LIFE) {
            continue;
        }
        // Notes that start in the same 1.5 s go the same way, as a phrase.
        float side = mod(floor(n.x / 1.5), 2.0) < 1.0 ? -1.0 : 1.0;
        float x = 200.0 + side * (NOTE_START + NOTE_SPEED * age);
        vec2 p = vec2(q.x - x, STAFF_BOTTOM - q.y);
        if (p.x < -45.0 || p.x > 30.0) {
            continue;
        }
        int names[3];
        bool eighth;
        unpackChord(n.y, names, eighth);
        float fade = smoothstep(0.0, 0.3, age)
            * (1.0 - smoothstep(0.6 * NOTE_LIFE, NOTE_LIFE, age))
            * smoothstep(left + 20.0, left + 60.0, x)
            * (1.0 - smoothstep(right - 60.0, right - 20.0, x));
        col = mix(col, TERRA, chord(p, names, eighth, px) * fade * mix(0.6, 1.0, n.z));
    }
    return col;
}

vec3 drawStrings(vec3 col, vec2 q, float px) {
    if (q.y < STRING_TOP - 1.5 || q.y > 289.0) {
        return col;
    }
    for (int s = 0; s < STRINGS; s++) {
        float dx = q.x - STRING_X[s];
        if (abs(dx) > STRING_HALF_WIDTH + 8.0 || q.y > STRING_COVER[s]) {
            continue;
        }
        vec4 state = texelFetch(iChannel1, ivec2(s, 0), 0);
        float amplitude = 4.5 * state.y + 1.5 * state.x * state.x;
        float u = clamp((q.y - STRING_TOP) / (STRING_BOTTOM[s] - STRING_TOP), 0.0, 1.0);
        float swing = max(sin(PI * u), 0.0);
        // A soft band shows the path of a fast vibration.
        float path = amplitude * swing * 1.1;
        float band = line(dx, 2.0 * (STRING_HALF_WIDTH + path), px);
        col = mix(col, tint, band * min(path / 3.0, 1.0) * 0.7);
        float body = line(dx - vibration(s, u, amplitude), 2.0 * STRING_HALF_WIDTH, px);
        vec3 color = mix(ink, TERRA, clamp(state.y * 1.3, 0.0, 1.0) * sqrt(swing));
        col = mix(col, color, body);
    }
    return col;
}

void mainImage(out vec4 color, in vec2 coord) {
    // Fit the scene to the height, or to the width on a portrait screen.
    float scale = min(iResolution.y, iResolution.x) * LOGO_SIZE / 400.0;
    vec2 q = FOCUS + (coord - 0.5 * iResolution.xy) * vec2(1.0, -1.0) / scale;
    float px = 1.0 / scale;
    float dark = darkness();
    paper = mix(LIGHT_PAPER, DARK_PAPER, dark);
    paperEdge = mix(LIGHT_PAPER_EDGE, DARK_PAPER_EDGE, dark);
    ink = mix(LIGHT_INK, DARK_INK, dark);
    inkSoft = mix(LIGHT_INK_SOFT, DARK_INK_SOFT, dark);
    tint = mix(LIGHT_TINT, DARK_TINT, dark);
    float halfWidth = 0.5 * iResolution.x / scale;
    float left = 200.0 - halfWidth + STAFF_MARGIN;
    float right = 200.0 + halfWidth - STAFF_MARGIN;

    // The paper is a little darker at the edges and has a fine grain.
    vec2 uv = coord / iResolution.xy;
    float edge = smoothstep(0.3, 1.2, length((uv - 0.5) * vec2(1.6, 1.2)));
    vec3 col = mix(paper, paperEdge, edge);
    col += (hash(floor(coord)) - 0.5) * 0.012;

    col = drawStaff(col, q, left, right, px);
    col = drawNotes(col, q, left, right, px);

    vec2 logo = vec2(q.x, 400.0 - q.y) / 400.0;
    if (all(greaterThanEqual(logo, vec2(0.0))) && all(lessThanEqual(logo, vec2(1.0)))) {
        col = mix(col, ink, texture(iChannel0, logo).a);
    }
    col = drawStrings(col, q, px);

    // The wordmark under the lyre, as in the header of the site.
    vec2 wordmarkSize = vec2(112.0, 24.18);
    vec2 w = vec2((q.x - 200.0) / wordmarkSize.x + 0.5, 0.5 - (q.y - 322.0) / wordmarkSize.y);
    if (all(greaterThanEqual(w, vec2(0.0))) && all(lessThanEqual(w, vec2(1.0)))) {
        vec2 text = texture(iChannel2, w).rg;
        col = mix(col, ink, text.r);
        col = mix(col, inkSoft, text.g);
    }
    color = vec4(col, 1.0);
}
