// The settings and the shared code of the Muses Labs lyre.
//
// The scene uses the units of the logo file. The logo is 400 units square,
// x goes right, and y goes down. The three strings are drawn in code, so
// that they can vibrate. lyre.png has the rest of the logo.

// 0 = follow the clock, as LIGHT_FROM and DARK_FROM set. 1 = always the
// paper colors of museslabs.org. 2 = always the dark colors of the site.
#define THEME 0

// The local hours when the paper colors and the dark colors start.
// LIGHT_FROM must come before DARK_FROM.
const float LIGHT_FROM = 6.0;
const float DARK_FROM = 12.0;

// 1 = play a built-in chord pattern instead of the system audio. Use it to
// preview the wallpaper with `phonto dump`, which has no audio.
#define DEMO 0

const float PI = 3.14159265;

const int STRINGS = 3;
// The note slots. The strings buffer is this many texels wide.
const int NOTES = 16;
// The texel in row 0 of the strings buffer that turns plucks into notes.
const int WRITER = STRINGS;

const float STRING_X[3] = float[3](178.0, 200.0, 222.0);
const float STRING_HALF_WIDTH = 4.5;
// The lower edge of the crossbar.
const float STRING_TOP = 151.25;
// The strings vibrate between the crossbar and the bowl.
const float STRING_BOTTOM[3] = float[3](279.3, 286.5, 279.1);
// A drawn string goes into the bowl to this line. It covers the cut in
// lyre.png.
const float STRING_COVER[3] = float[3](283.5, 288.5, 283.5);

// The bands of iSpectrum for each string: bass, middle, and treble.
const int BAND_FIRST[3] = int[3](0, 10, 20);
const int BAND_LAST[3] = int[3](9, 19, 31);

// The staff has five lines, SPACE units apart. Position 0 is the bottom
// line and position 8 is the top line.
const float SPACE = 11.0;
const float STAFF_BOTTOM = 239.0;

float staffY(float position) {
    return STAFF_BOTTOM - position * SPACE * 0.5;
}

// The place of each note name on the treble staff, from C to B. Position 0
// is E4 on the bottom line and position 6 is D5, so a chord always shows
// inside one octave. A sharp uses the place of the note under it.
const int NOTE_POSITION[12] = int[12](5, 5, 6, 6, 0, 1, 1, 2, 2, 3, 3, 4);
const bool NOTE_SHARP[12] = bool[12](false, true, false, true, false, false,
    true, false, true, false, true, false);
// A chord has at most three note names. NO_NOTE fills the empty places.
const int NO_NOTE = 12;

// A note slot keeps its chord in one float: the three note names in base
// 13, plus 2197 for an eighth note.
float packChord(int names[3], bool eighth) {
    return float(names[0] + 13 * names[1] + 169 * names[2]) + (eighth ? 2197.0 : 0.0);
}

void unpackChord(float code, out int names[3], out bool eighth) {
    int c = int(code + 0.5);
    eighth = c >= 2197;
    c %= 2197;
    names = int[3](c % 13, (c / 13) % 13, c / 169);
}

float hash(vec2 p) {
    vec3 q = fract(vec3(p.xyx) * 0.1031);
    q += dot(q, q.yzx + 33.33);
    return fract((q.x + q.y) * q.z);
}

#if DEMO
const float DEMO_BEAT = 0.4;
// C major, A minor, F major, and G major, for two bars of four beats each.
const int DEMO_CHORD[12] = int[12](0, 4, 7, 9, 0, 4, 5, 9, 0, 7, 11, 2);

// One pluck on each beat. The strings play a pattern over the chords.
float band(int i) {
    const int PATTERN[8] = int[8](0, 2, 1, 2, 0, 1, 2, 1);
    const int ROOT[4] = int[4](0, 3, 5, 2);
    float level = 0.0;
    float now = floor(iTime / DEMO_BEAT);
    for (int k = 0; k < 4; k++) {
        float n = now - float(k);
        if (n < 0.0) {
            break;
        }
        int beat = int(mod(n, 8.0));
        int s = PATTERN[beat];
        int root = ROOT[int(mod(floor(n / 8.0), 4.0))];
        int center = BAND_FIRST[s] + 2 + root + (s == 1 ? beat % 3 : 0);
        float age = iTime - n * DEMO_BEAT;
        float d = float(i - center);
        level = max(level, 0.9 * exp(-age * 3.0 - d * d / 2.0));
    }
    return level;
}

// The note names of the chord that plays now. The root is the strongest.
float chroma(int i) {
    int chord = int(mod(floor(iTime / (8.0 * DEMO_BEAT)), 4.0));
    float level = 0.1;
    for (int k = 0; k < 3; k++) {
        if (DEMO_CHORD[3 * chord + k] == i) {
            level = 1.0 - 0.1 * float(k);
        }
    }
    return level;
}
#else
float band(int i) {
    return iSpectrum[i];
}

float chroma(int i) {
    return iChroma[i];
}
#endif

// The level of the loudest band of string s, from 0 to 1.
float stringLevel(int s) {
    float level = 0.0;
    for (int i = BAND_FIRST[s]; i <= BAND_LAST[s]; i++) {
        level = max(level, band(i));
    }
    return level;
}
