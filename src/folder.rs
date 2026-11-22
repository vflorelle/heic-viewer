//! Contexte de navigation : la liste des images du dossier de l'image affichée.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use crate::decode::is_supported;

pub struct Folder {
    pub files: Vec<PathBuf>,
    pub index: usize,
}

impl Folder {
    /// Construit le contexte à partir d'un fichier (ses voisins du même dossier)
    /// ou d'un dossier (sa première image). `None` si aucune image trouvée.
    pub fn open(path: &Path) -> Option<Self> {
        // Chemins absolus : ce sont aussi ceux que signale le suivi du dossier.
        let path = &std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let (dir, selected) = if path.is_dir() {
            (path.to_path_buf(), None)
        } else {
            (path.parent()?.to_path_buf(), Some(path))
        };

        let mut files = list_images(&dir);
        let index = match selected {
            Some(sel) => match files.iter().position(|f| same_file(f, sel)) {
                Some(i) => i,
                // Fichier à l'extension inconnue : on le garde quand même dans la liste.
                None => {
                    files.push(sel.to_path_buf());
                    files.sort_by(|a, b| natural_cmp_paths(a, b));
                    files.iter().position(|f| f == sel)?
                }
            },
            None if files.is_empty() => return None,
            None => 0,
        };
        Some(Self { files, index })
    }

    /// Relit le dossier après des changements. L'image courante est suivie si elle a été
    /// renommée (`renamed` : ancien et nouveau chemin) ; si elle a disparu, la suivante (dans
    /// l'ordre d'avant) encore présente la remplace. `false` si le dossier n'a plus d'image.
    pub fn refresh(&mut self, renamed: &[(PathBuf, PathBuf)]) -> bool {
        let Some(dir) = self.current().parent().map(Path::to_path_buf) else {
            return true;
        };
        let mut current = self.current().to_path_buf();
        for (from, to) in renamed {
            if *from == current {
                current = to.clone();
            }
        }
        let mut files = list_images(&dir);
        // Fichier à l'extension inconnue, ouvert explicitement : gardé tant qu'il existe.
        if current.is_file() && !files.contains(&current) {
            files.push(current.clone());
            files.sort_by(|a, b| natural_cmp_paths(a, b));
        }
        let (up_to, following) = self.files.split_at(self.index + 1);
        let index = files.iter().position(|f| *f == current).or_else(|| {
            following
                .iter()
                .chain(up_to[..self.index].iter().rev())
                .find_map(|old| files.iter().position(|f| f == old))
        });
        self.index = match index {
            Some(i) => i,
            None if files.is_empty() => return false,
            None => self.index.min(files.len() - 1),
        };
        self.files = files;
        true
    }

    pub fn current(&self) -> &Path {
        &self.files[self.index]
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Décale l'index de `delta` en bouclant aux extrémités.
    pub fn step(&mut self, delta: isize) {
        let n = self.files.len() as isize;
        self.index = (self.index as isize + delta).rem_euclid(n) as usize;
    }

    pub fn go_to(&mut self, index: usize) {
        self.index = index.min(self.files.len() - 1);
    }
}

fn list_images(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.is_file() && is_supported(p))
                .collect()
        })
        .unwrap_or_default();
    files.sort_by(|a, b| natural_cmp_paths(a, b));
    files
}

/// Même fichier, même écrit autrement (chemin relatif, casse sous Windows…).
pub fn same_file(a: &Path, b: &Path) -> bool {
    a == b
        || match (a.canonicalize(), b.canonicalize()) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
}

fn natural_cmp_paths(a: &Path, b: &Path) -> Ordering {
    let name = |p: &Path| {
        p.file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    };
    natural_cmp(&name(a), &name(b))
}

/// Tri « naturel » : IMG_9.HEIC < IMG_10.HEIC.
fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let take_num = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut s = String::new();
                    while let Some(c) = it.next_if(char::is_ascii_digit) {
                        s.push(c);
                    }
                    s
                };
                let (na, nb) = (take_num(&mut a), take_num(&mut b));
                let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                let ord = ta
                    .len()
                    .cmp(&tb.len())
                    .then_with(|| ta.cmp(tb))
                    .then_with(|| na.len().cmp(&nb.len()));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(x), Some(y)) => {
                if x != y {
                    return x.cmp(&y);
                }
                a.next();
                b.next();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_sort() {
        let mut v = vec!["img_10.heic", "img_9.heic", "img_100.heic", "a.jpg", "img_010.heic"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(
            v,
            ["a.jpg", "img_9.heic", "img_10.heic", "img_010.heic", "img_100.heic"]
        );
    }

    #[test]
    fn refresh_follows_changes() {
        let dir = std::env::temp_dir().join(format!("heic-viewer-folder-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["a.jpg", "b.jpg", "c.jpg", "d.jpg"] {
            std::fs::write(dir.join(name), b"").unwrap();
        }
        let mut folder = Folder::open(&dir.join("b.jpg")).unwrap();
        let name = |f: &Folder| f.current().file_name().unwrap().to_string_lossy().into_owned();

        // Image affichée renommée : elle reste affichée, à sa nouvelle place dans l'ordre.
        std::fs::rename(dir.join("b.jpg"), dir.join("e.jpg")).unwrap();
        assert!(folder.refresh(&[(dir.join("b.jpg"), dir.join("e.jpg"))]));
        assert_eq!((name(&folder), folder.index, folder.len()), ("e.jpg".into(), 3, 4));

        // Supprimée : la suivante encore présente la remplace, sinon la précédente.
        std::fs::remove_file(dir.join("e.jpg")).unwrap();
        assert!(folder.refresh(&[]));
        assert_eq!((name(&folder), folder.len()), ("d.jpg".into(), 3));

        // Restaurée : elle refait partie de la liste.
        std::fs::write(dir.join("b.jpg"), b"").unwrap();
        assert!(folder.refresh(&[]));
        assert_eq!((name(&folder), folder.len()), ("d.jpg".into(), 4));

        for name in ["a.jpg", "b.jpg", "c.jpg", "d.jpg"] {
            std::fs::remove_file(dir.join(name)).unwrap();
        }
        assert!(!folder.refresh(&[]), "plus aucune image");
        std::fs::remove_dir(&dir).unwrap();
    }

    #[test]
    fn step_wraps_around() {
        let mut f = Folder {
            files: vec!["a".into(), "b".into(), "c".into()],
            index: 0,
        };
        f.step(-1);
        assert_eq!(f.index, 2);
        f.step(1);
        assert_eq!(f.index, 0);
    }
}
