// The tapes buffer: the front face of every tape, flat, drawn as graphics,
// with its label. Tape N has rows (12 - N) * 62 to (13 - N) * 62 - 1, from the
// bottom of the buffer. So tape 1 is at the top. A face starts 2 pixels from
// the left, so its soft edges have room. The image pass puts each face on its
// box. The labels change only when phonto starts, so phonto draws this buffer
// once. iChannel0 is the font. Alpha is 1 on a face and 0 elsewhere.
//
// Every label uses the same pixel font: the label of the configuration, or
// else the name of the tape in the video.

// The places on the front face of each tape, in pixels from its top left
// corner:
//   - TAGS and PAPERS: the number tag and the label (left, top, right, bottom)
//   - LABELS: the part of the label that the name can use
//   - LABEL_CENTERS: the middle of the name
// The tags and the labels have the sizes of the video, and lie a little
// differently on each tape.
const vec4 TAGS[12] = vec4[12](
    vec4(18.0, 9.0, 60.0, 49.0),
    vec4(9.0, 14.0, 50.0, 51.0),
    vec4(16.0, 13.0, 59.0, 51.0),
    vec4(13.0, 14.0, 56.0, 50.0),
    vec4(10.0, 12.0, 60.0, 49.0),
    vec4(12.0, 13.0, 60.0, 51.0),
    vec4(13.0, 14.0, 53.0, 53.0),
    vec4(17.0, 9.0, 57.0, 51.0),
    vec4(14.0, 12.0, 51.0, 52.0),
    vec4(12.0, 17.0, 58.0, 51.0),
    vec4(15.0, 14.0, 54.0, 51.0),
    vec4(11.0, 15.0, 55.0, 51.0)
);
const vec4 PAPERS[12] = vec4[12](
    vec4(69.0, 10.0, 387.0, 51.0),
    vec4(59.0, 13.0, 379.0, 54.0),
    vec4(68.0, 14.0, 386.0, 54.0),
    vec4(65.0, 14.0, 383.0, 54.0),
    vec4(69.0, 12.0, 386.0, 53.0),
    vec4(69.0, 13.0, 386.0, 54.0),
    vec4(62.0, 14.0, 379.0, 54.0),
    vec4(66.0, 12.0, 352.0, 53.0),
    vec4(60.0, 12.0, 378.0, 53.0),
    vec4(67.0, 13.0, 385.0, 54.0),
    vec4(63.0, 14.0, 381.0, 54.0),
    vec4(64.0, 13.0, 381.0, 53.0)
);
const vec4 LABELS[12] = vec4[12](
    vec4(99.0, 10.0, 384.0, 51.0),
    vec4(89.0, 13.0, 376.0, 54.0),
    vec4(98.0, 14.0, 383.0, 54.0),
    vec4(95.0, 14.0, 380.0, 54.0),
    vec4(99.0, 12.0, 383.0, 53.0),
    vec4(99.0, 13.0, 383.0, 54.0),
    vec4(92.0, 14.0, 376.0, 54.0),
    vec4(96.0, 12.0, 349.0, 53.0),
    vec4(90.0, 12.0, 375.0, 53.0),
    vec4(97.0, 13.0, 382.0, 54.0),
    vec4(93.0, 14.0, 378.0, 54.0),
    vec4(94.0, 13.0, 378.0, 53.0)
);
const float LABEL_CENTERS[12] = float[12](
    228.0, 219.0, 227.0, 224.0, 227.5, 227.5, 220.5, 209.0, 219.0, 226.0, 222.0, 222.5
);

// True if `p` is in rectangle `r` (left, top, right, bottom) with its
// corners cut by one art pixel, as pixel art rounds a box.
bool inBox(vec2 p, vec4 r) {
    if (p.x < r.x || p.x >= r.z || p.y < r.y || p.y >= r.w) {
        return false;
    }
    vec2 d = min(p - r.xy, r.zw - p);
    return d.x >= 3.0 || d.y >= 3.0;
}

