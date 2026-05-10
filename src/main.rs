mod decoder;
mod gl_renderer;
mod phonto;
mod wayland;

use phonto::Phonto;

const WALLPAPER_PATH: &str = "/home/plo/dotfiles/wallpapers/animated/night-city.mp4";

fn main() -> anyhow::Result<()> {
    env_logger::init();
    let mut phonto = Phonto::new()?;
    phonto.play(String::from(WALLPAPER_PATH))
}
