


# Phonto

> phonto (/'fon.to/) — from Greek φόντο: background

GPU-accelerated video wallpaper program for Wayland compositors and macOS, written in Rust.




https://github.com/user-attachments/assets/efba2097-1bb9-46f4-ae45-8f8f67916992


On Linux, phonto plays videos as your desktop background with minimal overhead, decoding and rendering entirely on the GPU through GStreamer and EGL. On macOS it drives an `AVPlayerLayer` attached to a window sitting just below the system wallpaper level, so VideoToolbox handles decoding and CoreAnimation handles compositing.

## Installation

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

Phonto requires GStreamer and a VA-API GStreamer plugin for GPU-accelerated decoding. Without the VA-API plugin, GStreamer falls back to software decoding and CPU usage will be significantly higher.

**Arch Linux:**
```bash
sudo pacman -S gst-plugin-va
```

**Ubuntu/Debian:**
```bash
sudo apt install gstreamer1.0-vaapi
```

**Fedora:**
```bash
sudo dnf install gstreamer1-vaapi
```

### macOS

No external dependencies. phonto links against system frameworks (`AVFoundation`, `CoreMedia`, `AppKit`, `QuartzCore`). Decoding goes through VideoToolbox automatically for codecs the OS supports (H.264, HEVC, ProRes, etc.).

## Usage

Play a specific video:
```bash
phonto /path/to/video.mp4
```

Play a random wallpaper from your configured search paths:
```bash
phonto --rand
```

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
