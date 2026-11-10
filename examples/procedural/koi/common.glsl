// Koi pond: constants and helpers for every pass.
//
// All passes bind the same channels, so these helpers can read them:
// iChannel0 = koi state (fish.glsl), iChannel1 = water height (water-a.glsl,
// water-b.glsl). The image pass also binds iChannel2 = tile masks
// (tiles.glsl), iChannel3 = pond bed (bed.glsl), and iChannel4 = caustics.
//
// Positions are in pond units: the screen is 1 unit tall, and its width is
// its aspect ratio. The pond looks the same at all resolutions.

// Set an hour (0 to 24) to preview its light. -1 follows the clock.
#define HOUR_OVERRIDE -1.0

const float PI = 3.14159265;
const float TAU = 6.28318531;

const int NUM_FISH = 12;
// Koi i is in column i. Row 0 = nose, heading, speed. Row 1 = turn rate,
// tail phase, depth, effort. Row 2 = how much the body bends (smoothed).
// Columns NUM_FISH to NUM_FISH + 2 of row 0 give the draw order of the koi.
// Column CONTROL, row 0 = the food from the last click: xy, the time it
// dropped, and 1 after the first click. Row 1 = the position of the pressed
// button in the last frame (xy) and this frame (zw), -1 if not pressed.
const int CONTROL = 15;
// How many seconds the koi stay at a click.
const float FOOD_TIME = 8.0;

// Varieties: 0 kohaku, 1 sanke, 2 showa, 3 tancho, 4 yamabuki (gold),
// 5 platinum, 6 asagi, 7 chagoi, 8 orenji (orange).
const int VARIETY[NUM_FISH] = int[NUM_FISH](0, 1, 2, 4, 0, 6, 3, 1, 5, 2, 7, 8);
// Length from nose to tail root, in pond units. Chagoi are big.
const float LENGTH[NUM_FISH] = float[NUM_FISH](
    0.135, 0.115, 0.14, 0.125, 0.105, 0.13, 0.115, 0.11, 0.12, 0.13, 0.175, 0.115);

// ---- hashing and noise ----------------------------------------------------

uint hashU(uint x) {
    x ^= x >> 16;
    x *= 0x7feb352du;
    x ^= x >> 15;
    x *= 0x846ca68bu;
    x ^= x >> 16;
    return x;
}

float hash1(float x) {
    return float(hashU(floatBitsToUint(x))) / 4294967295.0;
}

float hashI(ivec2 c) {
    return float(hashU(uint(c.x) * 0x9e3779b9u ^ hashU(uint(c.y)))) / 4294967295.0;
}

vec2 hash2(ivec2 c) {
    uint h = hashU(uint(c.x) * 0x9e3779b9u ^ hashU(uint(c.y) + 0x632be5abu));
    return vec2(float(h & 0xffffu), float(h >> 16)) / 65535.0;
}

// Smooth value noise, 0 to 1.
float noise(vec2 p) {
    vec2 i = floor(p);
    vec2 f = fract(p);
    vec2 u = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    ivec2 c = ivec2(i);
    return mix(mix(hashI(c), hashI(c + ivec2(1, 0)), u.x),
               mix(hashI(c + ivec2(0, 1)), hashI(c + ivec2(1, 1)), u.x), u.y);
}

// ---- koi --------------------------------------------------------------------

struct Fish {
    vec2 pos;      // the nose
    float heading; // radians
    float speed;   // pond units per second
    float turn;    // radians per second, positive = turning left
    float phase;   // tail beat
    float depth;   // 0 = at the surface, 1 = on the bottom
    float effort;  // how hard the tail works
};

Fish loadFish(int i) {
    vec4 a = texelFetch(iChannel0, ivec2(i, 0), 0);
    vec4 b = texelFetch(iChannel0, ivec2(i, 1), 0);
    return Fish(a.xy, a.z, a.w, b.x, b.y, b.z, b.w);
}

struct Koi {
    float len;
    int variety;
    float seed;   // 0..1, varies patterns and personality
    float cruise; // relaxed swimming speed
};

Koi koiOf(int i) {
    float seed = fract(float(i) * 0.6180339 + 0.137);
    return Koi(LENGTH[i], VARIETY[i], seed, 0.03 + 0.018 * fract(seed * 7.31));
}

// ---- time of day ------------------------------------------------------------

struct Light {
    vec3 ambient; // light that reaches the bed and the koi
    vec3 sun;     // color of direct light: caustics, glints
    float direct; // strength of direct light
    vec3 sky;     // color that the surface reflects
    float night;  // 0 by day, 1 at night
};