// The names of the tapes in the video, 12 characters each with 0 after the end.
const int DEFAULT_NAMES[144] = int[144](
    70, 97, 99, 116, 111, 114, 121, 0, 0, 0, 0, 0,  // Factory
    72, 111, 116, 101, 108, 0, 0, 0, 0, 0, 0, 0,  // Hotel
    67, 108, 117, 98, 0, 0, 0, 0, 0, 0, 0, 0,  // Club
    80, 114, 105, 115, 111, 110, 0, 0, 0, 0, 0, 0,  // Prison
    83, 116, 117, 100, 105, 111, 0, 0, 0, 0, 0, 0,  // Studio
    77, 97, 110, 115, 105, 111, 110, 0, 0, 0, 0, 0,  // Mansion
    67, 104, 105, 110, 97, 116, 111, 119, 110, 0, 0, 0,  // Chinatown
    68, 114, 103, 46, 116, 97, 112, 101, 0, 0, 0, 0,  // Drg.tape
    83, 108, 116, 114, 46, 104, 111, 117, 115, 101, 0, 0,  // Sltr.house
    66, 117, 110, 107, 101, 114, 0, 0, 0, 0, 0, 0,  // Bunker
    66, 117, 110, 107, 101, 114, 32, 80, 116, 46, 50, 0,  // Bunker Pt.2
    71, 111, 118, 116, 46, 32, 76, 97, 98, 0, 0, 0   // Govt. Lab
);

// The character at `index` of the label of `tape`: the label of the
// configuration, or else the name in the video. 0 after the end.
int labelChar(int tape, int index) {
    if (iActionLabelLength(tape) > 0) {
        return iActionLabel(tape, index);
    }
    return index < 12 ? DEFAULT_NAMES[(tape - 1) * 12 + index] : 0;
}

int labelLength(int tape) {
    int n = iActionLabelLength(tape);
    if (n > 0) {
        return n;
    }
    for (int i = 0; i < 12; i++) {
        if (DEFAULT_NAMES[(tape - 1) * 12 + i] == 0) {
            return i;
        }
    }
    return 12;
}

// The paper and the ink of the labels. Tape 8 has a yellow label.
const vec3 PAPER = vec3(173.0, 149.0, 178.0) / 255.0;
const vec3 PAPER_YELLOW = vec3(164.0, 156.0, 140.0) / 255.0;
const vec3 INK = vec3(28.0, 9.0, 32.0) / 255.0;

// The characters that go below the baseline.
bool descends(int c) {
    return c == 103 || c == 106 || c == 112 || c == 113 || c == 121 || c == 44 || c == 59
        || c == 0x3B2 || c == 0x3B3 || c == 0x3B6 || c == 0x3B7 || c == 0x3BC || c == 0x3BE
        || c == 0x3C1 || c == 0x3C2 || c == 0x3C6 || c == 0x3C8;
}

// Where the label of `tape` goes. Returns the left and the top of the text in
// scene pixels, the size of a font pixel, and how many characters fit.
vec4 labelLayout(int tape) {
    int n = labelLength(tape);
    vec4 box = LABELS[tape - 1];
    vec2 size = box.zw - box.xy;
    // Letters of 4 scene pixels match the original names. A long label, or
    // the short label of tape 2, uses 3.
    float scale = (size.y >= 36.0 && float(n * 6 - 1) * 4.0 <= size.x - 6.0) ? 4.0 : 3.0;
    int fit = int((size.x - 6.0) / scale + 1.0) / 6;
    n = min(n, fit);
    bool low = false;
    for (int i = 0; i < PHONTO_MAX_LABEL; i++) {
        if (i < n && descends(labelChar(tape, i))) {
            low = true;
        }
    }
    float width = float(n * 6 - 1) * scale;
    float center = clamp(LABEL_CENTERS[tape - 1], box.x + 3.0 + width * 0.5, box.z - 3.0 - width * 0.5);
    // Center the capitals, or the whole glyph if a letter goes below the
    // baseline.
    float rows = low ? 9.0 : 7.0;
    float top = box.y + max(0.0, floor((size.y - rows * scale) * 0.5));
    return vec4(floor(center - width * 0.5), top, scale, float(n));
}

