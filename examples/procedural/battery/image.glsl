// The battery as the tide in a glass, in the colors of museslabs.org.
//
// iBattery.x = the charge, from 0 to 1.
// iBattery.y = 1 while the battery charges.
// iBattery.z = 1 on AC power.
// iBattery.w = 1 if the system has a battery.
// iChannel0 = labels.png, a text atlas (see the text helpers below).

// 1 = cycle through the states of a battery, to preview them with
// `phonto dump`.
#define DEMO 0

const vec3 LIGHT_PAPER = vec3(0.966, 0.924, 0.842);
const vec3 LIGHT_INK = vec3(0.140, 0.105, 0.082);
const vec3 DARK_PAPER = vec3(0.104, 0.081, 0.066);
const vec3 DARK_INK = vec3(0.936, 0.920, 0.888);
const vec3 TERRA = vec3(0.785, 0.356, 0.197);

// Below this charge, the water turns terracotta.
const float LOW = 0.2;

// The glass, in display heights from the middle of the display.
const vec2 GLASS = vec2(0.1, 0.21);   // half width, half height
const float GLASS_Y = 0.04;           // the middle of the glass
const float RADIUS = 0.03;

float hash(float n) {
    return fract(sin(n * 127.1) * 43758.5453);
}

vec4 battery() {
#if DEMO
    // 12 seconds for each state: charging, plugged in, low, and no battery.
    float t = iTime;
    int state = int(mod(t / 12.0, 4.0));
    float charge = 0.15 + 0.8 * fract(t / 12.0);
    if (state == 0) return vec4(charge, 1.0, 1.0, 1.0);
    if (state == 1) return vec4(1.0, 0.0, 1.0, 1.0);
    if (state == 2) return vec4(0.12, 0.0, 0.0, 1.0);
    return vec4(1.0, 0.0, 1.0, 0.0);
#else
    return iBattery;
#endif
}

// ---- text -------------------------------------------------------------------

// labels.png: row 0 has one cell for each glyph, 0 to 9 and %. Each later
// row has one word. The sizes are in pixels of the atlas.
const vec2 ATLAS = vec2(916.0, 720.0);
const float ROW = 144.0;
const float CELL = 83.0;
// The advance of each glyph, in row heights.
const float ADVANCE[11] = float[11](0.396, 0.285, 0.347, 0.375, 0.375, 0.347, 0.368,
    0.347, 0.375, 0.361, 0.576);
const int PERCENT = 10;
// The words, and their widths in row heights.
const int CHARGING = 0;
const int ON_BATTERY = 1;
const int PLUGGED_IN = 2;
const int NO_BATTERY = 3;
const float WORD[4] = float[4](2.694, 3.132, 3.097, 3.132);

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

// The coverage of word w, centered on p = 0.
float word(vec2 p, int w) {
    p.x += 0.5 * WORD[w];
    float a = texture(iChannel0, (vec2(0.0, ATLAS.y - ROW * float(w + 2)) + p * ROW) / ATLAS).a;
    if (p.x < 0.0 || p.x > WORD[w] || p.y < 0.0 || p.y > 1.0) {
        return 0.0;
    }
    return a;
}

// The coverage of `n` and a percent sign, centered on p = 0.
float percent(vec2 p, int n) {
    int digits[3] = int[3](n / 100, n / 10 % 10, n % 10);
    int first = n >= 100 ? 0 : n >= 10 ? 1 : 2;
    float width = ADVANCE[PERCENT];
    for (int i = first; i < 3; i++) {
        width += ADVANCE[digits[i]];
    }
    float x = -0.5 * width;
    float ink = 0.0;
    for (int i = first; i < 3; i++) {
        ink = max(ink, glyph(p - vec2(x, 0.0), digits[i]));
        x += ADVANCE[digits[i]];
    }
    return max(ink, glyph(p - vec2(x, 0.0), PERCENT));
}

// ---- the glass ----------------------------------------------------------------

float roundedBox(vec2 p, vec2 half_, float r) {
    vec2 d = abs(p) - half_ + r;
    return length(max(d, 0.0)) + min(max(d.x, d.y), 0.0) - r;
}

