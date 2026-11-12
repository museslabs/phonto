//! Loads the image of a watched channel again when its file changes. A
//! script can then put album art, an icon, or rendered text into a scene.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use super::pack::{Texture, decode_texture, empty_texture};

/// How often the watcher checks the files.
const POLL: Duration = Duration::from_secs(1);

/// New images for the watched textures of a pack. A thread checks the files
/// and decodes the images, so the drawing thread only uploads them.
pub struct TextureWatcher {
    updates: Arc<Mutex<Vec<(usize, Texture)>>>,
}

impl TextureWatcher {
    /// Starts watching the textures that set `watch`. `None` if there are
    /// none.
    pub fn start(textures: &[Texture]) -> Option<Self> {
        Self::start_polling(textures, POLL)
    }

    fn start_polling(textures: &[Texture], poll: Duration) -> Option<Self> {
        let mut watched: Vec<(usize, PathBuf, Option<SystemTime>)> = textures
            .iter()
            .enumerate()
            .filter(|(_, t)| t.watch)
            .map(|(i, t)| (i, t.path.clone(), t.modified))
            .collect();
        if watched.is_empty() {
            return None;
        }
        let updates = Arc::new(Mutex::new(Vec::new()));
        let shared = Arc::clone(&updates);
        let spawned = std::thread::Builder::new()
            .name("texture-watch".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(poll);
                    for (index, path, seen) in &mut watched {
                        let now = modified(path);
                        if now == *seen {
                            continue;
                        }
                        *seen = now;
                        // A file that goes away empties the channel, as
                        // before the file first appeared. A file that a
                        // script still writes can fail to decode. Its next
                        // change loads it again.
                        let texture = match now {
                            None => empty_texture(path),
                            Some(_) => match decode_texture(path) {
                                Ok(texture) => texture,
                                Err(err) => {
                                    log::warn!("{err:#}");
                                    continue;
                                }
                            },
                        };
                        if let Ok(mut updates) = shared.lock() {
                            updates.retain(|(i, _)| i != index);
                            updates.push((*index, texture));
                        }
                    }
                }
            });
        if let Err(err) = spawned {
            log::warn!("cannot watch the textures: {err}");
            return None;
        }
        Some(Self { updates })
    }

    /// The new images since the last call, with the index of each texture.
    pub fn take(&self) -> Vec<(usize, Texture)> {
        self.updates
            .lock()
            .map(|mut u| std::mem::take(&mut *u))
            .unwrap_or_default()
    }
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes a new file and renames it, so the watcher never reads half
    /// of an image.
    fn save(path: &Path, width: u32) {
        let rgba = vec![255; (width * 2 * 4) as usize];
        let partial = path.with_extension("partial.png");
        image::save_buffer(&partial, &rgba, width, 2, image::ColorType::Rgba8).unwrap();
        std::fs::rename(partial, path).unwrap();
    }

    #[test]
    fn reloads_a_changed_or_new_file() {
        let dir = std::env::temp_dir().join(format!("phonto-watch-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (cover, later) = (dir.join("cover.png"), dir.join("later.png"));
        save(&cover, 4);
        let mut first = decode_texture(&cover).unwrap();
        first.watch = true;
        let missing = Texture {
            path: later.clone(),
            watch: true,
            modified: None,
            width: 1,
            height: 1,
            rgba: vec![0; 4],
        };
        let watcher =
            TextureWatcher::start_polling(&[first, missing], Duration::from_millis(20)).unwrap();

        // Change the first file, and make the second one.
        std::thread::sleep(Duration::from_millis(50));
        save(&cover, 8);
        save(&later, 3);
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let mut sizes = Vec::new();
        while sizes.len() < 2 {
            assert!(std::time::Instant::now() < deadline, "got {sizes:?}");
            sizes.extend(watcher.take().into_iter().map(|(i, t)| (i, t.width)));
            std::thread::sleep(Duration::from_millis(10));
        }
        sizes.sort();
        assert_eq!(sizes, [(0, 8), (1, 3)]);

        // A file that goes away empties its channel.
        std::fs::remove_file(&cover).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let emptied = loop {
            assert!(std::time::Instant::now() < deadline, "the channel stayed");
            if let Some((i, t)) = watcher.take().into_iter().next() {
                break (i, t.width, t.rgba);
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(emptied, (0, 1, vec![0; 4]));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
