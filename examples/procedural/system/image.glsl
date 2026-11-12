// The load of the computer as an orrery, in the colors of museslabs.org.
//
// iInput.load = the load averages of 1, 5, and 15 minutes.
// iInput.cores = the number of cores.
// iInput.disk = how full the disk is, in percent.
// iChannel0 = labels.png, a text atlas (see the text helpers below).
// iChannel1 = the orbits buffer (see orbits.glsl).

const vec3 LIGHT_PAPER = vec3(0.966, 0.924, 0.842);
const vec3 LIGHT_INK = vec3(0.140, 0.105, 0.082);
const vec3 DARK_PAPER = vec3(0.104, 0.081, 0.066);
const vec3 DARK_INK = vec3(0.936, 0.920, 0.888);
const vec3 TERRA = vec3(0.785, 0.356, 0.197);
const float PI = 3.14159265;

// The radius of each orbit, and of the disk ring, in display heights.
const float ORBIT[3] = float[3](0.11, 0.17, 0.23);
const float DISK_RING = 0.3;

// ---- text -------------------------------------------------------------------

// labels.png: row 0 has one cell for each glyph: 0 to 9, '.', '%', and a
// middle dot. Each later row has one word. The sizes are in pixels.
const vec2 ATLAS = vec2(2384.0, 864.0);
const float ROW = 144.0;
const float CELL = 83.0;
const float ADVANCE[13] = float[13](0.396, 0.285, 0.347, 0.375, 0.375, 0.347, 0.368,
    0.347, 0.375, 0.361, 0.229, 0.576, 0.229);
const int DOT = 10;
const int PERCENT = 11;
const int MIDDLE_DOT = 12;
const int LOAD = 0;
const int ON = 1;
const int CORES = 2;
const int DISK = 3;
const int SETUP = 4;
const float WORD[5] = float[5](1.306, 0.764, 1.562, 1.243, 16.542);
// The space between two words, in row heights.
const float SPACE = 0.28;

// The coverage of glyph g. p is in row heights from the bottom left of the
// advance of the glyph.
float glyph(vec2 p, int g) {
    float cell = CELL / ROW;
    vec2 q = vec2(p.x + 0.5 * (cell - ADVANCE[g]), p.y);
    // Sample before the test. After an early return, the GPU has no texture
    // coordinates for the pixels that left. On some GPUs the mipmap level
    // then goes wrong at the edge, and a faint box outlines the cell.
    float a = texture(iChannel0, (vec2(float(g) * CELL, ATLAS.y - ROW) + q * ROW) / ATLAS).a;
    if (q.x < 0.0 || q.x > cell || q.y < 0.0 || q.y > 1.0) {
        return 0.0;
    }
    return a;
}

// The coverage of word w. p is in row heights from its bottom left.
float word(vec2 p, int w) {
    float a = texture(iChannel0, (vec2(0.0, ATLAS.y - ROW * float(w + 2)) + p * ROW) / ATLAS).a;
    if (p.x < 0.0 || p.x > WORD[w] || p.y < 0.0 || p.y > 1.0) {
        return 0.0;
    }
    return a;
}

// The glyphs of `v`, with 0 or 2 decimals. Returns how many there are.
int digits(float v, int decimals, out int g[6]) {
    float scale = decimals == 2 ? 100.0 : 1.0;
    int n = int(round(clamp(v, 0.0, 999.0) * scale));
    int whole = n / int(scale);
    int count = 0;
    if (whole >= 100) g[count++] = whole / 100;
    if (whole >= 10) g[count++] = whole / 10 % 10;
    g[count++] = whole % 10;
    if (decimals == 2) {
        g[count++] = DOT;
        g[count++] = n / 10 % 10;
        g[count++] = n % 10;
    }
    return count;
}

// A line of text is a list of items: words, numbers, and single glyphs.
// Each `write` function draws an item at x, and moves x past it. A line
// draws twice. The first time, it draws far away from p, so it only measures
// its width. The second time, it draws centered on p.
float coverage;

void writeWord(vec2 p, inout float x, int w) {
    coverage = max(coverage, word(p - vec2(x, 0.0), w));
    x += WORD[w] + SPACE;
}

void writeGlyph(vec2 p, inout float x, int g) {
    coverage = max(coverage, glyph(p - vec2(x, 0.0), g));
    x += ADVANCE[g] + SPACE;
}

void writeNumber(vec2 p, inout float x, float v, int decimals) {
    int g[6];
    int count = digits(v, decimals, g);
    for (int i = 0; i < 6; i++) {
        if (i < count) {
            coverage = max(coverage, glyph(p - vec2(x, 0.0), g[i]));
            x += ADVANCE[g[i]];
        }
    }
    x += SPACE;
}

