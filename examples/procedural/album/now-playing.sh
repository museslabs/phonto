#!/bin/sh
# The input `track` of examples/procedural/album (see inputs.toml).
#
# phonto runs this script every second. It prints two numbers: 1 while music
# plays, else 0, and how far the track is, from 0 to 1. It also keeps the
# cover of the track in ~/.cache/phonto/cover, which the wallpaper watches.
#
# On Linux, it needs playerctl, which reads every player that supports MPRIS,
# such as Spotify, Firefox, Chromium, mpv, and VLC. A cover from the web also
# needs curl.
#
# On macOS, it needs media-control (brew install media-control). It reads
# what macOS shows as Now Playing, from any app, such as Spotify, Music, or a
# video in a browser.

cover="$HOME/.cache/phonto/cover"

# Numbers use a decimal point in every language. phonto reads a comma as the
# end of a number.
export LC_ALL=C

# Decodes the escapes of a file URL, such as %20.
decode() {
    printf '%s' "$1" | awk '{
        hex = "0123456789abcdef"
        out = ""
        for (i = 1; i <= length($0); i++) {
            c = substr($0, i, 1)
            if (c == "%") {
                high = index(hex, tolower(substr($0, i + 1, 1))) - 1
                low = index(hex, tolower(substr($0, i + 2, 1))) - 1
                out = out sprintf("%c", high * 16 + low)
                i += 2
            } else {
                out = out c
            }
        }
        printf "%s", out
    }'
}

# Gets the cover at the URL $1 into a file of its own. If no newer cover is
# wanted, it moves the file into place, so the wallpaper never reads half of
# an image. Then it records the URL of the cover in place.
fetch() {
    part="$cover.part.$$"
    case "$1" in
    file://*) cp "$(decode "${1#file://}")" "$part" ;;
    *) curl -sfL --max-time 30 "$1" -o "$part" ;;
    esac &&
        [ "$(cat "$cover.want" 2>/dev/null)" = "$1" ] &&
        mv "$part" "$cover" &&
        printf '%s\n' "$1" > "$cover.url"
    rm -f "$part"
}

if [ "$(uname)" = Darwin ]; then
    # An app that starts phonto may not have Homebrew in its PATH.
    PATH="$PATH:/opt/homebrew/bin:/usr/local/bin"
    info=$(media-control get --now --no-artwork 2>/dev/null)
    field() { printf '%s' "$info" | plutil -extract "$1" raw -o - - 2>/dev/null; }
    # Without media-control, or with nothing to show, there is no track.
    if ! id=$(field contentItemIdentifier); then
        echo 0 0
        exit 0
    fi
    # The cover is a few hundred kilobytes of base64. Only a new track reads
    # it. cover.url names the track of the cover in place.
    if [ "$id" != "$(cat "$cover.url" 2>/dev/null)" ]; then
        mkdir -p "${cover%/*}"
        part="$cover.part.$$"
        if media-control get 2>/dev/null |
            plutil -extract artworkData raw -o - - 2>/dev/null |
            base64 -D > "$part" 2>/dev/null &&
            [ -s "$part" ]; then
            mv "$part" "$cover" && printf '%s\n' "$id" > "$cover.url"
        elif [ "$(cat "$cover.url" 2>/dev/null)" != none ]; then
            # A track without a cover, such as some videos in a browser,
            # shows none, not the cover of the last track. phonto keeps the
            # last image of a deleted file, and the pack draws a cover of
            # one pixel as no cover. Each run still looks for a cover, in
            # case it comes later.
            [ -f "$cover" ] &&
                sips -z 1 1 "$cover" --out "$part" > /dev/null 2>&1 &&
                mv "$part" "$cover"
            echo none > "$cover.url"
        fi
        rm -f "$part"
    fi
    [ "$(field playing)" = true ] && playing=1 || playing=0
    awk -v p="$(field elapsedTimeNow)" -v l="$(field duration)" -v on="$playing" \
        'BEGIN { printf "%d %.4f\n", on, (l > 0 ? p / l : 0) }'
    exit 0
fi

if ! status=$(playerctl status 2>/dev/null); then
    echo 0 0
    exit 0
fi

# cover.url names the cover in place. cover.want names the cover that the
# last attempt fetched, and its time is the time of that attempt. A new cover
# starts at once. A cover that failed starts again after a minute.
url=$(playerctl metadata mpris:artUrl 2>/dev/null)
if [ -n "$url" ] && [ "$url" != "$(cat "$cover.url" 2>/dev/null)" ]; then
    if [ "$url" != "$(cat "$cover.want" 2>/dev/null)" ] ||
        [ -z "$(find "$cover.want" -mmin -1 2>/dev/null)" ]; then
        mkdir -p "${cover%/*}"
        printf '%s\n' "$url" > "$cover.want"
        case "$url" in
        # A local copy is fast, so the cover is in place when phonto reads
        # the output. `phonto dump` then shows it too.
        file://*) fetch "$url" > /dev/null 2>&1 ;;
        # A download runs in the background, so this script stays fast. It
        # must not keep the output of the script open.
        *) fetch "$url" > /dev/null 2>&1 & ;;
        esac
    fi
fi

position=$(playerctl position 2>/dev/null || echo 0)
length=$(playerctl metadata mpris:length 2>/dev/null || echo 0)
[ "$status" = Playing ] && playing=1 || playing=0
# The position is in seconds, and the length is in microseconds.
awk -v p="$position" -v l="$length" -v on="$playing" \
    'BEGIN { printf "%d %.4f\n", on, (l > 0 ? p * 1000000 / l : 0) }'
