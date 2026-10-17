use crate::backend::{self, Backend};
use crate::cli::{DumpArgs, YtDlpArgs};
use crate::config;
use crate::plan::{self, Source};

pub fn run(args: DumpArgs, yt_dlp: YtDlpArgs) -> anyhow::Result<()> {
    let config = config::load()?;
    let options = args.backend_options();
    let path = plan::resolve_source(
        Source::Path(args.path),
        &config.search_paths,
        &yt_dlp.into(),
    )?;
    backend::create(options)?.dump(path, args.at, args.out)
}
