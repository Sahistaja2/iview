//! Kausta pildid: loend (üks `read_dir`, Windowsis tulevad suurus ja aeg
//! samast päringust — iga faili eraldi ei küsita), loomulik järjestus
//! („IMG_2" enne „IMG_10") ja kõrvalkaust kausta lõpus.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use crate::config::{SortBy, has_ext};

pub struct Item {
    pub path: PathBuf,
    pub size: u64,
    pub modified: Option<SystemTime>,
    /// Ctrl+V-ga kleebitud pilt: ainult mälus, `path` on nimi kaustas, kuhu
    /// Ctrl+S salvestaks (faili pole).
    pub clip: Option<Arc<image::DynamicImage>>,
}

impl Item {
    pub fn file(path: PathBuf) -> Self {
        let meta = std::fs::metadata(&path).ok();
        Item { size: meta.as_ref().map_or(0, |m| m.len()), modified: meta.and_then(|m| m.modified().ok()), path, clip: None }
    }
    pub fn file_name(&self) -> String {
        self.path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
    }
    pub fn stem(&self) -> String {
        self.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
    }
    /// Laiend koos punktiga („.jpg") või tühi.
    pub fn ext(&self) -> String {
        self.path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default()
    }
}

/// Kausta pildifailid (peidetud välja) valitud järjestuses.
pub fn list(dir: &Path, exts: &[String], sort: SortBy) -> Vec<Item> {
    let Ok(read) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut items: Vec<Item> = read
        .filter_map(Result::ok)
        .filter_map(|e| {
            let path = e.path();
            if !has_ext(&path, exts) {
                return None;
            }
            let meta = e.metadata().ok()?;
            if !meta.is_file() || hidden(&meta) {
                return None;
            }
            Some(Item { path, size: meta.len(), modified: meta.modified().ok(), clip: None })
        })
        .collect();
    sort_items(&mut items, sort);
    items
}

pub fn sort_items(items: &mut [Item], sort: SortBy) {
    let by_name = |a: &Item, b: &Item| natural_cmp(&a.file_name(), &b.file_name());
    match sort {
        SortBy::Name => items.sort_by(by_name),
        SortBy::Modified => items.sort_by(|a, b| a.modified.cmp(&b.modified).then_with(|| by_name(a, b))),
        SortBy::Size => items.sort_by(|a, b| a.size.cmp(&b.size).then_with(|| by_name(a, b))),
    }
}

#[cfg(windows)]
fn hidden(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    meta.file_attributes() & 2 != 0
}

#[cfg(not(windows))]
fn hidden(_meta: &std::fs::Metadata) -> bool {
    false
}

/// Kausta lõpus: kõrvalkaust (nime järjekorras), kus on pilte, ja selle
/// esimene (edasi) või viimane (tagasi) pilt.
pub fn sibling_folder(dir: &Path, forward: bool, exts: &[String], sort: SortBy) -> Option<(PathBuf, PathBuf)> {
    let parent = dir.parent()?;
    let mut folders: Vec<PathBuf> = std::fs::read_dir(parent)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| e.path())
        .filter(|p| !crate::platform::is_hidden(p))
        .collect();
    folders.sort_by(|a, b| natural_cmp(&a.file_name().unwrap_or_default().to_string_lossy(), &b.file_name().unwrap_or_default().to_string_lossy()));
    let here = folders.iter().position(|f| f == dir)?;
    let candidates: Box<dyn Iterator<Item = &PathBuf>> = if forward { Box::new(folders[here + 1..].iter()) } else { Box::new(folders[..here].iter().rev()) };
    for folder in candidates {
        let files = list(folder, exts, sort);
        let file = if forward { files.first() } else { files.last() };
        if let Some(file) = file {
            return Some((folder.clone(), file.path.clone()));
        }
    }
    None
}

/// Loomulik võrdlus: numbrid arvuna („2" < „10"), tähed tõstutundetult.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut s = String::new();
                    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
                        s.push(c);
                        it.next();
                    }
                    s
                };
                let (n, m) = (take(&mut x), take(&mut y));
                let (nt, mt) = (n.trim_start_matches('0'), m.trim_start_matches('0'));
                let order = nt.len().cmp(&mt.len()).then_with(|| nt.cmp(mt)).then_with(|| n.len().cmp(&m.len()));
                if order != Ordering::Equal {
                    return order;
                }
            }
            (Some(c), Some(d)) => {
                let order = c.to_lowercase().cmp(d.to_lowercase());
                if order != Ordering::Equal {
                    return order;
                }
                x.next();
                y.next();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order() {
        let mut v = vec!["IMG_10.jpg", "img_2.jpg", "IMG_1.jpg", "IMG_02.jpg", "a.jpg"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, ["a.jpg", "IMG_1.jpg", "img_2.jpg", "IMG_02.jpg", "IMG_10.jpg"]);
    }

    /// Loendis ainult laiendiga failid; järgmine kaust jätab pildita kausta vahele.
    #[test]
    fn list_and_sibling_folders() {
        let root = tempfile::tempdir().unwrap();
        let (a, b, c) = (root.path().join("2020_01"), root.path().join("2020_02"), root.path().join("2020_10"));
        for d in [&a, &b, &c] {
            std::fs::create_dir(d).unwrap();
        }
        std::fs::write(a.join("x10.jpg"), b"x").unwrap();
        std::fs::write(a.join("x9.JPG"), b"x").unwrap();
        std::fs::write(a.join("loe.txt"), b"x").unwrap();
        std::fs::write(c.join("y.png"), b"x").unwrap();
        let exts = crate::config::exts_of("jpg png");
        let items = list(&a, &exts, SortBy::Name);
        let names: Vec<String> = items.iter().map(Item::file_name).collect();
        assert_eq!(names, ["x9.JPG", "x10.jpg"]);
        assert_eq!(sibling_folder(&a, true, &exts, SortBy::Name), Some((c.clone(), c.join("y.png"))));
        assert_eq!(sibling_folder(&c, false, &exts, SortBy::Name), Some((a.clone(), a.join("x10.jpg"))));
        assert_eq!(sibling_folder(&c, true, &exts, SortBy::Name), None);
    }
}
