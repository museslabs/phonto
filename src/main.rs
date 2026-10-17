mod backend;
mod cli;
mod commands;
mod config;
mod displays;
mod plan;
mod scale;
mod state;
mod wallpaper;

use clap::Parser;

use cli::{Cli, Command};

fn main() -> anyhow::Result<()> {
    env_logger::init();

    let cli = Cli::parse();
    match cli.command {
        // No subcommand: play a wallpaper.
        None => commands::play::run(cli.play, cli.yt_dlp),
        Some(Command::Displays) => commands::displays::run(),
        Some(Command::Dump(args)) => commands::dump::run(args, cli.yt_dlp),
        #[cfg(target_os = "macos")]
        Some(Command::InstallLiveLockscreen {
            video,
            name,
            remove,
        }) => backend::macos::live_lockscreen::install::run(video, name, remove),
    }
}
