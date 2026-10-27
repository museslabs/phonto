mod backend;
mod cli;
mod config;
mod displays;
mod plan;
mod scale;
mod state;
mod wallpaper;

use clap::Parser;

use cli::{Cli, Command};

use crate::backend::Backend;

fn play(args: cli::PlayArgs, yt_dlp: cli::YtDlpArgs) -> anyhow::Result<()> {
    let config = config::load()?;
    let plan = plan::build(args.source.into(), &config)?;
    let playback = plan::resolve(plan, &config.search_paths, &yt_dlp.into())?;

    // Per-display playback has no single "current" path, so skip the cache.
    if let plan::Playback::Mirror(path) = &playback {
        state::write_current(path);
    }

    let options = backend::RunOptions {
        pause: args.power.pause_mode(),
        scale: args.render.scale,
        #[cfg(target_os = "linux")]
        layer: args.render.layer,
    };
    backend::create(args.render.backend_options())?.run(playback, options)
}

fn dump(args: cli::DumpArgs, yt_dlp: cli::YtDlpArgs) -> anyhow::Result<()> {
    let config = config::load()?;
    let options = args.backend_options();
    let path = plan::resolve_source(
        plan::Source::Path(args.path),
        &config.search_paths,
        &yt_dlp.into(),
    )?;
    backend::create(options)?.dump(path, args.at, args.out)
}

fn main() -> anyhow::Result<()> {
    env_logger::init();

    let cli = Cli::parse();
    match cli.command {
        // No subcommand: play a wallpaper.
        None => play(cli.play, cli.yt_dlp),
        Some(Command::Displays) => {
            displays::print(&backend::Platform::list_displays()?);
            Ok(())
        }
        Some(Command::Dump(args)) => dump(args, cli.yt_dlp),
        #[cfg(target_os = "macos")]
        Some(Command::InstallLiveLockscreen {
            video,
            name,
            remove,
        }) => backend::macos::live_lockscreen::install::run(video, name, remove),
    }
}