// The index of the glyph of code point `c` in font.png: ASCII, then Greek
// capitals, Greek small letters, and the small vowels with tonos. A capital
// with tonos or dialytika, and a small letter with dialytika, uses the plain
// letter. Other characters show as "?".
int glyphIndex(int c) {
    if (c >= 32 && c <= 126) return c - 32;
    if (c >= 0x391 && c <= 0x3A9 && c != 0x3A2) return 95 + c - 0x391;
    if (c >= 0x3B1 && c <= 0x3C9) return 120 + c - 0x3B1;
    if (c >= 0x3AC && c <= 0x3AF) return 145 + c - 0x3AC;
    if (c >= 0x3CC && c <= 0x3CE) return 149 + c - 0x3CC;
    if (c == 0x386) return 95;
    if (c == 0x388) return 99;
    if (c == 0x389) return 101;
    if (c == 0x38A || c == 0x3AA) return 103;
    if (c == 0x38C) return 109;
    if (c == 0x38E || c == 0x3AB) return 115;
    if (c == 0x38F) return 119;
    if (c == 0x3CA || c == 0x390) return 128;
    if (c == 0x3CB || c == 0x3B0) return 140;
    return 31;
}

// True if pixel (col, row) of glyph `g` is ink. Each glyph is 5x9 pixels in
// a cell of 6x10, 16 cells in a row of font.png.
bool glyphPixel(int g, int col, int row) {
    if (col < 0 || col > 4 || row < 0 || row > 8) {
        return false;
    }
    ivec2 p = ivec2((g % 16) * 6 + col, (g / 16) * 10 + row);
    return texelFetch(iChannel0, ivec2(p.x, 99 - p.y), 0).a > 0.5;
}

// True if a stroke of glyph `g` continues from pixel (col, row) in the
// direction `step`. It can go straight, or along one of the two diagonals next
// to it.
bool continues(int g, int col, int row, ivec2 step) {
    ivec2 side = ivec2(step.y, step.x);
    ivec2 p = ivec2(col, row) + step;
    return glyphPixel(g, p.x, p.y) || glyphPixel(g, p.x + side.x, p.y + side.y)
        || glyphPixel(g, p.x - side.x, p.y - side.y);
}

// How much ink covers scene pixel `p` of the label of `tape`, from 0 to 1. `p`
// is relative to the top left of the text. Four samples smooth the edges, as
// the video does. At the size of the original names, a stroke that no other
// stroke continues is 3 scene pixels wide, not 4, as in those names.
float ink(int tape, vec2 p, float scale, int count) {
    float cover = 0.0;
    for (int i = 0; i < 4; i++) {
        vec2 f = (p + vec2(float(i & 1), float(i >> 1)) * 0.8 - 0.4) / scale;
        if (f.x < 0.0 || f.y < 0.0) {
            continue;
        }
        int cell = int(f.x) / 6;
        int col = int(f.x) - cell * 6;
        int row = int(f.y);
        if (cell >= count) {
            continue;
        }
        int g = glyphIndex(labelChar(tape, cell));
        if (!glyphPixel(g, col, row)) {
            continue;
        }
        vec2 fr = fract(f);
        float trim = scale >= 4.0 ? 0.25 : 0.0;
        if (fr.x > 1.0 - trim && !continues(g, col, row, ivec2(1, 0))) {
            continue;
        }
        if (fr.y > 1.0 - trim && !continues(g, col, row, ivec2(0, 1))) {
            continue;
        }
        cover += 0.25;
    }
    return cover;
}