// The display nearest the middle of the image. A spanned wallpaper shows the
// glass there.
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
    vec4 b = battery();
    bool hasBattery = b.w > 0.5;
    float charge = hasBattery ? clamp(b.x, 0.0, 1.0) : 1.0;
    bool charging = b.y > 0.5;
    bool low = hasBattery && !charging && charge < LOW;

    vec4 display = stage();
    float height = min(display.w, display.z * 1.4);
    vec2 p = (coord - display.xy - 0.5 * display.zw) / height;
    float px = 1.0 / height;
    float t = iTime;

    vec2 uv = coord / iResolution.xy;
    vec3 col = paper * (1.0 - 0.05 * smoothstep(0.3, 1.2, length((uv - 0.5) * vec2(1.6, 1.2))));
    col += (fract(sin(dot(floor(coord), vec2(12.9898, 78.233))) * 43758.5453) - 0.5) * 0.012;

    // The water. Its surface moves more while the battery charges.
    vec2 g = p - vec2(0.0, GLASS_Y);
    vec2 inner = GLASS - 0.014;
    float bottom = -inner.y;
    float level = bottom + charge * 2.0 * inner.y;
    float wave = charging ? 0.006 : 0.0025;
    float surface = level + wave * (sin(g.x * 34.0 + t * 1.7) + 0.6 * sin(g.x * 61.0 - t * 2.3));
    float inside = 1.0 - smoothstep(-px, px, roundedBox(g, inner, RADIUS - 0.012));
    float water = inside * (1.0 - smoothstep(-px, px, g.y - surface));
    float depth = clamp((surface - g.y) / (2.0 * inner.y), 0.0, 1.0);
    float strength = mix(0.62, 0.32, depth);
    if (low) {
        // Low water turns terracotta and breathes slowly.
        strength += 0.25 + 0.1 * sin(t * 2.0);
    }
    col = mix(col, mix(paper, TERRA, strength), water);
    // A bright line on the surface.
    float line = inside * (1.0 - smoothstep(0.0, 1.5 * px, abs(g.y - surface)));
    col = mix(col, mix(paper, TERRA, 0.95), line * step(0.001, charge));

    // Bubbles rise while the battery charges.
    if (charging) {
        for (int i = 0; i < 14; i++) {
            float fi = float(i);
            float speed = 0.04 + 0.05 * hash(fi);
            float rise = mod(t * speed + hash(fi + 7.0), 1.0);
            vec2 at = vec2((hash(fi + 3.0) - 0.5) * 1.6 * inner.x + 0.004 * sin(t * 3.0 + fi),
                bottom + rise * (level - bottom));
            float r = 0.003 + 0.004 * hash(fi + 11.0);
            float ring = 1.0 - smoothstep(0.0, 1.2 * px, abs(length(g - at) - r));
            col = mix(col, paper, ring * water * 0.8 * sin(rise * 3.14159));
        }
    }

    // The glass and its cap, drawn in ink like the rules of the site.
    float glass = abs(roundedBox(g, GLASS, RADIUS)) - 0.0022;
    float cap = roundedBox(g - vec2(0.0, GLASS.y + 0.018), vec2(0.035, 0.012), 0.006);
    float outline = min(glass, max(cap, -roundedBox(g, GLASS, RADIUS)));
    col = mix(col, ink, 1.0 - smoothstep(-px, px, outline));

    // The charge and the state, under the glass.
    vec2 label = p - vec2(0.0, GLASS_Y - GLASS.y - 0.08);
    float big = 0.055;
    if (hasBattery) {
        col = mix(col, ink, percent(label / big + vec2(0.0, 0.3), int(round(charge * 100.0))));
    }
    int state = !hasBattery ? NO_BATTERY : charging ? CHARGING : b.z > 0.5 ? PLUGGED_IN : ON_BATTERY;
    float small = 0.032;
    vec2 caption = (label + vec2(0.0, hasBattery ? 0.045 : 0.0)) / small + vec2(0.0, 0.3);
    col = mix(col, mix(paper, ink, low ? 1.0 : 0.6), word(caption, state));
    color = vec4(col, 1.0);
}
