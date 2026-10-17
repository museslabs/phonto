mod backend;
mod cli;
mod config;
mod displays;
mod plan;
mod scale;
mod wallpaper;

use backend::{Backend, RunOptions};
use clap::Parser;

use cli::{Cli, Command};

fn main() -> anyhow::Result<()> {
    env_logger::init();

    let cli = Cli::parse();

    if let Some(cmd) = cli.command {
        match cmd {
            Command::Displays => {
                let detected = displays::list()?;
                displays::print(&detected);
                return Ok(());
            }
            Command::Dump(args) => {
                let config = config::load()?;
                let options = args.backend_options();
                let path = plan::resolve_source(
                    plan::Source::Path(args.path),
                    &config.search_paths,
                    &cli.yt_dlp.into(),
                )?;
                return backend::create(options)?.dump(path, args.at, args.out);
            }
            #[cfg(target_os = "macos")]
            Command::InstallLiveLockscreen {
                video,
                name,
                remove,
            } => return backend::macos::live_lockscreen::install::run(video, name, remove),
        }
    }

    let config = config::load()?;
    let plan = plan::build(cli.play.source.into(), &config)?;
    let playback = plan::resolve(plan, &config.search_paths, &cli.yt_dlp.into())?;

    // Persist the resolved path so other tools (e.g. hyprlock) can read it.
    // For per-display playback there's no single "current" path; skip the cache.
    if let plan::Playback::Mirror(ref path) = playback
        && let Ok(home) = std::env::var("HOME")
    {
        let cache_dir = std::path::Path::new(&home).join(".cache/phonto");
        if std::fs::create_dir_all(&cache_dir).is_ok() {
            let _ = std::fs::write(cache_dir.join("current"), path);
        }
    }

    let options = RunOptions {
        pause: cli.play.power.pause_mode(),
        scale: cli.play.render.scale,
    };
    backend::create(cli.play.render.backend_options())?.run(playback, options)
}
