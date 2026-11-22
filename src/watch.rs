//! Suivi du dossier affiché : le système signale les suppressions, ajouts (restauration depuis
//! la corbeille…), renommages et modifications de fichiers, appliqués aussitôt, sans attendre
//! de revenir dans la visionneuse.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};

use eframe::egui;
use notify::event::{EventKind, ModifyKind, RenameMode};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};

/// Une copie ou un enregistrement produit une rafale d'événements : on attend que le dossier
/// soit calme depuis ce délai (en secondes) avant d'en tenir compte.
const QUIET: f64 = 0.2;

/// Changements survenus dans le dossier depuis le dernier relevé.
#[derive(Default)]
pub struct Changes {
    /// Fichiers renommés (ancien chemin, nouveau chemin), dans l'ordre.
    pub renamed: Vec<(PathBuf, PathBuf)>,
    /// Fichiers créés ou dont le contenu a changé : à décoder de nouveau.
    pub modified: Vec<PathBuf>,
}

impl Changes {
    /// Certains systèmes signalent un renommage en deux temps puis d'un bloc : une seule fois.
    fn add_rename(&mut self, from: PathBuf, to: PathBuf) {
        let rename = (from, to);
        if self.renamed.last() != Some(&rename) {
            self.renamed.push(rename);
        }
    }
}

pub struct DirWatcher {
    dir: PathBuf,
    /// Arrête le suivi en étant libéré.
    _watcher: RecommendedWatcher,
    rx: Receiver<notify::Event>,
    pending: Option<Changes>,
    /// Ancien nom d'un renommage dont le nouveau n'est pas encore arrivé.
    rename_from: Option<PathBuf>,
    last_event: f64,
}

impl DirWatcher {
    /// `None` si le système refuse de suivre le dossier : la liste n'est alors relue qu'à
    /// l'ouverture.
    pub fn new(dir: &Path, ctx: &egui::Context) -> Option<Self> {
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            if let Ok(event) = event {
                let _ = tx.send(event);
                ctx.request_repaint();
            }
        })
        .ok()?;
        watcher.watch(dir, RecursiveMode::NonRecursive).ok()?;
        Some(Self {
            dir: dir.to_path_buf(),
            _watcher: watcher,
            rx,
            pending: None,
            rename_from: None,
            last_event: 0.0,
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Changements à appliquer, une fois le dossier calme ; `now` : horloge d'egui.
    pub fn poll(&mut self, ctx: &egui::Context, now: f64) -> Option<Changes> {
        while let Ok(event) = self.rx.try_recv() {
            self.last_event = now;
            let changes = self.pending.get_or_insert_default();
            match event.kind {
                EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => {
                    if let [from, to] = &event.paths[..] {
                        changes.add_rename(from.clone(), to.clone());
                    }
                }
                EventKind::Modify(ModifyKind::Name(RenameMode::From)) => {
                    self.rename_from = event.paths.into_iter().next();
                }
                EventKind::Modify(ModifyKind::Name(RenameMode::To)) => {
                    for to in event.paths {
                        match self.rename_from.take() {
                            Some(from) => changes.add_rename(from, to),
                            None => changes.modified.push(to),
                        }
                    }
                }
                // Métadonnées seules (dates, droits) : l'image n'a pas changé.
                EventKind::Modify(ModifyKind::Metadata(_)) | EventKind::Access(_) => {}
                EventKind::Create(_) | EventKind::Modify(_) | EventKind::Any | EventKind::Other => {
                    changes.modified.extend(event.paths);
                }
                EventKind::Remove(_) => {}
            }
        }
        self.pending.as_ref()?;
        let wait = self.last_event + QUIET - now;
        if wait > 0.0 {
            ctx.request_repaint_after_secs(wait as f32);
            return None;
        }
        self.pending.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Attend (au plus 5 s) les changements signalés par le système.
    fn wait(watcher: &mut DirWatcher, ctx: &egui::Context) -> Changes {
        let start = std::time::Instant::now();
        loop {
            // Horloge fictive bien après le dernier événement : pas d'attente du calme.
            if let Some(changes) = watcher.poll(ctx, start.elapsed().as_secs_f64() + 1000.0) {
                return changes;
            }
            assert!(start.elapsed().as_secs() < 5, "aucun changement signalé");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    #[test]
    fn reports_renames_and_modifications() {
        let dir = std::env::temp_dir().join(format!("heic-viewer-watch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.jpg"), b"1").unwrap();
        let ctx = egui::Context::default();
        let mut watcher = DirWatcher::new(&dir, &ctx).expect("suivi du dossier");

        std::fs::rename(dir.join("a.jpg"), dir.join("b.jpg")).unwrap();
        let changes = wait(&mut watcher, &ctx);
        assert_eq!(changes.renamed, [(dir.join("a.jpg"), dir.join("b.jpg"))]);

        std::fs::write(dir.join("b.jpg"), b"2").unwrap();
        assert!(wait(&mut watcher, &ctx).modified.contains(&dir.join("b.jpg")));

        std::fs::remove_file(dir.join("b.jpg")).unwrap();
        let changes = wait(&mut watcher, &ctx);
        assert!(changes.renamed.is_empty());
        std::fs::remove_dir(&dir).unwrap();
    }
}
