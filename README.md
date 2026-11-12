


# Phonto

> phonto (/'fon.to/) — from Greek φόντο: background

GPU-accelerated video wallpaper program for Wayland compositors and macOS, written in Rust. Also supports live lockscreen wallpapers on macOS, and interactive procedural (shader) wallpapers.


![example](./mac-example.gif)


On Linux, phonto plays videos as your desktop background with minimal overhead, decoding and rendering entirely on the GPU through GStreamer and EGL. On macOS it drives an `AVPlayerLayer` attached to a window sitting just below the system wallpaper level, so VideoToolbox handles decoding and CoreAnimation handles compositing.

## Installation

Using Nix (NixOS and macOS with nix-darwin):

```bash
nix profile install github:museslabs/phonto
```

Using Homebrew (macOS and Linux x86_64):

```bash
brew tap museslabs/phonto
brew install phonto
```

Using AUR on Arch Linux:

```bash
yay -S phonto
```

Using cargo:

```bash
cargo install phonto
```

From source:

```bash
git clone https://github.com/museslabs/phonto
cd phonto
cargo build --release
```

## Dependencies

### Linux (Wayland)

Phonto requires GStreamer runtime plugins in addition to the VA-API plugin used for GPU-accelerated decoding. On Linux, missing demuxer or codec plugins can cause startup failures  when opening MP4/H.264 files. Without the VA-API plugin, GStreamer falls back to software decoding and CPU usage will be significantly higher.

**Arch Linux:**
```bash
sudo pacman -S gstreamer gst-plugins-base gst-plugins-good gst-plugins-bad gst-libav gst-plugin-va
```

`gst-plugins-good` provides `qtdemux` for MP4 files, and `gst-plugins-bad` includes the H.264 parser used by many videos.

**Ubuntu/Debian:**
```bash
sudo apt install gstreamer1.0-vaapi gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-plugins-bad gstreamer1.0-libav
```

**Fedora:**
```bash
sudo dnf install gstreamer1-vaapi gstreamer1-plugins-base gstreamer1-plugins-good gstreamer1-plugins-bad-free gstreamer1-libav
```

### macOS

No external dependencies. phonto links against system frameworks (`AVFoundation`, `CoreMedia`, `AppKit`, `QuartzCore`). Decoding goes through VideoToolbox automatically for codecs the OS supports (H.264, HEVC, ProRes, etc.).

## Usage

Play a specific video:
```bash
phonto /path/to/video.mp4
```

Play a YouTube video or livestream (requires yt-dlp):
```bash
phonto "https://www.youtube.com/watch?v=dQw4w9WgXcQ"
phonto youtu.be/dQw4w9WgXcQ    # bare URL, no https:// needed
```

Play a streaming URL directly (video-only, GStreamer handles it natively):
```bash
phonto https://example.com/stream.m3u8
phonto rtsp://camera.local/stream
```

Play a procedural wallpaper, a GLSL shader that phonto draws live (see
[Procedural wallpapers](#procedural-wallpapers)):
```bash
phonto examples/procedural/plasma.glsl
phonto examples/procedural/life        # a pack directory with a manifest.toml
```

Play a random wallpaper from your configured search paths:
```bash
phonto --rand
```

On Linux/Wayland, choose the layer-shell layer to render on:
```bash
phonto /path/to/video.mp4 --layer background
```

Available layers are `background` (default), `bottom`, `top`, and `overlay`.

Dump a still frame to an image file:
```bash
phonto dump PATH --out OUT.png [--at SECONDS] [--shader PATH] [--size WIDTHxHEIGHT]
```

`dump` supports local files, resolved URLs and procedural wallpapers. `--out`/`-o`
is required, `--at` selects the timestamp in seconds (default `0.0`), and `--shader`
applies a GLSL fragment shader before saving (only available on Wayland/Linux).
`--size` sets the image size for procedural wallpapers (default `1920x1080`).
Videos keep their own size.

Phonto also writes the currently selected video path to `~/.cache/phonto/current`.
This is useful when `--rand` chooses a wallpaper and another tool needs to reuse
the same video. For per-display playback there is no single "current" video, so the
cache file is skipped in that mode.

## Streaming & YouTube (yt-dlp)

YouTube URLs are resolved automatically through [yt-dlp](https://github.com/yt-dlp/yt-dlp). phonto detects `youtube.com`, `youtu.be`, and `m.youtube.com` — with or without `https://` prefix — and spawns `yt-dlp -g` to extract the direct stream URL. YouTube requires a browser-like User-Agent to avoid 403 errors, which phonto sets on the GStreamer pipeline automatically.

**Dependencies:** yt-dlp must be installed and on your `$PATH`.

```bash
# Arch Linux
sudo pacman -S yt-dlp

# macOS/Linux
brew install yt-dlp

# pip (any platform)
pip install yt-dlp
```

### Authentication

YouTube may block yt-dlp without browser cookies. Pass `--cookies-from-browser` to use your logged-in session:

```bash
phonto --cookies-from-browser firefox "youtube.com/watch?v=..."
phonto --cookies-from-browser chrome youtu.be/...
```

### Quality

By default phonto uses the default yt-dlp quality. Override with `--yt-dlp-format`:

```bash
phonto --yt-dlp-format "bestvideo" "youtube.com/..."         # best video
phonto --yt-dlp-format "best[height<=720]" "youtube.com/..." # cap at 720p
phonto --yt-dlp-format "worst" "youtube.com/..."             # lowest quality
```

### Extra yt-dlp arguments

Pass arbitrary flags through to yt-dlp with `--yt-dlp-args`:

```bash
phonto --yt-dlp-args "--no-check-certificate" "youtube.com/..."
phonto --yt-dlp-args "--proxy socks5://127.0.0.1:9050" "youtube.com/..."
```

For more options, see [yt-dlp's Usage and Options](https://github.com/yt-dlp/yt-dlp#usage-and-options).

### How it works

When phonto detects a YouTube URL it calls `yt-dlp -f <format> -g [--cookies-from-browser <browser>] [extra args] <url>` and takes the first stream URL from stdout. That URL feeds into GStreamer's `uridecodebin` with GPU decoding and rendering — the same pipeline used for local files.

## Multi-monitor

By default phonto mirrors a single video across every connected display. New
monitors plugged in mid-session attach automatically, and unplugging then
re-plugging a monitor brings the wallpaper back without restart. A procedural
wallpaper can also cover all displays as one scene (see
[One scene across all displays](#one-scene-across-all-displays)).

### Listing displays

```bash
phonto displays
```

The first column is the native ID for the current OS. You can pass that string
straight into `--display`, or wire it into an `[[alias]]` block ([see below](#cross-platform-aliases)) to
share one config across machines.

On macOS the ID is the display's name. The description starts with the
display's UUID, which works as an ID too. After it comes the display's top-left
corner in the arrangement from System Settings. That tells you which row is
which monitor.

### Two monitors of the same model on macOS

macOS numbers their names, like `DELL U2723QE (1)` and `DELL U2723QE (2)`, but
only while both are connected. Unplug one and the other is `DELL U2723QE`
again. A name copied with both connected matches nothing when phonto starts
with one, and the other way around. Pin each monitor by its UUID instead. It
stays with the monitor when macOS renames it.

Monitors that report the same serial number, or none, may still swap UUIDs.
`system_profiler -json SPDisplaysDataType` lists each monitor's serial as
`_spdisplays_display-serial-number`.

If a monitor matches two entries, one by UUID and one by name, the UUID's video
plays.

### Pinning videos per display from the CLI

Repeat `--display ID PATH` for each display you want to set. Every display must
be named explicitly. There is no implicit default.

```bash
phonto --display "DELL U2723QE" /path/to/ocean.mp4 \
       --display "eDP-1" /path/to/forest.mp4
```

The first value is the display ID (as printed by `phonto displays`, or an alias
name from your config) and the second is the video path.

For a random video on one specific display, use `--display-rand ID`. It picks
from your `search_paths` the same way `--rand` does.

```bash
phonto --display "DELL U2723QE" /path/to/pinned.mp4 \
       --display-rand "eDP-1"
```

`--display` and `--display-rand` can be mixed and repeated. Displays you don't
mention get no wallpaper from phonto and keep whatever the OS is showing there.

### Persistent assignment via config

The same assignments belong in `config.toml` under `[[display]]` blocks. Then
`phonto` with no arguments reads them.

```toml
[[display]]
id = "DELL U2723QE"
path = "~/Videos/ocean.mp4"

[[display]]
id = "eDP-1"
random = true
```

Each `[[display]]` needs exactly one of `path` or `random = true`. Phonto errors
at startup if both or neither are set.

A `[[display]]` whose display is not currently connected is not an error.
Phonto waits and attaches the wallpaper when that display appears.

### Cross-platform aliases

Native display IDs differ across operating systems. The same monitor might be
`"DELL U2723QE"` on macOS and `"DP-1"` on a wlroots compositor. Alias blocks
let you give a portable name to a physical display once and reference it from
`[[display]].id`.

```toml
[[alias]]
name = "main"
wayland = "DP-1"
macos = "DELL U2723QE"

[[alias]]
name = "laptop"
wayland = "eDP-1"
macos = "Built-in Retina Display"

[[display]]
id = "main"
path = "~/Videos/ocean.mp4"

[[display]]
id = "laptop"
random = true
```

Run `phonto displays` on each OS once to copy the right native ID into the
alias. The same dotfile then works on both. Either field is optional, so a
laptop-only alias can leave the other one out.

## Procedural wallpapers

phonto can also draw a GLSL shader as your wallpaper, live, instead of a
video. The shaders use the conventions of [Shadertoy](https://www.shadertoy.com),
so most Shadertoy shaders work after a copy and paste. A shader can react to
the pointer. It can also keep state from one frame to the next, for
simulations such as the Game of Life.

Examples are in [`examples/procedural`](examples/procedural):

```bash
phonto examples/procedural/plasma.glsl   # one file
phonto examples/procedural/life          # a pack: a buffer pass and an image pass
phonto examples/procedural/koi           # a koi pond in the light of the real sun
phonto examples/procedural/museslabs     # the Muses Labs lyre plays your system audio
phonto examples/procedural/battery       # your battery as the tide in a glass
phonto examples/procedural/system        # the load of your computer, from input commands
phonto examples/procedural/album         # the cover of the music that plays, as a live image
phonto --span examples/procedural/koi    # one pond across all your displays
```

Each example shows some of the features:

| Example | Features |
|---|---|
| plasma | a single file, the pointer |
| life | buffers, a buffer that reads itself |
| koi | buffers, the pointer, `iLocation` and the sun, `--span` |
| museslabs | audio and chroma, textures, the dark mode setting, `--span` |
| battery | `iBattery`, the dark mode setting, a text atlas |
| system | input commands, buffers, the dark mode setting |
| album | a live image, an input command, audio |

The koi pond shows the most features. If you click the water, food drops, and
the koi swim to it and feed. If you hold the button and drag, a wake follows
the pointer. Moving the pointer without a click does nothing. The light
follows the sun, from dawn to a night with a moon and fireflies. Dawn comes at
the sunrise of your `[location]`, and dusk at its sunset, so the light changes
with the seasons. To preview a different hour, set `HOUR_OVERRIDE` at the top
of `examples/procedural/koi/common.glsl`. With `--span`, the koi swim from one
display to the next, and the moon shows on the display at the right.

The Muses Labs lyre uses the colors and the type of museslabs.org. Its three
strings play the audio that your system plays. The left string follows the
bass, the middle string the midrange, and the right string the treble. A
pluck (a fast rise in the sound) makes a string vibrate and glow. It also
writes the chord that plays on the staff, from the two or three strongest
note names in `iChroma`.

A note that comes soon after the last one is an eighth note, and a slower
note is a quarter note. In a full mix, some chords are wrong, because the
strongest note names do not always come from the melody. When the audio
stops, the strings stop and the notes fade.

The lyre follows your clock. It uses the paper colors of the site from
06:00 and the dark colors of the site from 12:00.

To change the hours, set `LIGHT_FROM` and `DARK_FROM` at the top of
`examples/procedural/museslabs/common.glsl`. `THEME` in the same file changes
the rule. 1 is always paper, 2 is always dark, 3 follows the dark mode of your
desktop, and 4 follows the sun. To preview the lyre without audio, for example
with `dump`, set `DEMO` to 1 in the same file. With `--span`, the lyre stands
on the display nearest the middle of the desktop, and the staff crosses all
the displays. `LYRE_DISPLAY` chooses another display.

The battery is the tide in a glass. The water level is the charge, bubbles
rise while the battery charges, and the water turns terracotta below 20%. A
system without a battery shows a full glass. The colors follow the dark mode
of your desktop. `DEMO` at the top of `examples/procedural/battery/image.glsl`
shows each state in turn.

The system example shows the load of your computer as an orrery. Three
planets go around faster when the load of the last 1, 5, and 15 minutes is
higher for each core, and the outer ring shows how full the disk is. The
values come from input commands that work on Linux and on macOS. Add them to
your configuration first:

```bash
cat examples/procedural/system/inputs.toml >> ~/.config/phonto/config.toml
```

The album example shows the cover of the music that plays, on a background of
its own colors. A line under the cover shows how far the track is. It needs
`playerctl`, which reads every player that supports MPRIS, such as Spotify,
Firefox, mpv, and VLC. Its input command is a script. It prints whether music
plays and how far the track is, and it keeps the cover in
`~/.cache/phonto/cover`, which the pack watches. Set it up first:

```bash
cp examples/procedural/album/now-playing.sh ~/.config/phonto/
cat examples/procedural/album/inputs.toml >> ~/.config/phonto/config.toml
```

A pack that sets `audio = true` reads the monitor of your default output. A
monitor is a copy of the sound that goes to your speakers. phonto reads it
through the GStreamer PulseAudio plugin, which also works with PipeWire
through `pipewire-pulse`. On Debian or Ubuntu, install
`gstreamer1.0-pulseaudio`. Arch and Fedora include the plugin in their
GStreamer good plugins package.

phonto selects the default output when it starts. If you change outputs,
restart phonto. If the monitor is not available, phonto logs a warning and
draws the scene without audio. `dump` always draws without audio. While
phonto reads the monitor, the sound server keeps your output device awake. A
battery pause stops the reading.

You can use a procedural wallpaper everywhere that you can use a video path:
`--display ID PATH`, `[[display]].path`, and `dump`, and on Wayland also the
hyprlock recipe and `--shader` post-processing. `--rand` also picks pack
directories, and `.glsl` files that have a `mainImage` function. A procedural
wallpaper always fills the screen, so `--scale` has no effect. The same
wallpapers run on Wayland and macOS. macOS does not have `--span` yet, and it
does not read the audio.

### One scene across all displays

`--span` draws a procedural wallpaper once for the whole desktop. Each display
then shows its own part of the scene:

```bash
phonto --span examples/procedural/koi
```

phonto gets the place of each display from the compositor, through the
xdg-output protocol. `iResolution`, `fragCoord`, `iMouse`, and `iPointer` then
use the pixels of the whole desktop, so most shaders span without changes. The
buffers also cover the whole desktop, so a simulation such as the Game of Life
continues from one display to the next. The cost is the same as one copy on
each display, plus any gaps between the displays.

A centered scene falls on the edge between two displays. `iDisplays` gives
the place of each display in the scene, so a pack can put its main subject
inside one display, as the lyre and the battery do. It can also give each
display its own vignette, as the koi pond does.

`--span` does not work with `--display`, or with the `[[display]]` entries of
the configuration, because they give each display its own wallpaper. A video
still plays on each display.

`--span` works only on Wayland so far.

### Single-file wallpapers

A single-file wallpaper is a `.glsl` file with the `mainImage` function of
Shadertoy:

```glsl
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    fragColor = vec4(0.5 + 0.5 * cos(iTime + uv.xyx + vec3(0, 2, 4)), 1.0);
}
```

Do not declare the `#version` line, the default precisions, or the uniforms in
the table below. phonto adds them. If the file has a `#version` line, phonto
ignores it. The code is GLSL ES 3.00, the language of WebGL 2. A single file
draws at the full output resolution, at a maximum of 60 fps.

### Inputs

Uniforms are the inputs that phonto sets before each frame:

| Uniform | Type | Meaning |
|---|---|---|
| `iResolution` | `vec3` | The size of this pass in pixels. `z` is 1. |
| `iImageResolution` | `vec3` | Only in phonto. The size of the image pass. It is different from `iResolution` in a buffer with its own `size` or `height`. |
| `iTime` | `float` | Seconds of playback. It stops during a pause. It goes back to 0 after 24 hours, to keep float precision. |
| `iTimeDelta` | `float` | Seconds since the last frame. The maximum is 0.25, or two frame periods below 8 fps. |
| `iFrameRate` | `float` | Frames per second, smoothed. |
| `iFrame` | `int` | The number of frames drawn on this display. It starts at 0 again when the display changes size. |
| `iDate` | `vec4` | Year, month (0 to 11), day (1 to 31), and seconds since local midnight. |
| `iMouse` | `vec4` | The same as on Shadertoy. `xy` is the drag position and `zw` is the click position. `z` is positive while the left button is pressed. `w` is positive only on the frame of the click. |
| `iPointer` | `vec4` | Only in phonto. `xy` is the pointer position. `z` is 1 while the pointer is over the wallpaper. `w` is 1 while the left button is pressed. Use it for hover effects. |
| `iAudio` | `vec4` | Only in phonto. The smoothed bass, midrange, treble, and overall volume, each from 0 to 1. If the pack does not set `audio = true`, the values are zero. |
| `iSpectrum[32]` | `float[32]` | Only in phonto. 32 smoothed frequency bands from 40 Hz to 11 kHz, with a logarithmic spacing. Each value is from 0 to 1. If the pack does not set `audio = true`, the values are zero. |
| `iChroma[12]` | `float[12]` | Only in phonto. The strength of each note name, from C to B, over all octaves from 130 Hz to 5 kHz. Each value is from 0 to 1, and the strongest note name is near 1 while sound plays. If the pack does not set `audio = true`, the values are zero. |
| `iColorScheme` | `float` | Only in phonto. 1 if your desktop prefers dark colors. 0 if it prefers light colors or has no preference. A change shows at once. |
| `iBattery` | `vec4` | Only in phonto. `x` is the charge from 0 to 1. `y` is 1 while the battery charges. `z` is 1 on AC power. `w` is 1 if the system has a battery. Without a battery, `x` is 1. phonto reads the battery every 10 seconds. |
| `iLocation` | `vec4` | Only in phonto. `x` is the latitude and `y` is the longitude, in degrees. `z` is the offset of your local time from UTC, in hours. `w` is 1 if you set a `[location]` in the configuration. |
| `iDisplays[8]` | `vec4` | Only in phonto. The place of each display in the image: `x`, `y`, `width`, and `height`, in the pixels of the image pass, from the bottom left. Without `--span`, there is one display, the whole image. |
| `iDisplayCount` | `int` | Only in phonto. The number of displays in `iDisplays`. |
| `iInput` | struct | Only in phonto. The inputs that the pack names in `[inputs]`, as `iInput.<name>` (see [Your own inputs](#your-own-inputs)). |
| `iChannel0` to `iChannel7` | `sampler2D` | The channels of the pass (see Packs). A channel without an input is black. Shadertoy has four channels. Channels 4 to 7 are only in phonto. |
| `iChannelResolution[8]` | `vec3` | The size of each channel in pixels. |

`fragCoord` is in the pixels of the pass, with the origin at the bottom left,
as on Shadertoy. `iMouse` and `iPointer` are always in the pixels of the image
pass. In a buffer with its own size, multiply them by
`iResolution.xy / iImageResolution.xy`.

phonto reads the dark mode setting through the XDG desktop portal. GNOME,
KDE, and the GTK portal that other desktops use all give it. If no portal
gives it, `iColorScheme` is 0.

On macOS, `iColorScheme` follows System Settings > Appearance, including
Auto.

With `iLocation` and `iDate`, a shader can compute the sun. The function
`sun()` in `examples/procedural/museslabs/common.glsl` gives the height and
the direction of the sun, and the times of sunrise and sunset. Copy it into
your pack. Without a `[location]`, `iLocation` is the equator at the middle
of your time zone, so the sun rises near 06:00 and sets near 18:00. `dump`
uses the system inputs of the moment that it runs, as it does for `iDate`.

On Wayland, the wallpaper gets the pointer only where no window covers it,
because Wayland does not tell other clients where the pointer is.

On macOS, `iPointer.xy` follows the pointer everywhere on the screen, also
under windows, because a translucent window shows the wallpaper. `iPointer.z`
is 1 only where the pointer is over the desktop. A click there presses the
button of `iMouse`. A click on a window does not. phonto needs no permission
for this. Finder still gets the click too, so it deselects your desktop icons,
and with the default setting macOS moves all windows away to show the desktop.
For wallpapers that you click, such as the koi pond, set System Settings >
Desktop & Dock > Click wallpaper to reveal desktop to "Only in Stage Manager".

On macOS, phonto does not read the system audio yet. `iAudio`, `iSpectrum`,
and `iChroma` are 0 there.

phonto reads the keyboard only on macOS, through a keyboard channel (see
[Packs](#packs)). `iChannelTime` and `iSampleRate` exist only so that shaders
that declare sound inputs compile.

### Packs

A pack is a directory with a `manifest.toml`, shaders, and optional images. It
adds up to eight buffer passes. A buffer pass (a buffer) draws offscreen, and
other passes can read its output. A buffer can read its own last frame, so it
can keep state. The image pass draws the result on screen.

```toml
name = "Life"              # optional, the default is the directory name
fps = 30                   # frame-rate cap, 0 = refresh rate of the display (default 60)
render_scale = 0.25        # draw at a fraction of the output size, 0 < scale <= 1 (default 1)
common = "common.glsl"     # optional, added before the code of every pass
audio = false              # true = read the audio that the system plays (default false)

[inputs]                   # optional, values from your [[input]] commands, with defaults
workspace = 1.0            # a float
wind = [0.0, 0.0]          # a vec2. Lists of 2 to 4 numbers are vec2 to vec4

[[buffer]]                 # 0 to 8, drawn in this order every frame
name = "cells"
shader = "cells.glsl"
channels = [{ buffer = "cells", filter = "nearest", wrap = "repeat" }]

[image]                    # the pass on screen
shader = "image.glsl"
channels = ["cells", "noise.png"]
```

A buffer draws at the size of the image pass, unless it sets one of these
keys:

| Key | Meaning |
|---|---|
| `size = [16, 2]` | A fixed size in pixels, for state such as one texel for each creature. |
| `height = 300` | A fixed height. The width follows the aspect ratio of the screen, so a simulation behaves the same at all resolutions. |
| `format` | `rgba16f` (default), `rgba32f` for precise state such as positions, or `rgba8` to use less memory. If the GPU cannot draw to the format, phonto uses a smaller format and writes a warning. |
| `once = true` | The buffer draws only when the wallpaper starts or the display changes size. Then it keeps the result. Use it for content that does not change, such as a background. |

`channels[n]` is the input of `iChannelN`, up to eight channels. A channel is
a string or a table. A string is a buffer name, or else the path of an image
in the pack. A table has exactly one of `buffer = "name"`,
`texture = "file.png"` or `keyboard = true`, and optional sampling keys:

| Key | Values | Default |
|---|---|---|
| `filter` | `nearest`, `linear`, `mipmap` (only for textures) | `linear` for buffers, `mipmap` for textures, `nearest` for the keyboard |
| `wrap` | `clamp`, `repeat`, `mirror` | `clamp` for buffers and the keyboard, `repeat` for textures |
| `watch` | `true` loads the image again when its file changes (see [Live images](#live-images)) | `false` |

The keyboard channel works only on macOS so far. It is the keyboard texture
of Shadertoy: 256 texels wide, one for each JavaScript key code, and 3 rows
high. Row 0 is 1 while a key is down, row 1 is 1 on the frame after a press,
and row 2 flips on each press. Read it with
`texelFetch(iChannelN, ivec2(code, row), 0).x`. A wallpaper only gets the
keyboard after you click the desktop, as a Shadertoy canvas gets it after a
click. A click on a window, or a switch to another app, gives the keyboard
back. While the wallpaper has it, Finder's desktop shortcuts, such as
<kbd>⇧⌘N</kbd> for a new folder, do not work. phonto needs no permission for
this.

If a buffer already drew in this frame, a pass that reads it gets the output
of this frame. If a pass reads itself, or a buffer that draws later, it gets
the output of the last frame. Shadertoy works the same way. By default, a
buffer stores 16-bit floats, so it can hold values outside 0 to 1. Images
(PNG, JPEG, and the other formats of the `image` crate) load with
`uv = (0, 0)` at their bottom-left corner.

Each display gets its own buffers at its own size, so the state of a wallpaper
is different on each display. With `--span`, all displays share one set of
buffers. A shader compile error gives the file and the
line. For example, `0:12` is line 12 of the shader of the pass, and `1:12` is
line 12 of the common file.

### Your own inputs

A pack can read values that your own commands give. Examples are the
weather, the CPU load, the count of unread mail, and the workspace. You write
the commands in your configuration. phonto runs each one with `sh` at its
interval, and reads the one to four numbers that it prints. Commands come
only from your configuration, never from a pack, because packs can come from
the internet.

```toml
# ~/.config/phonto/config.toml
[[input]]
name = "workspace"
command = "hyprctl activeworkspace -j | jq .id"
every = 1                  # seconds between two runs

[[input]]
name = "wind"
command = "curl -s 'https://api.open-meteo.com/v1/forecast?latitude=37.98&longitude=23.73&current=wind_speed_10m,wind_direction_10m' | jq '.current.wind_speed_10m, .current.wind_direction_10m'"
every = 900
```

The pack names the inputs that it reads in `[inputs]`, with a default for
each. The shader then reads `iInput.workspace` and `iInput.wind`:

```glsl
float calm = 1.0 - clamp(iInput.wind.x / 40.0, 0.0, 1.0);
```

Spaces, commas, and brackets separate the numbers, so a JSON list also works.
`true` and `false` count as 1 and 0. phonto runs only the commands for the
inputs of the wallpaper that plays. A battery pause also pauses the commands.
If a command fails or prints no number, the input keeps its last value, and
phonto writes one warning. A command that runs longer than its interval
stops, with all the programs that it started. This limit is at least 1 second
and at most 10 seconds. An input without a command keeps its default. `dump` runs each command once.

### Live images

A texture channel with `watch = true` loads its image again when the file
changes. A script can then put album art, a weather icon, or text into the
scene. The path can be absolute, or it can start with `~/`:

```toml
[image]
shader = "image.glsl"
channels = [{ texture = "~/.cache/phonto/cover.jpg", watch = true }]
```

phonto checks the file each second. Write a new file and rename it into
place, so that phonto never reads half of an image. If the file does not
exist yet, the channel is empty until it appears. If the new image does not
load, the channel keeps the last image, and phonto writes a warning. To show
text, render it to an image, for example with ImageMagick:

```bash
magick -background none -fill white -pointsize 64 label:"3 unread" /tmp/mail.png &&
  mv /tmp/mail.png ~/.cache/phonto/mail.png
```

### Performance

A video costs almost nothing, because the hardware decodes it. A shader costs
the work that it does for each pixel in each frame, so a complex shader can
keep the GPU busy. To make a wallpaper cheaper:

- Lower `fps`. 30 fps is enough for slow motion.
- Lower `render_scale`. At 0.5, phonto draws a quarter of the pixels and
  scales them up with linear filtering.
- Give smooth buffers a smaller `height`. Draw content that does not change in
  a `once = true` buffer.
- Use `--pause-on-battery` or `--pause-below`. Time stops during the pause and
  continues after it.

On macOS, the output size is the size of the screen in points, not in pixels.
On a Retina display that looks like 2560x1440, a pass draws 2560x1440 pixels,
the same as Wayland at that size, and macOS scales them up to the screen.

## Lock screen backgrounds with hyprlock (Wayland)

Hyprlock owns the lock-screen UI, but phonto can render the animated background
on a higher layer while hyprlock is active. Start a second phonto process on the
`overlay` layer before launching hyprlock:

```bash
phonto "$(cat "$HOME/.cache/phonto/current")" --layer overlay &
phonto_lock_pid=$!

hyprlock
kill "$phonto_lock_pid"
```

This pairs well with a normal desktop phonto session running on the default
`background` layer. If you use `phonto --rand`, the cache file lets the lock
screen reuse the same randomly selected wallpaper.

Hyprlock must leave its background transparent or partially transparent for the
video to be visible behind its controls.

## Live lockscreen wallpapers (macOS)

The lock screen is owned by `loginwindow`, which hides every non-Apple-signed window the moment the screen locks. Phonto sidesteps that by registering your video into Apple's aerial catalog, so Apple's own signed extension plays it on both the desktop and the lock screen.

```bash
phonto install-live-lockscreen /path/to/video.mp4
```

This transcodes the video to HEVC Main10 with two temporal sub-layers (the exact bitstream shape the aerials extension requires for multi-cycle playback), generates a thumbnail, and registers the asset under a **Phonto** section in System Settings → Wallpaper. It then restarts `WallpaperAgent` so macOS picks up the new entry, which makes your desktop wallpaper redraw once. The encode keeps the source's resolution, frame rate and colours. Its bitrate follows the footage, up to 160 Mbps. Expect 125 to 250 MB for 20 seconds of typical 4K footage, and up to 400 MB for the noisiest.

To activate, open **System Settings → Wallpaper**, pick the **Phonto** category, click your entry, then lock the screen (Apple menu → Lock Screen) to verify it plays across cycles.

To remove a previously-installed entry:

```bash
phonto install-live-lockscreen /path/to/video.mp4 --remove
```

Optional flags:

- `--name <STRING>` — display name shown in the picker (defaults to the file stem).

## Configuration

Phonto reads its config from `$XDG_CONFIG_HOME/phonto/config.toml`, falling back to `~/.config/phonto/config.toml`.

### `search_paths`

A list of directories to scan when using `--rand`. Each entry has a `path` and a `depth` controlling how many levels deep to search.

```toml
[[search_paths]]
path = "/home/user/wallpapers"
depth = 1

[[search_paths]]
path = "/mnt/media/videos"
depth = 2
```

`depth = 0` scans only the top-level directory. `depth = 1` includes one level of subdirectories, and so on.

### `location`

Your place on Earth, in degrees, with north and east positive. Procedural
wallpapers get it in `iLocation`, so that they can compute the sun. phonto
does not send your location anywhere.

```toml
[location]
latitude = 37.98
longitude = 23.73
```

### `input`

Commands that give procedural wallpapers values, as `iInput.<name>`. Each
entry has a `name`, a shell `command` that prints one to four numbers, and
`every`, the seconds between two runs. See
[Your own inputs](#your-own-inputs).

### Path expansion

Every path field in the config (`[[search_paths]].path`, `[[display]].path`)
and every path on the CLI (positional, `--display ID PATH`, `--shader`)
accepts a leading `~/` which expands to `$HOME`. The same
`~/Downloads/wall.mp4` works on macOS (`/Users/you`) and Linux (`/home/you`)
without editing the dotfile.

### GLSL shaders (Wayland only)

`--shader PATH` applies a custom GLSL ES fragment shader to every frame. Pass the path to any `.glsl` file:

```bash
phonto /path/to/video.mp4 --shader ~/.config/phonto/edge-detect.glsl
```

Shaders must target **GLSL ES 3.00** — start every shader with `#version 300 es`. Use `in` for inputs, declare an `out vec4` for the output colour, and use `texture()` (not `texture2D()`).

Your shader has access to:

- `u_tex` (`sampler2D`) — the current video frame.
- `v_uv` (`vec2`) — UV coordinates for the current fragment.
- `u_resolution` (`vec2`, optional) — surface size in pixels, useful for computing per-texel offsets (`1.0 / u_resolution`).

An example sobel edge detection shader:

```glsl
#version 300 es
precision mediump float;
uniform sampler2D u_tex;
uniform vec2 u_resolution;
in vec2 v_uv;
out vec4 frag_color;
void main() {
    vec2 px = 1.0 / u_resolution;
    float tl = dot(texture(u_tex, v_uv + vec2(-px.x,  px.y)).rgb, vec3(0.299, 0.587, 0.114));
    float tc = dot(texture(u_tex, v_uv + vec2(  0.0,  px.y)).rgb, vec3(0.299, 0.587, 0.114));
    float tr = dot(texture(u_tex, v_uv + vec2( px.x,  px.y)).rgb, vec3(0.299, 0.587, 0.114));
    float ml = dot(texture(u_tex, v_uv + vec2(-px.x,   0.0)).rgb, vec3(0.299, 0.587, 0.114));
    float mr = dot(texture(u_tex, v_uv + vec2( px.x,   0.0)).rgb, vec3(0.299, 0.587, 0.114));
    float bl = dot(texture(u_tex, v_uv + vec2(-px.x, -px.y)).rgb, vec3(0.299, 0.587, 0.114));
    float bc = dot(texture(u_tex, v_uv + vec2(  0.0, -px.y)).rgb, vec3(0.299, 0.587, 0.114));
    float br = dot(texture(u_tex, v_uv + vec2( px.x, -px.y)).rgb, vec3(0.299, 0.587, 0.114));
    float gx = -tl - 2.0*ml - bl + tr + 2.0*mr + br;
    float gy = -tl - 2.0*tc - tr + bl + 2.0*bc + br;
    frag_color = vec4(vec3(sqrt(gx*gx + gy*gy)), 1.0);
}
```

Note: multi-pass effects (separable blur, bloom) are not supported, the shader runs once per frame with no intermediate framebuffers.

### Fit modes

`--scale` controls how the video fits to the screen:

- `stretch` — distort to fill, aspect not preserved.
- `fit` — preserve aspect, letterbox to fit inside.
- `fill` — preserve aspect, crop overflow to fill (default).
- `center` — render at native pixel size, centered.

```bash
phonto /path/to/video --scale fit
```

On macOS, `center` currently falls back to `fill` (`AVPlayerLayer` has no native equivalent).

### Battery pausing

Stop playback when the laptop is unplugged or charge drops below a threshold. The two flags are mutually exclusive.

- `--pause-on-battery` — pause whenever the system is on battery, resume on AC.
- `--pause-below PERCENT` — pause when on battery *and* charge is below `PERCENT` (1–100). Always plays on AC.

```bash
phonto /path/to/video --pause-on-battery
phonto /path/to/video --pause-below 30
```