// "load 1.38 · 1.45 · 1.48 on 12 cores", centered on p.x = 0.
float loadLine(vec2 p) {
    vec3 load = iInput.load;
    float x = 0.0;
    coverage = 0.0;
    for (int pass = 0; pass < 2; pass++) {
        vec2 at = pass == 0 ? vec2(-1e4) : p + vec2(0.5 * x, 0.0);
        x = 0.0;
        writeWord(at, x, LOAD);
        writeNumber(at, x, load.x, 2);
        writeGlyph(at, x, MIDDLE_DOT);
        writeNumber(at, x, load.y, 2);
        writeGlyph(at, x, MIDDLE_DOT);
        writeNumber(at, x, load.z, 2);
        writeWord(at, x, ON);
        writeNumber(at, x, iInput.cores, 0);
        writeWord(at, x, CORES);
        x -= SPACE;
    }
    return coverage;
}

// "disk 82%", centered on p.x = 0.
float diskLine(vec2 p) {
    float x = 0.0;
    coverage = 0.0;
    for (int pass = 0; pass < 2; pass++) {
        vec2 at = pass == 0 ? vec2(-1e4) : p + vec2(0.5 * x, 0.0);
        x = 0.0;
        writeWord(at, x, DISK);
        writeNumber(at, x, iInput.disk, 0);
        x -= SPACE;
        writeGlyph(at, x, PERCENT);
        x -= SPACE;
    }
    return coverage;
}

// ---- drawing -------------------------------------------------------------------

// The coverage of a line of `width` at a distance of `d`.
float line(float d, float width, float px) {
    return clamp((0.5 * width - abs(d)) / px + 0.5, 0.0, 1.0);
}

// The display nearest the middle of the image. A spanned wallpaper shows the
// orrery there.
vec4 stage() {
    vec4 best = iDisplays[0];
    float nearest = 1e9;
    for (int i = 0; i < 8; i++) {
        vec4 d = iDisplays[i];
        float distance = length(d.xy + 0.5 * d.zw - 0.5 * iResolution.xy);
        if (i < iDisplayCount && distance < nearest - 1.0) {
            best = d;
            nearest = distance;
        }
    }
    return best;
}

void mainImage(out vec4 color, in vec2 coord) {
    vec3 paper = mix(LIGHT_PAPER, DARK_PAPER, iColorScheme);
    vec3 ink = mix(LIGHT_INK, DARK_INK, iColorScheme);
    vec3 rule = mix(paper, ink, 0.24);

    vec4 display = stage();
    float height = min(display.w, display.z * 1.4);
    vec2 p = (coord - display.xy - 0.5 * display.zw) / height - vec2(0.0, 0.05);
    float px = 1.0 / height;
    float radius = length(p);
    // The angle from the top, clockwise, from 0 to 1.
    float turn = fract(atan(p.x, p.y) / (2.0 * PI) + 1.0);

    vec2 uv = coord / iResolution.xy;
    vec3 col = paper * (1.0 - 0.05 * smoothstep(0.3, 1.2, length((uv - 0.5) * vec2(1.6, 1.2))));
    col += (fract(sin(dot(floor(coord), vec2(12.9898, 78.233))) * 43758.5453) - 0.5) * 0.012;
    bool configured = iInput.cores > 0.5;

    // The orbits, and a planet on each. A trail follows the planet. It is
    // longer when the load is higher.
    float hairline = max(px, 0.0012);
    for (int i = 0; i < 3; i++) {
        vec4 orbit = texelFetch(iChannel1, ivec2(i, 0), 0);
        float r = ORBIT[i];
        col = mix(col, rule, line(radius - r, hairline, px));
        float angle = orbit.x;
        vec2 planet = r * vec2(sin(angle), cos(angle));
        float behind = fract(angle / (2.0 * PI) - turn);
        float trail = 0.08 + 0.4 * orbit.y;
        float fade = (1.0 - smoothstep(0.0, trail, behind)) * step(behind, trail);
        col = mix(col, TERRA, line(radius - r, 0.004, px) * fade * 0.6);
        float body = 1.0 - smoothstep(-px, px, length(p - planet) - 0.009);
        col = mix(col, TERRA, body);
    }

    // The sun in the middle grows a little with the load of the last minute.
    float glow = texelFetch(iChannel1, ivec2(0, 0), 0).y;
    col = mix(col, mix(paper, TERRA, 0.35), exp(-radius * radius / (0.0012 + 0.0025 * glow)) * 0.6);
    col = mix(col, ink, 1.0 - smoothstep(-px, px, radius - 0.014));

    // The disk ring: full to the angle of the used part.
    float used = clamp(iInput.disk / 100.0, 0.0, 1.0);
    col = mix(col, rule, line(radius - DISK_RING, hairline, px));
    col = mix(col, ink, line(radius - DISK_RING, 0.006, px) * step(turn, used));

    // The caption, in the style of the figure captions of the site.
    float size = 0.03;
    vec2 caption = (p - vec2(0.0, -DISK_RING - 0.09)) / size;
    if (configured) {
        col = mix(col, ink, loadLine(caption));
        col = mix(col, mix(paper, ink, 0.6), diskLine(caption + vec2(0.0, 1.3)));
    } else {
        col = mix(col, mix(paper, ink, 0.6), word(caption + vec2(0.5 * WORD[SETUP], 0.0), SETUP));
    }
    color = vec4(col, 1.0);
}
