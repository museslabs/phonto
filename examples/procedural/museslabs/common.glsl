// The settings and the shared code of the Muses Labs lyre.
//
// The scene uses the units of the logo file. The logo is 400 units square,
// x goes right, and y goes down. The three strings are drawn in code, so
// that they can vibrate. lyre.png has the rest of the logo.

// The colors: the paper colors of museslabs.org, or the dark colors of the
// site.
// 0 = follow the clock, as LIGHT_FROM and DARK_FROM set.
// 1 = always paper. 2 = always dark.
// 3 = follow the light or dark setting of the desktop, as the site does.
// 4 = follow the sun: paper from sunrise to sunset. Set your location in
//     the phonto configuration.
#define THEME 0

// The local hours when the paper colors and the dark colors start.
const float LIGHT_FROM = 6.0;
const float DARK_FROM = 12.0;

// The display that shows the lyre when the wallpaper spans several displays,
// from 0. -1 = the display nearest the middle of the desktop.
const int LYRE_DISPLAY = -1;

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

// The sun at this moment, from iDate and iLocation. x is its elevation
// above the horizon, and y is its azimuth from north to the east, in degrees.
// z is the sunrise and w is the sunset, in seconds since local midnight. In a
// polar night, z and w are -1. In a polar day, z is 0 and w is 86400. The
// formulas are the NOAA approximations, accurate to about a minute. Copy
// this function into your own packs.
vec4 sun() {
    const int DAYS_BEFORE[12] = int[12](0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334);
    int year = int(iDate.x);
    int month = int(iDate.y);
    bool leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    float day = float(DAYS_BEFORE[month] + int(iDate.z) - 1 + (leap && month > 1 ? 1 : 0));
    float lat = radians(iLocation.x);
    float lon = iLocation.y;
    float zone = iLocation.z * 60.0;   // minutes ahead of UTC

    // The fraction of the year, in radians.
    float g = 2.0 * PI / 365.0 * (day + (iDate.w / 3600.0 - 12.0) / 24.0);
    // How far the solar time is ahead of the clock, in minutes.
    float equation = 229.18 * (0.000075 + 0.001868 * cos(g) - 0.032077 * sin(g)
        - 0.014615 * cos(2.0 * g) - 0.040849 * sin(2.0 * g));
    // The latitude where the sun is overhead, in radians.
    float declination = 0.006918 - 0.399912 * cos(g) + 0.070257 * sin(g)
        - 0.006758 * cos(2.0 * g) + 0.000907 * sin(2.0 * g)
        - 0.002697 * cos(3.0 * g) + 0.00148 * sin(3.0 * g);

    float solar = iDate.w / 60.0 + equation + 4.0 * lon - zone;
    float hourAngle = radians(solar / 4.0 - 180.0);
    float cosZenith = clamp(sin(lat) * sin(declination)
        + cos(lat) * cos(declination) * cos(hourAngle), -1.0, 1.0);
    float elevation = 90.0 - degrees(acos(cosZenith));
    float azimuth = degrees(atan(sin(hourAngle),
        cos(hourAngle) * sin(lat) - tan(declination) * cos(lat))) + 180.0;

    // The sun is up when its center is 0.833 degrees below the horizon,
    // because the air bends its light and the disk has a size.
    float cosRise = cos(radians(90.833)) / (cos(lat) * cos(declination))
        - tan(lat) * tan(declination);
    vec2 times = vec2(-1.0);
    if (cosRise <= -1.0) {
        times = vec2(0.0, 86400.0);
    } else if (cosRise < 1.0) {
        float halfDay = degrees(acos(cosRise)) * 4.0;
        float noon = 720.0 - 4.0 * lon - equation + zone;
        times = mod(vec2(noon - halfDay, noon + halfDay) * 60.0, 86400.0);
    }
    return vec4(elevation, azimuth, times);
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