// The colors of a tape, from the video.
const vec3 OUTLINE = vec3(30.0, 20.0, 45.0) / 255.0;
const vec3 SEAM = vec3(14.0, 5.0, 29.0) / 255.0;
const vec3 BODY = vec3(63.0, 53.0, 78.0) / 255.0;
const vec3 BODY_DARK = vec3(54.0, 44.0, 68.0) / 255.0;
const vec3 BEVEL_DARK = vec3(87.0, 76.0, 100.0) / 255.0;
const vec3 BEVEL = vec3(120.0, 106.0, 130.0) / 255.0;
const vec3 BODY_HIGH = vec3(84.0, 70.0, 95.0) / 255.0;
const vec3 BODY_LOW = vec3(56.0, 45.0, 68.0) / 255.0;
const vec3 BODY_FOOT = vec3(44.0, 34.0, 58.0) / 255.0;
const vec3 CAP_LINE = vec3(108.0, 95.0, 124.0) / 255.0;
const vec3 DIVIDER = vec3(45.0, 36.0, 60.0) / 255.0;
const vec3 PAPER_LIGHT = vec3(192.0, 172.0, 198.0) / 255.0;
const vec3 PAPER_DARK = vec3(148.0, 127.0, 156.0) / 255.0;
const vec3 YELLOW_LIGHT = vec3(186.0, 178.0, 156.0) / 255.0;
const vec3 YELLOW_DARK = vec3(138.0, 130.0, 112.0) / 255.0;
const vec3 SHADOW = vec3(34.0, 24.0, 48.0) / 255.0;
const vec3 ICON = vec3(122.0, 106.0, 134.0) / 255.0;
const vec3 TAG = vec3(204.0, 199.0, 204.0) / 255.0;
const vec3 TAG_SHADE = vec3(156.0, 148.0, 160.0) / 255.0;
const vec3 TAG_EDGE = vec3(146.0, 138.0, 152.0) / 255.0;
const vec3 RED = vec3(164.0, 40.0, 62.0) / 255.0;
const vec3 RED_DARK = vec3(98.0, 16.0, 36.0) / 255.0;
const vec3 RED_BLEED = vec3(204.0, 120.0, 134.0) / 255.0;
const vec3 NOTCH = vec3(12.0, 4.0, 28.0) / 255.0;

float hash(vec2 p) {
    p = fract(p * vec2(123.34, 456.21));
    p += dot(p, p + 45.32);
    return fract(p.x * p.y);
}

// From here on, the sizes are in art pixels. One art pixel is 3 scene pixels,
// and every part of a tape lines up with them.

// The color of the body of a tape at art row `y` of its front face, which is
// `h` rows tall. From the top: a dark seam, a light bevel, the body, and a
// darker foot.
vec3 bodyRow(int y, int h) {
    if (y == 0) return SEAM;
    if (y == 1) return BEVEL;
    if (y == 2) return BODY_HIGH;
    if (y == h - 1) return OUTLINE;
    if (y == h - 2) return BODY_FOOT;
    if (y == h - 3) return BODY_LOW;
    return BODY;
}

