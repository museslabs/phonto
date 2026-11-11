// The cover of the music that plays, on a background of its own colors.
//
// iChannel0 = the cover, which now-playing.sh keeps current. Before the
// first cover, it is a single clear pixel.
// iInput.track: x = 1 while music plays, y = how far the track is.

const vec3 LIGHT_PAPER = vec3(0.966, 0.924, 0.842);
const vec3 LIGHT_INK = vec3(0.140, 0.105, 0.082);
const vec3 DARK_PAPER = vec3(0.104, 0.081, 0.066);
const vec3 DARK_INK = vec3(0.936, 0.920, 0.888);

// The side of the cover, and its corner radius, in display heights.
const float SIDE = 0.46;
const float RADIUS = 0.012;

float roundedBox(vec2 p, vec2 half_, float r) {
    vec2 d = abs(p) - half_ + r;
    return length(max(d, 0.0)) + min(max(d.x, d.y), 0.0) - r;
}

// The cover at uv in 0..1, cropped to a square if it is not one.
vec3 cover(vec2 uv, float lod) {
    vec2 size = iChannelResolution[0].xy;
    vec2 square = vec2(min(size.x, size.y)) / size;
    return textureLod(iChannel0, 0.5 + (uv - 0.5) * square, lod).rgb;
}

// The display nearest the middle of the image. A spanned wallpaper shows the
// cover there.
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
    vec4 display = stage();
    float height = min(display.w, display.z * 1.4);
    vec2 p = (coord - display.xy - 0.5 * display.zw) / height - vec2(0.0, 0.03);
    float px = 1.0 / height;
    float t = iTime;
    bool hasCover = iChannelResolution[0].x > 1.0;
    float playing = iInput.track.x;
    float progress = clamp(iInput.track.y, 0.0, 1.0);

    vec3 paper = mix(LIGHT_PAPER, DARK_PAPER, iColorScheme);
    vec3 ink = mix(LIGHT_INK, DARK_INK, iColorScheme);
    vec3 col = paper;
    vec3 line = mix(paper, ink, 0.3);
    vec3 mark = ink;

    if (hasCover) {
        // The smallest mipmaps of the cover blur it to its main colors. Two
        // of them drift slowly over the screen.
        float lod = log2(max(iChannelResolution[0].x, iChannelResolution[0].y)) - 2.0;
        vec2 uv = coord / iResolution.xy;
        vec2 drift = 0.08 * vec2(sin(t * 0.05), cos(t * 0.04));
        vec3 wash = mix(cover(uv + drift, lod), cover(1.0 - uv - drift, lod + 0.7), 0.5);
        col = wash * (0.55 - 0.15 * length(uv - 0.5));
        line = vec3(1.0, 1.0, 1.0) * 0.3;
        mark = vec3(1.0, 1.0, 1.0) * 0.9;

        // The cover breathes with the bass while music plays.
        float side = SIDE * (1.0 + 0.012 * iAudio.x * playing);
        float box = roundedBox(p, vec2(0.5 * side), RADIUS);
        col *= 1.0 - 0.45 * exp(-max(box + 0.01, 0.0) * 30.0);
        vec3 art = cover(p / side + 0.5, 0.0);
        // A paused track looks quieter.
        float grey = dot(art, vec3(0.299, 0.587, 0.114));
        art = mix(vec3(grey), art, mix(0.5, 1.0, playing)) * mix(0.8, 1.0, playing);
        col = mix(col, art, 1.0 - smoothstep(-px, px, box));
    } else {
        // No cover yet: an empty frame.
        float frame = abs(roundedBox(p, vec2(0.5 * SIDE), RADIUS)) - 0.5 * max(px, 0.0012);
        col = mix(col, line, 1.0 - smoothstep(-px, px, frame));
    }

    // How far the track is: a hairline under the cover, and a dot at the
    // place that plays.
    vec2 q = p - vec2(0.0, -0.5 * SIDE - 0.045);
    float across = clamp(q.x / SIDE + 0.5, 0.0, 1.0);
    float rule = (1.0 - smoothstep(0.5 * max(px, 0.0015), 0.5 * max(px, 0.0015) + px, abs(q.y)))
        * step(abs(q.x), 0.5 * SIDE);
    col = mix(col, across <= progress ? mark : line, rule);
    float dot_ = 1.0 - smoothstep(-px, px, length(q - vec2((progress - 0.5) * SIDE, 0.0)) - 0.005);
    col = mix(col, mark, dot_ * step(0.001, progress));

    col += (fract(sin(dot(floor(coord), vec2(12.9898, 78.233))) * 43758.5453) - 0.5) * 0.01;
    color = vec4(col, 1.0);
}
