// Zero TV: the tape menu of the video, as a launcher. Each tape is action
// `tapeN`. The [[action]] entries of the configuration give the command and
// the label of each tape. All places are in scene pixels: the pixels of the
// 1920x1080 video, from its top left. An art pixel is 3 scene pixels.
//
// The video gives the room, the TV, and the shelf. The tapes are drawn as
// graphics, in the colors of the video. Each tape is the same box: the
// sleeve of a VHS tape, 19 x 2.5 x 10.5 cm. It lies flat, with its spine to
// the viewer. The spine is the front face, with a number tag and a label. As
// in the video, depth goes up and to the right: 10.5 cm of depth is 30
// pixels to the right and 12 up. A box shows its front face, its top face,
// and the side that faces the viewer.
//
// The stacks are untidy, as in the video. Each box sits a little to the left
// or right and to the front or back, at a small angle. A box turned with its
// right end back shows its left side. The stacks are drawn from the bottom up,
// so a tape is in front of the tapes under it.
//
// A selected tape slides straight out of its stack toward the viewer, by the
// depth of a box. That is 30 pixels to the left and 12 down. Every tape,
// turned or not, moves the same way. The back of the shelf shows where it was.

// To preview without the pointer, for example with `phonto dump`, set
// DEMO_TAPE to a tape from 1 to 12. DEMO_CLICK is the time of a click on that
// tape in seconds, or a negative number for no click.
#define DEMO_TAPE 0
#define DEMO_CLICK -1.0
// How far the demo tape is out, from 0 to 1.
#define DEMO_OUT 1.0

const vec2 VIDEO = vec2(1920.0, 1080.0);

// The scene pixel at `frag`, a pixel of an image of `res` pixels from its
// bottom left. The video covers the image and keeps its aspect ratio, as
// with `--scale fill`.
vec2 toScene(vec2 frag, vec2 res) {
    float scale = max(res.x / VIDEO.x, res.y / VIDEO.y);
    vec2 p = (frag - (res - VIDEO * scale) * 0.5) / scale;
    return vec2(p.x, VIDEO.y - p.y);
}

// back.png and the tapes buffer cover this part of the scene.
const ivec2 SHELF_ORIGIN = ivec2(120, 760);
const ivec2 SHELF_SIZE = ivec2(1640, 280);

bool inShelf(vec2 s) {
    ivec2 t = ivec2(floor(s)) - SHELF_ORIGIN;
    return all(greaterThanEqual(t, ivec2(0))) && all(lessThan(t, SHELF_SIZE));
}

// The texel of a shelf image at scene pixel `s`, with the bottom row first.
ivec2 shelfTexel(vec2 s) {
    ivec2 t = ivec2(floor(s)) - SHELF_ORIGIN;
    return ivec2(t.x, SHELF_SIZE.y - 1 - t.y);
}

// The place of each box at rest:
//   - x and y of the top left corner of its front face, in scene pixels
//   - the height of its front face
//   - how far the box is turned, in degrees
// Tapes 1-4, 5-8, and 9-12 are the stacks, each from the top down. Each box
// comment gives its place on the shelf: x at the front, and z in cm back
// from the front of the stack.
const vec4 PLACES[12] = vec4[12](
    vec4(190.0, 782.0, 61.0, 0.0),  // x 190, z 0.0 cm
    vec4(169.7, 842.3, 62.0, -3.0),  // x 168, z 0.6 cm
    vec4(150.3, 905.7, 62.0, -8.0),  // x 152, z -0.6 cm
    vec4(172.6, 966.8, 61.0, -5.0),  // x 172, z 0.2 cm
    vec4(732.0, 782.0, 61.0, 2.0),  // x 732, z 0.0 cm
    vec4(714.3, 842.1, 62.0, -4.0),  // x 712, z 0.8 cm
    vec4(742.9, 905.5, 62.0, 3.0),  // x 744, z -0.4 cm
    vec4(734.9, 966.7, 61.0, -2.0),  // x 734, z 0.3 cm
    vec4(1288.0, 782.0, 61.0, 9.0),  // x 1288, z 0.0 cm
    vec4(1280.4, 842.4, 62.0, 12.0),  // x 1279, z 0.5 cm
    vec4(1274.6, 905.6, 62.0, 15.0),  // x 1276, z -0.5 cm
    vec4(1284.6, 966.8, 61.0, 10.0)   // x 1284, z 0.2 cm
);