// The strokes of the digits, as written with a marker. Each segment goes from
// (x0, y0) to (x1, y1) in a unit box, with x to the right and y down. The
// segments of digit d are DIGIT_STROKE_START[d] to DIGIT_STROKE_START[d + 1].
const vec4 DIGIT_STROKES[65] = vec4[65](
    vec4(0.50, 0.05, 0.82, 0.20),
    vec4(0.82, 0.20, 0.90, 0.50),
    vec4(0.90, 0.50, 0.80, 0.82),
    vec4(0.80, 0.82, 0.50, 0.96),
    vec4(0.50, 0.96, 0.20, 0.84),
    vec4(0.20, 0.84, 0.10, 0.50),
    vec4(0.10, 0.50, 0.20, 0.16),
    vec4(0.20, 0.16, 0.50, 0.05),
    vec4(0.28, 0.28, 0.56, 0.05),
    vec4(0.56, 0.05, 0.56, 0.95),
    vec4(0.28, 0.95, 0.82, 0.94),
    vec4(0.14, 0.26, 0.34, 0.07),
    vec4(0.34, 0.07, 0.64, 0.05),
    vec4(0.64, 0.05, 0.85, 0.22),
    vec4(0.85, 0.22, 0.80, 0.45),
    vec4(0.80, 0.45, 0.50, 0.68),
    vec4(0.50, 0.68, 0.14, 0.95),
    vec4(0.14, 0.95, 0.88, 0.92),
    vec4(0.14, 0.12, 0.60, 0.05),
    vec4(0.60, 0.05, 0.86, 0.22),
    vec4(0.86, 0.22, 0.62, 0.45),
    vec4(0.62, 0.45, 0.42, 0.48),
    vec4(0.62, 0.45, 0.88, 0.66),
    vec4(0.88, 0.66, 0.76, 0.90),
    vec4(0.76, 0.90, 0.45, 0.97),
    vec4(0.45, 0.97, 0.12, 0.86),
    vec4(0.66, 0.96, 0.66, 0.05),
    vec4(0.66, 0.05, 0.10, 0.66),
    vec4(0.10, 0.66, 0.92, 0.66),
    vec4(0.86, 0.06, 0.26, 0.06),
    vec4(0.26, 0.06, 0.20, 0.45),
    vec4(0.20, 0.45, 0.55, 0.40),
    vec4(0.55, 0.40, 0.86, 0.56),
    vec4(0.86, 0.56, 0.86, 0.80),
    vec4(0.86, 0.80, 0.55, 0.97),
    vec4(0.55, 0.97, 0.14, 0.88),
    vec4(0.80, 0.08, 0.45, 0.10),
    vec4(0.45, 0.10, 0.20, 0.40),
    vec4(0.20, 0.40, 0.15, 0.70),
    vec4(0.15, 0.70, 0.35, 0.95),
    vec4(0.35, 0.95, 0.70, 0.95),
    vec4(0.70, 0.95, 0.86, 0.72),
    vec4(0.86, 0.72, 0.70, 0.50),
    vec4(0.70, 0.50, 0.40, 0.50),
    vec4(0.40, 0.50, 0.18, 0.66),
    vec4(0.12, 0.08, 0.88, 0.07),
    vec4(0.88, 0.07, 0.46, 0.95),
    vec4(0.30, 0.52, 0.76, 0.50),
    vec4(0.50, 0.05, 0.80, 0.20),
    vec4(0.80, 0.20, 0.50, 0.45),
    vec4(0.50, 0.45, 0.20, 0.20),
    vec4(0.20, 0.20, 0.50, 0.05),
    vec4(0.50, 0.45, 0.86, 0.70),
    vec4(0.86, 0.70, 0.50, 0.96),
    vec4(0.50, 0.96, 0.14, 0.70),
    vec4(0.14, 0.70, 0.50, 0.45),
    vec4(0.82, 0.38, 0.60, 0.50),
    vec4(0.60, 0.50, 0.30, 0.48),
    vec4(0.30, 0.48, 0.15, 0.28),
    vec4(0.15, 0.28, 0.35, 0.06),
    vec4(0.35, 0.06, 0.70, 0.06),
    vec4(0.70, 0.06, 0.86, 0.30),
    vec4(0.86, 0.30, 0.80, 0.70),
    vec4(0.80, 0.70, 0.55, 0.95),
    vec4(0.55, 0.95, 0.24, 0.90)
);
const int DIGIT_STROKE_START[11] = int[11](0, 8, 11, 18, 26, 29, 36, 45, 48, 56, 65);

// A smooth value noise from 0 to 1 along a line.
float noise1(float x, float seed) {
    float i = floor(x);
    float f = fract(x);
    return mix(hash(vec2(i, seed)), hash(vec2(i + 1.0, seed)), f * f * (3.0 - 2.0 * f));
}

