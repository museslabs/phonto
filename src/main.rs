mod backend;
mod phonto;

use clap::Parser;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Path to the video file
    path: String,
}

fn main() -> anyhow::Result<()> {
    env_logger::init();
    let args = Args::parse();
    phonto::run(args.path)
}
