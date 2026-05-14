mod decoder;
mod gl_renderer;
mod phonto;
mod wayland;

use clap::Parser;
use phonto::Phonto;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Path to the video file
    path: String,
}

fn main() -> anyhow::Result<()> {
    env_logger::init();
    let args = Args::parse();
    let mut phonto = Phonto::new()?;
    phonto.play(String::from(args.path))
}