// A number tag is a scrap of paper, stuck on a little crooked, with torn
// edges. `tagFrame` turns scene pixel `s` into the frame of the paper of
// `tape`: the offset from its middle, along its own edges.
vec2 tagFrame(int tape, vec2 s, out vec2 halfSize) {
    vec4 r = TAGS[tape - 1];
    halfSize = (r.zw - r.xy) * 0.5 - 1.5;
    float angle = (hash(vec2(float(tape) * 3.71, 2.0)) - 0.5) * 0.16;
    vec2 d = s - (r.xy + r.zw) * 0.5;
    float c = cos(angle);
    float n = sin(angle);
    return vec2(c * d.x + n * d.y, -n * d.x + c * d.y);
}

// True if scene point `s` is on the paper of the tag of `tape`. Each edge
// goes in and out by a few pixels, as torn paper does.
bool onTag(int tape, vec2 s) {
    vec2 h;
    vec2 l = tagFrame(tape, s, h);
    float seed = float(tape) * 5.17;
    float top = h.y + (noise1(l.x / 6.0, seed) - 0.6) * 6.0;
    float bottom = h.y + (noise1(l.x / 6.0, seed + 11.0) - 0.6) * 6.0;
    float left = h.x + (noise1(l.y / 6.0, seed + 23.0) - 0.6) * 6.0;
    float right = h.x + (noise1(l.y / 6.0, seed + 37.0) - 0.6) * 6.0;
    return l.x > -left && l.x < right && l.y > -top && l.y < bottom;
}

float segmentDistance(vec2 p, vec2 a, vec2 b) {
    vec2 ab = b - a;
    float t = clamp(dot(p - a, ab) / max(dot(ab, ab), 1e-4), 0.0, 1.0);
    return length(p - a - ab * t);
}

// The red number of `tape` at scene pixel `s`, written with a marker. The
// strokes lean to the right, a little differently on each tag. The pen presses
// harder in some places, and the ink bleeds a little at the edges. Returns the
// cover (x) and how far inside the stroke the pixel is (y), from 0 at the edge
// to 1 in the middle.
vec2 tagInk(int tape, vec2 s) {
    vec2 h;
    vec2 l = tagFrame(tape, s, h);
    bool one = tape < 10;
    float height = 2.0 * h.y * 0.8;
    float width = height * (one ? 0.7 : 0.5);
    float gap = height * 0.08;
    float total = one ? width : width * 2.0 + gap;
    float best = 1e4;
    for (int i = 0; i < 2; i++) {
        if (one && i == 1) {
            break;
        }
        int digit = one ? tape : (i == 0 ? 1 : tape - 10);
        vec2 origin = vec2(-total * 0.5 + float(i) * (width + gap), -height * 0.5);
        vec2 p = l - origin;
        for (int k = DIGIT_STROKE_START[digit]; k < DIGIT_STROKE_START[digit + 1]; k++) {
            vec4 seg = DIGIT_STROKES[k];
            // The hand shakes: each point of a stroke moves a little. A point
            // that two segments share moves the same way for both.
            vec2 a = seg.xy;
            vec2 b = seg.zw;
            float seed = float(tape * 13 + i * 5);
            a += (vec2(hash(floor(a * 20.0) + seed), hash(floor(a * 20.0) + seed + 3.1)) - 0.5) * 0.09;
            b += (vec2(hash(floor(b * 20.0) + seed), hash(floor(b * 20.0) + seed + 3.1)) - 0.5) * 0.09;
            // The writing leans to the right.
            a.x += (0.5 - a.y) * 0.18;
            b.x += (0.5 - b.y) * 0.18;
            best = min(best, segmentDistance(p, a * vec2(width, height), b * vec2(width, height)));
        }
    }
    // The width of the stroke changes with the pressure of the pen, and its
    // edge is a little rough.
    float pressure = 0.75 + 0.5 * noise1(dot(l, vec2(0.11, 0.07)), float(tape) * 2.3);
    float thick = height * 0.12 * pressure;
    best += (hash(floor(s / 1.5) + float(tape)) - 0.5) * 1.4;
    float cover = 1.0 - smoothstep(thick, thick + 1.5, best);
    float inside = 1.0 - clamp(best / max(thick, 0.1), 0.0, 1.0);
    return vec2(cover, inside);
}