Light lightKey(int k) {
    if (k == 2) { // dawn
        return Light(vec3(0.74, 0.6, 0.64), vec3(1.0, 0.68, 0.56), 0.5, vec3(1.0, 0.62, 0.62), 0.2);
    }
    if (k == 3) { // morning
        return Light(vec3(0.95, 0.98, 0.96), vec3(1.0, 0.95, 0.85), 0.85, vec3(0.62, 0.78, 0.92), 0.0);
    }
    if (k == 4) { // noon
        return Light(vec3(1.05, 1.07, 1.04), vec3(1.0, 1.0, 0.95), 1.0, vec3(0.66, 0.84, 0.96), 0.0);
    }
    if (k == 5) { // afternoon
        return Light(vec3(1.02, 0.99, 0.92), vec3(1.0, 0.9, 0.75), 0.9, vec3(0.7, 0.8, 0.9), 0.0);
    }
    if (k == 6) { // golden hour
        return Light(vec3(0.98, 0.76, 0.54), vec3(1.0, 0.62, 0.32), 0.75, vec3(1.0, 0.68, 0.42), 0.05);
    }
    if (k == 7) { // dusk
        return Light(vec3(0.5, 0.4, 0.58), vec3(0.95, 0.5, 0.55), 0.3, vec3(0.55, 0.32, 0.55), 0.5);
    }
    // night
    return Light(vec3(0.21, 0.27, 0.42), vec3(0.6, 0.75, 1.0), 0.25, vec3(0.05, 0.09, 0.2), 1.0);
}

const float KEY_HOURS[10] = float[10](0.0, 4.8, 6.2, 8.0, 12.5, 16.0, 18.0, 19.3, 20.8, 24.0);

Light lightAt(float hour) {
    int k = 0;
    for (int i = 0; i < 9; i++) {
        if (hour >= KEY_HOURS[i]) {
            k = i;
        }
    }
    float f = smoothstep(KEY_HOURS[k], KEY_HOURS[k + 1], hour);
    Light a = lightKey(k);
    Light b = lightKey(k + 1);
    return Light(mix(a.ambient, b.ambient, f), mix(a.sun, b.sun, f), mix(a.direct, b.direct, f),
                 mix(a.sky, b.sky, f), mix(a.night, b.night, f));
}

// The direction to the light. During the day the sun goes from left to
// right. The moon is at the upper right.
vec3 lightDirection(float hour, float night) {
    float day = clamp((hour - 6.0) / 12.0, 0.0, 1.0);
    vec2 across = normalize(vec2(-cos(day * PI), 0.55));
    float height = mix(0.35, 1.2, sin(day * PI));
    vec3 sun = vec3(across * cos(height), sin(height));
    return normalize(mix(sun, normalize(vec3(0.45, 0.6, 0.9)), night));
}

// ---- lily pads and fireflies -----------------------------------------------

const int NUM_PADS = 8;

// xy = position as a fraction of the pond, z = radius, w = rotation.
vec4 padDef(int i) {
    if (i == 0) return vec4(0.07, 0.82, 0.075, 0.4);
    if (i == 1) return vec4(0.17, 0.93, 0.05, 2.1);
    if (i == 2) return vec4(0.03, 0.6, 0.045, 4.0);
    if (i == 3) return vec4(0.9, 0.15, 0.08, 1.2);
    if (i == 4) return vec4(0.975, 0.35, 0.05, 5.1);
    if (i == 5) return vec4(0.79, 0.06, 0.045, 3.3);
    if (i == 6) return vec4(0.63, 0.95, 0.055, 0.9);
    return vec4(0.33, 0.04, 0.04, 2.7);
}

vec2 padCentre(int i, vec2 pond, float t) {
    float fi = float(i);
    return padDef(i).xy * pond + 0.012 * vec2(sin(t * 0.05 + fi * 1.7), cos(t * 0.04 + fi * 2.3));
}

const int NUM_FIREFLIES = 10;

vec2 fireflyPos(int k, vec2 pond, float t) {
    float fk = float(k);
    vec2 base = vec2(hash1(fk * 3.1 + 0.2), hash1(fk * 7.3 + 1.1)) * pond;
    return base + vec2(sin(t * 0.13 + fk * 1.9) * 0.08 + sin(t * 0.31 + fk) * 0.03,
                       cos(t * 0.11 + fk * 2.7) * 0.06);
}

// ---- lighting -----------------------------------------------------------------

float hourOfDay() {
    return mod(HOUR_OVERRIDE >= 0.0 ? HOUR_OVERRIDE : iDate.w / 3600.0, 24.0);
}

// Shadows fall away from the light, and are longer when the light is low.
vec2 shadowDirection(vec3 ld) {
    return -normalize(ld.xy + 1e-4) * (0.35 / max(ld.z, 0.35));
}

// How far refraction can move what is under the surface, at most, in pond
// units.
const float MAX_REFRACTION = 0.012;

// How far from a koi at `depth` its shadow falls on the bed.
float shadowReach(float depth) {
    return 0.015 + 0.06 * (1.0 - depth);
}

