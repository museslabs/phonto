mod backend;
mod cli;
mod config;
mod displays;
mod gallery;
mod plan;
mod procedural;
mod scale;
mod settings;
mod state;
mod wallpaper;

use clap::Parser;

use cli::{Cli, Command, GalleryCommand};

use crate::backend::Backend;

fn play(args: cli::PlayArgs, yt_dlp: cli::YtDlpArgs) -> anyhow::Result<()> {
    let config = config::load()?;
    let plan = plan::build(args.source.into(), &config)?;
    #[cfg(target_os = "macos")]
    let pause_behind = plan::pause_behind(&config)?;
    // [[display]] entries give each display its own wallpaper, so they
    // cannot share one scene. The CLI already rejects --span with --display.
    if args.render.span && matches!(plan, plan::Plan::PerDisplay(_)) {
        anyhow::bail!(
            "--span draws one wallpaper across all displays, so it does not work with the \
             [[display]] entries of the configuration. Pass the wallpaper path with --span"
        );
    }
    let playback = plan::resolve(
        plan,
        &config.search_paths,
        &yt_dlp.into(),
        config.gallery.as_deref(),
    )?;

    // The settings of the wallpapers from the gallery win over config.toml.
    let (inputs, actions) = settings::merge(config.inputs, config.actions, &playback.paths())?;

    // Per-display playback has no single "current" path, so skip the cache.
    if let plan::Playback::Mirror(path) = &playback {
        state::write_current(path);
    }

    let options = backend::RunOptions {
        pause: args.power.pause_mode(),
        scale: args.render.scale,
        #[cfg(target_os = "linux")]
        layer: args.render.layer,
        span: args.render.span,
        #[cfg(target_os = "macos")]
        pause_behind,
        location: config.location,
        inputs,
        actions,
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
        config.gallery.as_deref(),
    )?;
    let (inputs, actions) = settings::merge(config.inputs, config.actions, &[&path])?;
    backend::create(options)?.dump(backend::DumpRequest {
        path,
        at: args.at,
        out: args.out,
        size: args.size,
        location: config.location,
        inputs,
        actions,
    })
}

fn main() -> anyhow::Result<()> {
    env_logger::init();

    let cli = Cli::parse();
    // With --json, an error is JSON on stderr too, for the program that reads
    // the output.
    let json = cli.command.as_ref().is_some_and(Command::json);
    match run(cli) {
        Err(err) if json => {
            eprintln!("{}", serde_json::json!({ "error": format!("{err:#}") }));
            std::process::exit(1);
        }
        result => result,
    }
}

fn run(cli: Cli) -> anyhow::Result<()> {
    match cli.command {
        // No subcommand: play a wallpaper.
        None => play(cli.play, cli.yt_dlp),
        Some(Command::Displays) => {
            displays::print(&backend::Platform::list_displays()?);
            Ok(())
        }
        Some(Command::Dump(args)) => dump(args, cli.yt_dlp),
        Some(Command::Gallery(command)) => {
            let config = config::load()?;
            let base = config.gallery.as_deref();
            match command {
                GalleryCommand::List { json } => gallery::browse(&[], base, json),
                GalleryCommand::Search { words, json } => gallery::browse(&words, base, json),
                GalleryCommand::Show { wallpaper, json } => gallery::show(&wallpaper, base, json),
            }
        }
        Some(Command::List { json }) => gallery::list_installed(json),
        Some(Command::Install { wallpaper, json }) => {
            gallery::install(&wallpaper, config::load()?.gallery.as_deref(), json)
        }
        Some(Command::Uninstall {
            wallpaper,
            remove_settings,
            json,
        }) => {
            let config = config::load()?;
            gallery::uninstall(&wallpaper, config.gallery.as_deref(), remove_settings, json)
        }
        Some(Command::Settings(args)) => settings::run(&args.wallpaper, args.command, args.json),
        #[cfg(target_os = "macos")]
        Some(Command::InstallLiveLockscreen {
            video,
            name,
            remove,
        }) => backend::macos::live_lockscreen::install::run(video, name, remove),
    }
}
