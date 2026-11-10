use std::path::{Path, PathBuf};

use rand::seq::IndexedRandom;

use crate::config::SearchPath;
use crate::procedural;

const WALLPAPER_EXTENSIONS: &[&str] = &["mp4", "mkv", "webm", "avi", "mov", "gif", "ogv"];

pub fn collect(search_paths: &[SearchPath]) -> Vec<PathBuf> {
    let mut wallpapers = Vec::new();
    for sp in search_paths {
        let expanded = crate::config::expand_tilde(&sp.path);
        let root = Path::new(&expanded);
        // A search path can be a pack. Then it is one wallpaper, and its
        // shaders do not work alone.
        if procedural::SUPPORTED && procedural::is_pack_dir(root) {
            wallpapers.push(root.to_path_buf());
            continue;
        }
        walk(root, sp.depth, 0, &mut wallpapers);
    }
    wallpapers
}

pub fn pick_random(search_paths: &[SearchPath]) -> Option<PathBuf> {
    let wallpapers = collect(search_paths);
    wallpapers.choose(&mut rand::rng()).cloned()
}

fn walk(dir: &Path, max_depth: u32, depth: u32, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // A procedural pack counts as one wallpaper, like a video file.
            if procedural::SUPPORTED && procedural::is_pack_dir(&path) {
                out.push(path);
            } else if depth < max_depth {
                walk(&path, max_depth, depth + 1, out);
            }
        } else if path.is_file() && is_wallpaper(&path) {
            out.push(path);
        }
    }
}

fn is_wallpaper(path: &Path) -> bool {
    let is_video = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| WALLPAPER_EXTENSIONS.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false);
    is_video || (procedural::SUPPORTED && procedural::is_procedural_shader_file(path))
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    const SHADER: &str = "void mainImage(out vec4 c, in vec2 p) { c = vec4(1.0); }";

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "phonto-wallpaper-test-{}-{tag}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn search(path: &Path, depth: u32) -> Vec<PathBuf> {
        let mut found = collect(&[SearchPath {
            path: path.to_string_lossy().into_owned(),
            depth,
        }]);
        found.sort();
        found
    }

    #[test]
    fn search_path_that_is_a_pack_is_one_wallpaper() {
        let pack = temp_dir("root-pack");
        std::fs::write(
            pack.join(procedural::MANIFEST),
            "[image]\nshader = \"image.glsl\"",
        )
        .unwrap();
        std::fs::write(pack.join("image.glsl"), SHADER).unwrap();
        std::fs::write(pack.join("cells.glsl"), SHADER).unwrap();

        assert_eq!(search(&pack, 1), vec![pack.clone()]);
        let _ = std::fs::remove_dir_all(&pack);
    }

    #[test]
    fn packs_and_shaders_inside_a_search_path() {
        let root = temp_dir("mixed");
        let pack = root.join("pond");
        std::fs::create_dir_all(&pack).unwrap();
        std::fs::write(
            pack.join(procedural::MANIFEST),
            "[image]\nshader = \"image.glsl\"",
        )
        .unwrap();
        std::fs::write(pack.join("image.glsl"), SHADER).unwrap();
        std::fs::write(root.join("plasma.glsl"), SHADER).unwrap();
        // A post-processing shader has no mainImage and is not a wallpaper.
        std::fs::write(root.join("edge.glsl"), "void main() {}").unwrap();
        std::fs::write(root.join("clip.mp4"), "").unwrap();

        let expected = vec![root.join("clip.mp4"), root.join("plasma.glsl"), pack];
        assert_eq!(search(&root, 0), expected);
        let _ = std::fs::remove_dir_all(&root);
    }
}