// ---- koi pose ---------------------------------------------------------------

struct Pose {
    vec2 pos;
    vec2 fwd;
    vec2 left;
    float len;
    float bend;  // how much the spine curves from turning
    float swing; // how far the tail swings, in body lengths
    float phase;
    float depth;
    float effort;
};

// Rows 0 to 2 of the column of a koi in the koi buffer (see fish.glsl).
Pose poseOf(vec4 a, vec4 b, vec4 c, Koi k) {
    Pose P;
    P.pos = a.xy;
    P.fwd = vec2(cos(a.z), sin(a.z));
    P.left = vec2(-P.fwd.y, P.fwd.x);
    P.len = k.len;
    P.bend = c.x;
    P.swing = 0.06 + 0.035 * min(b.w, 2.0);
    P.phase = b.y;
    P.depth = b.z;
    P.effort = b.w;
    return P;
}

// The spine at distance u behind the nose: its side offset and slope. It is
// the bend from turning, plus a wave that grows toward the tail.
struct SpinePoint {
    float y;
    float slope;
};

SpinePoint spineAt(Pose P, float u) {
    float t = max(u / P.len, 1e-4);
    float a = P.phase - t * 5.0;
    float sa = sin(a);
    float t07 = pow(t, 0.7);
    float t17 = t07 * t;
    return SpinePoint(0.5 * P.bend * u * u + P.swing * P.len * t17 * sa,
                      P.bend * u + P.swing * (1.7 * t07 * sa - 5.0 * t17 * cos(a)));
}

// Half the body width at t (0 = nose, 1 = tail root), as a fraction of the
// widest point. The head is blunt and round, and the body tapers.
float halfWidth(float t) {
    float x = clamp(t / 0.2, 0.0, 1.0);
    float nose = sqrt(x * (2.0 - x));
    float taper = 1.0 - 0.8 * pow(clamp((t - 0.24) / 0.76, 0.0, 1.0), 1.4);
    return nose * taper;
}

const float WIDTH = 0.12;          // widest half-width, in body lengths
const float WIDTH_PELVIC = 0.741;  // halfWidth(0.58), where the pelvic fins sit
const float WIDTH_EYES = 0.818;    // halfWidth(0.085), where the eyes sit

// Point q in the frame of the fish: x = distance behind the nose, y = to the
// side.
vec2 localOf(Pose P, vec2 q) {
    vec2 d = q - P.pos;
    return vec2(-dot(d, P.fwd), dot(d, P.left));
}

// True if `local` can be on the koi (body, fins, or tail). `sp` is the spine
// at local.x.
bool nearKoi(Pose P, vec2 local, SpinePoint sp) {
    return local.x > -0.02 * P.len && local.x < 1.32 * P.len
        && abs(local.y - sp.y) < 0.42 * P.len;
}

Pose loadPose(int i) {
    return poseOf(texelFetch(iChannel0, ivec2(i, 0), 0), texelFetch(iChannel0, ivec2(i, 1), 0),
                  texelFetch(iChannel0, ivec2(i, 2), 0), koiOf(i));
}

// The largest bend from turning. The tail root is then at most 0.3 lengths
// to the side.
float maxBend(float len) {
    return 0.6 / len;
}

// ---- water ------------------------------------------------------------------

// One step of the wave equation on the height field in iChannel1. Each texel
// holds (height now, height one step ago).
vec2 waterStep(vec2 fragCoord) {
    ivec2 c = ivec2(fragCoord);
    ivec2 last = ivec2(iChannelResolution[1].xy) - 1;
    vec2 h = texelFetch(iChannel1, c, 0).xy;
    float n = texelFetch(iChannel1, ivec2(c.x, min(c.y + 1, last.y)), 0).x;
    float s = texelFetch(iChannel1, ivec2(c.x, max(c.y - 1, 0)), 0).x;
    float e = texelFetch(iChannel1, ivec2(min(c.x + 1, last.x), c.y), 0).x;
    float w = texelFetch(iChannel1, ivec2(max(c.x - 1, 0), c.y), 0).x;
    // At 30 fps there are two steps for each frame. At higher frame rates the
    // steps are smaller, so ripples spread at the same speed.
    float k = clamp(iTimeDelta * 30.0, 0.0, 1.0);
    float next = 2.0 * h.x - h.y + 0.5 * k * k * (n + s + e + w - 4.0 * h.x);
    // Ripples fade as they move. Near the banks they fade faster and do not
    // bounce back.
    float toBank = float(min(min(c.x, last.x - c.x), min(c.y, last.y - c.y)));
    next *= pow(mix(0.9, 0.985, smoothstep(0.0, 14.0, toBank)), k);
    return vec2(clamp(next, -4.0, 4.0), h.x);
}
