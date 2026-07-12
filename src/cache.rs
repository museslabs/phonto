use anyhow::Context;
use serde::Serialize;

const CACHE_PATH: &str = ".cache/phonto";

#[derive(Serialize)]
struct Cache<'a> {
    path: &'a str,
    shader: Option<&'a str>,
}

pub fn write(path: &str, shader: Option<&str>) -> anyhow::Result<()> {
    let home = std::env::var("HOME")?;
    let cache_dir = std::path::Path::new(&home).join(CACHE_PATH);

    std::fs::create_dir_all(&cache_dir)
        .context(format!("could not create cache dir {CACHE_PATH}"))?;

    std::fs::write(
        cache_dir.join("current.json"),
        serde_json::to_string(&Cache { path, shader })?,
    )
    .context("could not create current.json")?;

    Ok(())
}
