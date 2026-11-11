// The state of the strings and the notes. Each frame reads the last one.
//
// Row 0, texels 0 to 2, one for each string:
//   x = the slow level, y = the pluck strength, z = the level in the last
//   frame, w = the time of the last pluck.
// Row 0, texel WRITER, which turns plucks into notes:
//   x = the time to write the next note, or 0 if no note waits,
//   y = the time of the newest pluck that it saw, z = the note count,
//   w = the time of the pluck of the last note.
// Row 1, one texel for each note slot:
//   x = the start time, y = the chord (see packChord), z = the strength,
//   w = the note count that started the note.

// A pluck is a level that rises in this frame and is well above the slow
// level. A level that falls after a hit is not a new pluck.
const float PLUCK_RISE = 0.08;
const float PLUCK_MIN_LEVEL = 0.2;
const float PLUCK_GAP = 0.2;     // seconds between two plucks of a string
const float PLUCK_FADE = 0.6;    // seconds
const float SLOW_FADE = 0.3;     // seconds
// Plucks closer together than this write one note. So a chord on several
// strings writes one chord.
const float NOTE_GAP = 0.15;
// iChroma uses a long window, so it is late. A note waits this long after
// its pluck, so that it shows the new chord.
const float CHROMA_DELAY = 0.12;
// A note name joins the chord when its chroma is at least this part of the
// strongest note name.
const float CHORD_SHARE = 0.7;
// A note that comes sooner than this after the last note is an eighth note.
// A slower note is a quarter note.
const float EIGHTH_GAP = 0.3;
// The note count starts again at 0 after this value. It is a multiple of
// NOTES, so each count keeps the same slot.
const float COUNT_WRAP = 960.0;

vec4 updateString(int s, vec4 string) {
    float level = stringLevel(s);
    float since = iTime - string.w;
    // iTime goes back to 0 after a day. Forget a pluck from the future.
    if (since < 0.0) {
        since = 1e4;
    }
    float pluck = string.y * exp(-iTimeDelta / PLUCK_FADE);
    float rise = level - string.x;
    bool rising = level > string.z + 0.01;
    if (rising && rise > PLUCK_RISE && level > PLUCK_MIN_LEVEL && since > PLUCK_GAP) {
        pluck = max(pluck, clamp(0.3 + rise * 2.5, 0.0, 1.0));
        string.w = iTime;
    }
    string.x += (level - string.x) * (1.0 - exp(-iTimeDelta / SLOW_FADE));
    string.y = pluck;
    string.z = level;
    return string;
}

vec4 updateWriter(vec4 writer) {
    // iTime goes back to 0 after a day. Forget the times from the future.
    if (writer.y > iTime || writer.w > iTime) {
        writer = vec4(0.0, 0.0, writer.z, 0.0);
    }
    float newest = 0.0;
    for (int s = 0; s < STRINGS; s++) {
        float pluck = texelFetch(iChannel0, ivec2(s, 0), 0).w;
        if (pluck <= iTime) {
            newest = max(newest, pluck);
        }
    }
    if (newest > writer.y) {
        if (writer.x == 0.0 && newest - writer.w > NOTE_GAP) {
            writer.x = iTime + CHROMA_DELAY;
            writer.w = newest;
        }
        writer.y = newest;
    }
    if (writer.x > 0.0 && iTime >= writer.x) {
        writer.x = 0.0;
        writer.z = mod(writer.z + 1.0, COUNT_WRAP);
    }
    return writer;
}

// The chord that plays now: the strongest note names, at most three. Two
// note names on one staff position, such as C and C sharp, do not join
// together.
float chordNow(bool eighth) {
    int names[3] = int[3](NO_NOTE, NO_NOTE, NO_NOTE);
    float strongest = 0.0;
    for (int i = 0; i < 12; i++) {
        strongest = max(strongest, chroma(i));
    }
    if (strongest > 0.05) {
        for (int n = 0; n < 3; n++) {
            int best = NO_NOTE;
            float level = CHORD_SHARE * strongest;
            for (int i = 0; i < 12; i++) {
                bool taken = false;
                for (int m = 0; m < n; m++) {
                    taken = taken || NOTE_POSITION[names[m]] == NOTE_POSITION[i];
                }
                if (!taken && chroma(i) >= level) {
                    best = i;
                    level = chroma(i);
                }
            }
            if (best == NO_NOTE) {
                break;
            }
            names[n] = best;
        }
    }
    return packChord(names, eighth);
}

void mainImage(out vec4 state, in vec2 coord) {
    ivec2 cell = ivec2(coord);
    state = texelFetch(iChannel0, cell, 0);
    if (cell.y == 0) {
        if (cell.x < STRINGS) {
            state = updateString(cell.x, state);
        } else if (cell.x == WRITER) {
            state = updateWriter(state);
        }
        return;
    }
    vec4 writer = texelFetch(iChannel0, ivec2(WRITER, 0), 0);
    // Each note count has one slot, so one pluck writes one note.
    if (int(writer.z) % NOTES == cell.x && writer.z != state.w) {
        vec4 last = texelFetch(iChannel0, ivec2((cell.x + NOTES - 1) % NOTES, 1), 0);
        float gap = iTime - last.x;
        float strength = 0.0;
        for (int s = 0; s < STRINGS; s++) {
            strength = max(strength, texelFetch(iChannel0, ivec2(s, 0), 0).y);
        }
        state = vec4(iTime, chordNow(gap > 0.0 && gap < EIGHTH_GAP), strength, writer.z);
    }
}
