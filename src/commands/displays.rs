use crate::displays;

pub fn run() -> anyhow::Result<()> {
    displays::print(&displays::list()?);
    Ok(())
}