// The width of a front face, in scene pixels: 19 cm.
const float TAPE_W = 450.0;
const float PX_PER_CM = TAPE_W / 19.0;
// One cm of depth, on the screen.
const vec2 RECEDE = vec2(30.0, -12.0) / 10.5;
const float DEPTH_CM = 10.5;

// The part of the shelf where each stack can draw, from left to right: x from
// and to. A tape that slides out can leave the space of its stack. So the
// parts overlap, and a pixel near the edge of two stacks looks at both.
const vec2 STACK_SPANS[3] = vec2[3](vec2(40.0, 705.0), vec2(640.0, 1245.0), vec2(1200.0, 1800.0));

// True if stack `stack` (0 for tapes 1-4, 1 for 5-8, 2 for 9-12) can draw
// at `s`.
bool stackNear(int stack, vec2 s) {
    return s.x >= STACK_SPANS[stack].x && s.x < STACK_SPANS[stack].y;
}

// True near the shelf, where a tape can be.
bool nearShelf(vec2 s) {
    return s.x >= 40.0 && s.x < 1800.0 && s.y >= 755.0 && s.y < 1060.0;
}

// A box on the screen. Down its height is straight down.
struct Box {
    vec2 origin;   // the top left corner of its front face
    vec2 along;    // the screen step for one pixel along its width
    vec2 depth;    // the screen step for its whole depth
    float height;  // the height of its front face
};

// Box `tape`, moved by `offset` from its place at rest.
Box boxOf(int tape, vec2 offset) {
    vec4 place = PLACES[tape - 1];
    float turn = radians(place.w);
    Box b;
    b.origin = place.xy + offset;
    b.along = vec2(cos(turn), 0.0) + RECEDE * sin(turn) / PX_PER_CM;
    b.depth = (vec2(-PX_PER_CM * sin(turn), 0.0) + RECEDE * cos(turn)) * DEPTH_CM;
    b.height = place.z;
    return b;
}

// The part of box `b` that covers scene pixel `p`. It is 1 for the front
// face, 2 for the top face, 3 for the right side, 4 for the left side, or 0.
// `face` is the place on that part:
//   - front face: x along the width and y down the height, in pixels
//   - top face: x along the width, and the depth from 0 to 1
//   - side: the depth from 0 to 1, and y down the height
// `edge` is the distance in scene pixels to the nearest edge of the face,
// for soft edges. A pixel within half a pixel outside a face counts.
int boxCover(Box b, vec2 p, out vec2 face, out float edge) {
    vec2 d = p - b.origin;
    // The front face: x along the width, and straight down.
    float u = d.x / b.along.x;
    float v = d.y - u * b.along.y;
    edge = min(min(u, TAPE_W - u) * b.along.x, min(v, b.height - v));
    if (edge > -0.5) {
        face = vec2(u, v);
        return 1;
    }
    // The top face: along the width and back through the depth.
    float det = b.along.x * b.depth.y - b.along.y * b.depth.x;
    float tu = (d.x * b.depth.y - d.y * b.depth.x) / det;
    float tw = (b.along.x * d.y - b.along.y * d.x) / det;
    edge = min(min(tu, TAPE_W - tu) * abs(det) / length(b.depth),
               min(tw, 1.0 - tw) * abs(det) / length(b.along));
    if (edge > -0.5) {
        face = vec2(tu, tw);
        return 2;
    }
    // The side that faces the viewer: the right side if the depth goes to
    // the right, else the left side.
    bool right = b.depth.x > 0.0;
    vec2 s = right ? d - b.along * TAPE_W : d;
    float w = s.x / b.depth.x;
    float h = s.y - w * b.depth.y;
    edge = min(min(w, 1.0 - w) * abs(b.depth.x), min(h, b.height - h));
    if (edge > -0.5) {
        face = vec2(w, h);
        return right ? 3 : 4;
    }
    return 0;
}

