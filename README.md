# Phonto

> phonto (/'fon.to/), from the Greek φόντο: background

phonto plays a video as your desktop wallpaper, or draws an interactive one from a GLSL shader, on Wayland compositors and on macOS. The GPU does the work. On Linux, GStreamer decodes the video and phonto draws it through EGL. On macOS, VideoToolbox decodes it and CoreAnimation draws it.

An interactive wallpaper is a GLSL shader that phonto draws on your desktop. It can react to the time, the pointer, the keyboard, your battery, the sound that your computer plays and the output of your own commands. [Pinakothiki](https://pinakothiki.museslabs.org) is the gallery of phonto wallpapers.

![example](./mac-example.gif)

The documentation is at [museslabs.org/docs/phonto](https://museslabs.org/docs/phonto/).

## Installation

With Nix, on NixOS and on macOS with nix-darwin:

```bash
nix profile install github:museslabs/phonto
```

With Homebrew, on macOS and on Linux on x86_64:

```bash
brew tap museslabs/phonto
brew install phonto
```

From the AUR, on Arch Linux:

```bash
yay -S phonto
```

With cargo:

```bash
cargo install phonto
```

On macOS, run `MACOSX_DEPLOYMENT_TARGET=14.2 cargo install phonto` instead.

On Linux, phonto needs a Wayland compositor with wlr-layer-shell, such as Hyprland, Sway, niri, river or KDE Plasma, and the GStreamer plugins for your videos. On macOS, phonto needs version 14.2 or later. [Installing](https://museslabs.org/docs/phonto/installing/) lists the packages for each system.

## Usage

```bash
phonto ~/Videos/harbour.mp4                        # a video on every display
phonto "https://www.youtube.com/watch?v=VIDEO_ID"  # a YouTube video, through yt-dlp
phonto --rand                                      # a random video from your search paths
phonto pinakothiki:koi                             # an interactive wallpaper from Pinakothiki
phonto install koi                                 # keep it
phonto settings dashboard edit                     # choose the commands that a wallpaper runs
phonto --span pinakothiki:koi                      # one scene across all your displays
phonto displays                                    # the IDs of your displays
phonto dump ~/Videos/harbour.mp4 -o frame.png      # one frame, as an image
```

phonto plays until you stop it. `phonto --help` lists every flag.

## Documentation

- [A first wallpaper](https://museslabs.org/docs/phonto/first-wallpaper/): a video, a random pick, and how to start phonto with your session.
- [Multiple displays](https://museslabs.org/docs/phonto/displays/), [lock screens](https://museslabs.org/docs/phonto/lock-screens/) and [battery](https://museslabs.org/docs/phonto/battery/).
- [Interactive wallpapers](https://museslabs.org/docs/phonto/interactive-wallpapers/), [Pinakothiki](https://museslabs.org/docs/phonto/pinakothiki/) and [wallpaper settings](https://museslabs.org/docs/phonto/wallpaper-settings/).
- [Making a wallpaper](https://museslabs.org/docs/phonto/making-wallpapers/), and [publishing it](https://museslabs.org/docs/phonto/publishing/) on Pinakothiki.
- [Configuration](https://museslabs.org/docs/phonto/configuration/) and the [command line](https://museslabs.org/docs/phonto/command-line/).
- [For other programs](https://museslabs.org/docs/phonto/other-programs/): the JSON output, for apps that use phonto.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

phonto is GPL-3.0-or-later. See [LICENSE](LICENSE).