// The tag of `tape` at scene pixel `s`, drawn in art pixels. It has paper, an
// outline, a shadow on the box, and the red number. `seam` is the row of the
// seam of the box behind it. Returns alpha 0 where there is no tag.
vec4 numberTag(int tape, vec2 s, vec2 art, float seam) {
    // The paper and its edges are in art pixels. `art` is the middle of the
    // art pixel of `s`.
    bool on = onTag(tape, art);
    if (!on) {
        return onTag(tape, art - vec2(3.0)) ? vec4(SHADOW, 1.0) : vec4(0.0);
    }
    bool edge = !onTag(tape, art + vec2(3.0, 0.0)) || !onTag(tape, art - vec2(3.0, 0.0))
        || !onTag(tape, art + vec2(0.0, 3.0)) || !onTag(tape, art - vec2(0.0, 3.0));
    vec2 h;
    vec2 l = tagFrame(tape, art, h);
    vec2 u = l / h;
    // Light at the top left, grayer to the bottom right, with blotches and
    // specks of dirt.
    vec3 c = mix(TAG, TAG_SHADE, smoothstep(-0.3, 1.2, (u.x + u.y) * 0.5));
    c *= 0.94 + 0.08 * noise1(art.x / 9.0 + art.y / 13.0, float(tape) * 4.1);
    float speck = hash(floor(art / 3.0) + float(tape) * 1.9);
    if (speck < 0.1) {
        c *= 0.88;
    }
    if (edge) {
        c = TAG_EDGE;
    }
    // The seam of the box shows through the thin paper.
    if (abs(art.y - seam) < 2.0) {
        c *= 0.78;
    }
    // The ink: dark red in the middle of a stroke, red, and pink where it
    // bleeds into the paper.
    vec2 ink = tagInk(tape, s);
    if (ink.x > 0.0) {
        vec3 red = mix(RED_BLEED, RED, smoothstep(0.0, 0.35, ink.y));
        red = mix(red, RED_DARK, smoothstep(0.45, 0.9, ink.y));
        c = mix(c, red, ink.x * (edge ? 0.6 : 1.0));
    }
    return vec4(c, 1.0);
}

// The reel icon at the left of a label: a narrow capsule and three marks.
// `a` is the art pixel in the label, `h` its height in art pixels.
bool reelIcon(ivec2 a, int h) {
    bool capsule = a.x >= 2 && a.x <= 5 && a.y >= 2 && a.y <= h - 3;
    bool inside = a.x >= 3 && a.x <= 4 && a.y >= 3 && a.y <= h - 4;
    bool round = (a.x == 2 || a.x == 5) && (a.y == 2 || a.y == h - 3);
    if (capsule && !inside && !round) {
        return true;
    }
    // The marks: a wavy column, one art pixel wide, that steps left and
    // right.
    int mid = h / 2;
    if (a.y < mid - 4 || a.y > mid + 4) {
        return false;
    }
    int x = 7 + ((a.y - mid + 8) / 2) % 2;
    return a.x == x;
}