// What of `tape`, moved by `offset`, covers scene pixel `p`.
int tapeCover(int tape, vec2 p, vec2 offset) {
    vec2 face;
    float edge;
    return boxCover(boxOf(tape, offset), p, face, edge);
}

// The state buffer is 13x1 texels. Texel 0: the selected tape, the time it
// became selected, the last clicked tape, and the time of that click. Texel
// N: how far tape N is out, from 0 to 1.
const vec4 START_STATE = vec4(1.0, -100.0, 0.0, -100.0);

// The time line of a click, in seconds. The tape comes out, flies to the
// VCR, and goes into its slot, and the tapes above it fall into its place.
// The TV plays. Then the tape rewinds: the whole time line runs backward,
// fast, with the noise of a VHS tape that rewinds.
const float CLICK_OUT = 0.12;
const float CLICK_ARRIVE = 0.5;
const float CLICK_IN = 0.72;
const float FALL_START = 0.16;
const float FALL_TIME = 0.2;
const float REWIND_START = 2.9;
const float REWIND_END = 3.45;

// Seconds since `then`. A time from before iTime went back to 0 counts as
// long ago.
float since(float then) {
    float d = iTime - then;
    return d < 0.0 ? 1e4 : d;
}

// The tape of the click that plays now, or 0.
int clickedTape(vec4 state) {
    return (state.z > 0.5 && since(state.w) < REWIND_END) ? int(state.z + 0.5) : 0;
}

// The time on the time line of the click: forward, and then backward
// during the rewind.
float clickTime(vec4 state) {
    float t = since(state.w);
    if (t < REWIND_START) {
        return t;
    }
    return REWIND_START * (1.0 - clamp((t - REWIND_START) / (REWIND_END - REWIND_START), 0.0, 1.0));
}

// How far a tape `amount` out (0 to 1) is from its place at rest, in whole art
// pixels. It moves straight toward the viewer, by up to the depth of a box. A
// tape does not move along its own depth. The screen shows depth much shorter
// than width, so the depth of a turned box goes far to the side. The most
// turned tape then comes out twice as far.
vec2 pullOffset(float amount) {
    return floor(-RECEDE * DEPTH_CM * amount / 3.0 + 0.5) * 3.0;
}

// How far `tape` has fallen because a tape under it left the stack, in whole
// art pixels.
float fallOf(vec4 state, int tape) {
    int gone = clickedTape(state);
    if (gone == 0 || tape >= gone || (tape - 1) / 4 != (gone - 1) / 4) {
        return 0.0;
    }
    float u = clamp((clickTime(state) - FALL_START) / FALL_TIME, 0.0, 1.0);
    return floor(u * u * PLACES[gone - 1].z / 3.0 + 0.5) * 3.0;
}

// The tape that the pointer is on at `s`: the front-most tape there. The
// image pass draws the stacks from left to right, and each stack from its
// bottom tape up, so this looks in the other order.
int hitTape(sampler2D state, vec2 s) {
    if (!nearShelf(s)) {
        return 0;
    }
    vec4 st = texelFetch(state, ivec2(0), 0);
    int gone = clickedTape(st);
    for (int stack = 2; stack >= 0; stack--) {
        if (!stackNear(stack, s)) {
            continue;
        }
        for (int k = stack * 4 + 1; k <= stack * 4 + 4; k++) {
            if (k == gone) {
                continue;
            }
            vec2 offset = pullOffset(texelFetch(state, ivec2(k, 0), 0).x) + vec2(0.0, fallOf(st, k));
            if (tapeCover(k, s, offset) > 0) {
                return k;
            }
        }
    }
    return 0;
}

// True while a click plays. The menu takes no hover and no click then, so
// a double click runs the action once.
bool isBusy(vec4 state) {
    return clickedTape(state) > 0;
}
