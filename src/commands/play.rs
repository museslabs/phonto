use crate::backend::{self, Backend, RunOptions};
use crate::cli::{PlayArgs, YtDlpArgs};
use crate::config;
use crate::plan::{self, Playback};
use crate::state;

pub fn run(args: PlayArgs, yt_dlp: YtDlpArgs) -> anyhow::Result<()> {
    let config = config::load()?;
    let plan = plan::build(args.source.into(), &config)?;
    let playback = plan::resolve(plan, &config.search_paths, &yt_dlp.into())?;

    // Per-display playback has no single "current" path, so skip the cache.
    if let Playback::Mirror(path) = &playback {
        state::write_current(path);
    }

    let options = RunOptions {
        pause: args.power.pause_mode(),
        scale: args.render.scale,
    };
    backend::create(args.render.backend_options())?.run(playback, options)
}