// The front face of `tape` at `s`, in pixels from its top left corner, or
// alpha 0 off the face.
vec4 frontFace(int tape, vec2 s) {
    vec4 f = vec4(0.0, 0.0, TAPE_W, PLACES[tape - 1].z);
    if (!inBox(s, f)) {
        return vec4(0.0);
    }
    ivec2 a = ivec2(floor((s - f.xy) / 3.0));
    ivec2 n = ivec2(floor((f.zw - f.xy) / 3.0));
    if (a.x >= n.x) a.x = n.x - 1;
    if (a.y >= n.y) a.y = n.y - 1;
    vec3 c = bodyRow(a.y, n.y);
    // The ends of the box: a light bevel at the left, the outline.
    if (a.x == 0 || a.x == n.x - 1) {
        c = OUTLINE;
    } else if (a.x == 1 && a.y > 0) {
        c = mix(c, BEVEL, 0.45);
    }
    vec4 tagRect = TAGS[tape - 1];
    vec4 paper = PAPERS[tape - 1];
    ivec2 t0 = ivec2(floor((tagRect.xy - f.xy) / 3.0 + 0.5));
    ivec2 tn = ivec2(floor((tagRect.zw - tagRect.xy) / 3.0 + 0.5));
    ivec2 p0 = ivec2(floor((paper.xy - f.xy) / 3.0 + 0.5));
    ivec2 pn = ivec2(floor((paper.zw - paper.xy) / 3.0 + 0.5));
    int mid = p0.y + pn.y / 2;
    // The end cap at the right: a light edge line, and the box past it a
    // little darker, as its side turns away.
    int cap = n.x - 9;
    if (a.y > 1 && a.y < n.y - 2 && a.x > 0 && a.x < n.x - 1) {
        if (a.x == cap) {
            c = CAP_LINE;
        } else if (a.x > cap) {
            c = mix(c, BODY_DARK, 0.6);
        }
    }
    // The notch: a dark slot from the label to the end cap, and its lit
    // lower edge.
    int px1 = p0.x + pn.x;
    if (a.x > px1 && a.x < cap) {
        if (a.y == mid) {
            c = NOTCH;
        } else if (a.y == mid + 1) {
            c = BEVEL_DARK;
        }
    }
    // A long left end has the folds of the sleeve of the box.
    if (t0.x > 13 && a.y > 2 && a.y < n.y - 3 && (a.x == t0.x - 5 || a.x == t0.x - 9)) {
        c = DIVIDER;
    }
    // The dash from the tag to the label.
    if (a.x >= t0.x + tn.x && a.x < p0.x && a.y == mid) {
        c = NOTCH;
    }
    // The shadow that the label throws on the box.
    ivec2 ps = a - p0 - ivec2(1, 1);
    if (all(greaterThanEqual(ps, ivec2(0))) && all(lessThan(ps, pn))) {
        c = SHADOW;
    }
    // The number tag, with its own shadow.
    if (s.x >= tagRect.x - 9.0 && s.x < tagRect.z + 9.0 && s.y >= tagRect.y - 9.0 && s.y < tagRect.w + 9.0) {
        vec2 art = f.xy + (vec2(a) + 0.5) * 3.0;
        float seam = f.y + float(mid) * 3.0 + 1.5;
        vec4 tag = numberTag(tape, s, art, seam);
        if (tag.a > 0.0) {
            c = tag.rgb;
        }
    }
    // The label: paper with light top and left edges, darker bottom and right
    // edges, cut corners, the reel icon, and the name.
    ivec2 la = a - p0;
    if (all(greaterThanEqual(la, ivec2(0))) && all(lessThan(la, pn))) {
        bool corner = (la.x == 0 || la.x == pn.x - 1) && (la.y == 0 || la.y == pn.y - 1);
        if (!corner) {
            bool yellow = tape == 8;
            vec3 base = yellow ? PAPER_YELLOW : PAPER;
            if (la.y == 0 || la.x == 0) {
                c = yellow ? YELLOW_LIGHT : PAPER_LIGHT;
            } else if (la.y == pn.y - 1 || la.x == pn.x - 1) {
                c = yellow ? YELLOW_DARK : PAPER_DARK;
            } else {
                c = base;
            }
            if (reelIcon(la, pn.y)) {
                c = yellow ? mix(ICON, PAPER_YELLOW, 0.3) : ICON;
            }
            vec4 box = LABELS[tape - 1];
            if (s.x >= box.x && s.x < box.z && s.y >= box.y + 3.0 && s.y < box.w - 3.0) {
                vec4 text = labelLayout(tape);
                c = mix(c, INK, ink(tape, s - text.xy, text.z, int(text.w)));
            }
        }
    }
    return vec4(c, 1.0);
}

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    ivec2 texel = ivec2(fragCoord);
    int tape = 12 - texel.y / 62;
    vec2 s = vec2(fragCoord.x - 2.0, float((13 - tape) * 62 - 1 - texel.y) + 0.5);
    fragColor = frontFace(tape, s);
}
