# Phonto

> phonto (/'fon.to/) — from Greek φόντο: background

GPU-accelerated video wallpaper program for wayland compositors written in rust

Phonto plays videos as your desktop background with minimal overhead, decoding and rendering entirely on the GPU through gstreamer and EGL.

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

## Usage

```bash
phonto /path/to/video
```
