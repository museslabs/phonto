# Phonto

Live wallpaper program for wayland compositors written in rust.

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

## Usage

```bash
phonto /path/to/video
```
