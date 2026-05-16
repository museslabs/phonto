use crate::backend;

pub fn run(video_path: String) -> anyhow::Result<()> {
    let backend = backend::init()?;
    backend.run(video_path)
}
