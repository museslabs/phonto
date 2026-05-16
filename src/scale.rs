use clap::ValueEnum;

#[derive(Copy, Clone, Debug, ValueEnum)]
pub enum ScaleMode {
    /// Distort the video to fill the screen exactly. Aspect not preserved.
    Stretch,
    /// Preserve aspect, fit entire video inside the screen with letterbox bars.
    Fit,
    /// Preserve aspect, fill the screen by cropping overflow.
    Fill,
    /// Render at native pixel size, centered. Cropped if larger than screen.
    Center,
}
