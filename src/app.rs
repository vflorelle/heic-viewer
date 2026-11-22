//! Interface : affichage, zoom, navigation dans le dossier, glisser-déposer.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use eframe::egui::{
    self, Color32, Key, Mesh, PointerButton, Pos2, Rect, Sense, TextureHandle, TextureOptions, Vec2, ViewportCommand,
    pos2, vec2,
};

use crate::actions;
use crate::config::{Config, Language};
use crate::cursor::OsCursor;
use crate::decode::{Cancel, DecodedImage, EXTENSIONS, decode_file};
use crate::folder::{Folder, same_file};
use crate::properties::Properties;
use crate::sharp::{Prepared, SharpView, fit_scale, lanczos_size, prepare_fit};
use crate::watch::{Changes, DirWatcher};

pub const APP_NAME: &str = "HEIC Viewer";

const NUM_KEYS: [Key; 10] = [
    Key::Num0,
    Key::Num1,
    Key::Num2,
    Key::Num3,
    Key::Num4,
    Key::Num5,
    Key::Num6,
    Key::Num7,
    Key::Num8,
    Key::Num9,
];

pub type DecodeResult = Arc<Result<DecodedImage, String>>;

/// Image actuellement envoyée au GPU.
struct Shown {
    path: PathBuf,
    texture: Option<TextureHandle>,
    result: DecodeResult,
    /// Fichier modifié depuis : l'image est décodée de nouveau, l'ancienne reste affichée d'ici là.
    outdated: bool,
}

/// Zoom prédéfini actif (touches 1 à 9) et vue à restaurer si on rappuie sur la même touche.
struct ZoomPreset {
    level: u8,
    previous_zoom: Option<f32>,
    previous_pan: Vec2,
}

/// Actions déclenchables depuis le clavier, la souris, la barre d'outils ou le menu.
#[derive(Clone, Copy)]
enum Action {
    Open,
    Previous,
    Next,
    First,
    Last,
    ToggleFullscreen,
    RotateLeft,
    RotateRight,
    Fit,
    ZoomPreset(u8),
    ZoomBy(f32),
    CopyImage,
    CopyPath,
    RevealInFolder,
    ShowOnMap {
        lat: f64,
        lon: f64,
    },
    ImageProperties,
    #[cfg(windows)]
    FileProperties,
    Rename,
    /// Mise à la Corbeille, après confirmation.
    #[cfg(windows)]
    Delete,
    Options,
    About,
    /// Échap : ferme ce qui est ouvert (menu, propriétés, plein écran), sinon quitte.
    Escape,
    Quit,
}

/// Fenêtre modale ouverte : les raccourcis de la visionneuse sont alors suspendus.
enum Dialog {
    /// Renommage de l'image : nom saisi, erreur du dernier essai.
    Rename {
        path: PathBuf,
        name: String,
        error: Option<String>,
        /// À la première image : focus et sélection du nom sans l'extension.
        select: bool,
    },
    #[cfg(windows)]
    Delete(PathBuf),
    /// Options : langue choisie, appliquée à la validation.
    Options(Language),
    About,
}

pub struct ViewerApp {
    folder: Option<Folder>,
    shown: Option<Shown>,
    /// Version Lanczos de l'image affichée sous 100 %.
    sharp: SharpView,

    /// Images décodées (courante + voisines préchargées).
    cache: HashMap<PathBuf, DecodeResult>,
    /// Versions Lanczos préparées au décodage, pour l'affichage ajusté de ces images.
    prepared: HashMap<PathBuf, Prepared>,
    /// Décodages en cours, annulables si l'image n'est plus utile.
    pending: HashMap<PathBuf, Arc<Cancel>>,
    tx: Sender<(PathBuf, DecodeResult, Option<Prepared>)>,
    rx: Receiver<(PathBuf, DecodeResult, Option<Prepared>)>,

    /// `None` = ajusté à la fenêtre, sinon facteur d'échelle écran / texture.
    zoom: Option<f32>,
    pan: Vec2,
    zoom_preset: Option<ZoomPreset>,
    /// Rotation manuelle en quarts de tour (sens horaire).
    quarter_turns: u8,
    fullscreen: bool,
    /// Position et taille de la fenêtre avant le plein écran (si elle n'était pas agrandie).
    windowed: Option<(Pos2, Vec2)>,
    message: Option<String>,
    /// Vue et échelles calculées au dernier rendu (clavier, barre d'état).
    view_rect: Rect,
    last_fit: f32,
    last_scale: Option<f32>,
    /// L'image dépasse de la vue (zoomée) : les flèches du clavier la déplacent.
    overflows: bool,

    menu: Option<Pos2>,
    /// Le clic en cours a servi à fermer le menu : il ne doit rien faire d'autre.
    swallow_click: bool,
    properties: Option<Properties>,
    toast: Option<(String, f64)>,
    /// Plein écran : dernier changement d'image ou de zoom, pour afficher l'indicateur.
    osd_since: f64,
    /// Touche de navigation ou de rotation maintenue.
    held: Option<HeldKey>,
    /// Image demandée, pas encore affichée : +1 suivante, -1 précédente, 0 aucune. Les appuis
    /// ne s'additionnent pas : le dernier l'emporte, et rien ne continue après l'arrêt.
    step: isize,
    /// Instant où l'image courante est apparue à l'écran ; `None` le temps de l'image (au sens
    /// de l'affichage) où elle est envoyée à la carte graphique, parfois longue pour une grande
    /// photo : elle n'est réellement visible qu'à la fin de celle-ci.
    shown_at: Option<f64>,
    /// Échap ne quitte pas pendant un glisser-déposer.
    drag: DragGuard,
    /// Le curseur affiché est celui d'un glisser-déposer : pendant le survol, puis (glisser
    /// annulé) jusqu'au premier mouvement de la souris ou au retour du focus. La position n'est
    /// pas annoncée d'ici là, sinon la flèche dessinée s'ajouterait à ce curseur.
    drag_cursor: bool,
    /// Position réelle de la souris, quand egui ne la connaît pas : retour par Alt+Tab,
    /// changement de bureau virtuel, plein écran…
    cursor: OsCursor,
    /// Le sélecteur de fichier s'ouvre dans `ui`, qui dispose de la fenêtre parente.
    open_requested: bool,
    dialog: Option<Dialog>,
    config: Config,
    /// Suivi du dossier de l'image affichée (gardé s'il n'a plus d'image, pour qu'une image
    /// restaurée y réapparaisse).
    watcher: Option<DirWatcher>,
    /// Fenêtre parente de la confirmation de Windows (suppression définitive).
    #[cfg(windows)]
    hwnd: Option<windows::Win32::Foundation::HWND>,
    clipboard: Option<arboard::Clipboard>,
    /// Clic molette sur le bouton de la barre des tâches = nouvelle instance.
    #[cfg(windows)]
    relaunch: Option<crate::taskbar::Relaunch>,
}

impl ViewerApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        initial: Option<PathBuf>,
        (config, config_error): (Config, Option<String>),
    ) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let (tx, rx) = channel();
        let mut app = Self {
            folder: None,
            shown: None,
            sharp: SharpView::default(),
            cache: HashMap::new(),
            prepared: HashMap::new(),
            pending: HashMap::new(),
            tx,
            rx,
            zoom: None,
            pan: Vec2::ZERO,
            zoom_preset: None,
            quarter_turns: 0,
            fullscreen: false,
            windowed: None,
            message: None,
            view_rect: Rect::NOTHING,
            last_fit: 1.0,
            last_scale: None,
            overflows: false,
            menu: None,
            swallow_click: false,
            properties: None,
            toast: None,
            osd_since: f64::NEG_INFINITY,
            held: None,
            step: 0,
            shown_at: Some(f64::NEG_INFINITY),
            drag: DragGuard::default(),
            drag_cursor: false,
            cursor: OsCursor::new(cc),
            open_requested: false,
            dialog: None,
            config,
            watcher: None,
            #[cfg(windows)]
            hwnd: crate::win::hwnd(cc),
            clipboard: None,
            #[cfg(windows)]
            relaunch: crate::taskbar::Relaunch::enable(cc),
        };
        if let Some(path) = initial {
            app.open_path(&path);
        }
        if let Some(error) = config_error {
            app.notify(&cc.egui_ctx, Err(error));
        }
        app
    }

    /// Ouvre un fichier ou un dossier : le contexte de navigation devient son dossier.
    fn open_path(&mut self, path: &Path) {
        match Folder::open(path) {
            Some(folder) => {
                self.folder = Some(folder);
                self.message = None;
                self.step = 0;
            }
            None => {
                let path = path.display();
                self.message = Some(tr!("Aucune image trouvée dans {path}", "No image found in {path}"));
            }
        }
    }

    fn current_dir(&self) -> Option<&Path> {
        self.folder.as_ref().and_then(|f| f.current().parent())
    }

    /// Rattaché à la fenêtre : modal, et sans entrée supplémentaire dans la barre des tâches.
    fn open_dialog(&mut self, parent: &eframe::Frame) {
        let mut dialog = rfd::FileDialog::new()
            .set_parent(parent)
            .set_title(tr!("Ouvrir une image", "Open an image"))
            .add_filter("Images (HEIC, JPEG, PNG…)", EXTENSIONS)
            .add_filter(tr!("Tous les fichiers", "All files"), &["*"]);
        if let Some(dir) = self.current_dir() {
            dialog = dialog.set_directory(dir);
        }
        if let Some(path) = dialog.pick_file() {
            self.open_path(&path);
        }
    }

    fn current_path(&self) -> Option<PathBuf> {
        self.folder.as_ref().map(|f| f.current().to_path_buf())
    }

    /// Image courante décodée (hors chargement en cours).
    fn current_image(&self) -> Option<DecodeResult> {
        let shown = self.shown.as_ref()?;
        (Some(shown.path.as_path()) == self.folder.as_ref().map(Folder::current)).then(|| shown.result.clone())
    }

    fn run(&mut self, ctx: &egui::Context, action: Action) {
        match action {
            Action::Open => self.open_requested = true,
            Action::Previous | Action::Next => {
                self.step = if matches!(action, Action::Next) { 1 } else { -1 };
                ctx.request_repaint();
            }
            Action::First | Action::Last => {
                self.step = 0;
                let index = if matches!(action, Action::First) { 0 } else { usize::MAX };
                self.navigate(|f| f.go_to(index));
            }
            Action::ToggleFullscreen => {
                self.set_fullscreen(ctx, !self.fullscreen);
                self.osd_since = ctx.input(|i| i.time);
            }
            Action::RotateLeft => self.rotate(3),
            Action::RotateRight => self.rotate(1),
            Action::Fit => self.reset_view(),
            Action::ZoomPreset(level) => self.toggle_zoom_preset(level),
            Action::ZoomBy(factor) => {
                let scale = self.zoom.unwrap_or(self.last_fit) * factor;
                self.zoom_around(scale, None);
                self.zoom_preset = None;
            }
            Action::CopyImage => match self.current_image().as_deref() {
                Some(Ok(img)) => {
                    let r = actions::copy_image(&mut self.clipboard, img);
                    let done = tr!("Image copiée dans le presse-papiers", "Image copied to the clipboard");
                    self.notify(ctx, r.map(|()| done));
                }
                _ => self.notify(ctx, Err(tr!("aucune image à copier", "no image to copy"))),
            },
            Action::CopyPath => {
                if let Some(path) = self.current_path() {
                    let r = actions::copy_text(&mut self.clipboard, &path.to_string_lossy());
                    let done = tr!("Chemin copié dans le presse-papiers", "Path copied to the clipboard");
                    self.notify(ctx, r.map(|()| done));
                }
            }
            Action::RevealInFolder => {
                if let Some(path) = self.current_path()
                    && let Err(e) = actions::reveal_in_folder(&path)
                {
                    self.notify(ctx, Err(e));
                }
            }
            Action::ShowOnMap { lat, lon } => {
                let url = format!("https://www.openstreetmap.org/?mlat={lat}&mlon={lon}#map=16/{lat}/{lon}");
                if let Err(e) = actions::open_url(&url) {
                    self.notify(ctx, Err(e));
                }
            }
            Action::ImageProperties => {
                if let Some(path) = self.current_path() {
                    let image = self.current_image();
                    let image = image.as_deref().and_then(|r| r.as_ref().ok());
                    self.properties = Some(Properties::read(&path, image));
                }
            }
            #[cfg(windows)]
            Action::FileProperties => {
                if let Some(path) = self.current_path()
                    && let Err(e) = actions::show_file_properties(&path)
                {
                    self.notify(ctx, Err(e));
                }
            }
            Action::Rename => {
                if let Some(path) = self.current_path() {
                    let name = file_name(&path);
                    self.dialog = Some(Dialog::Rename {
                        path,
                        name,
                        error: None,
                        select: true,
                    });
                }
            }
            #[cfg(windows)]
            Action::Delete => {
                if let Some(path) = self.current_path() {
                    self.dialog = Some(Dialog::Delete(path));
                }
            }
            Action::Options => self.dialog = Some(Dialog::Options(self.config.display.language)),
            Action::About => self.dialog = Some(Dialog::About),
            Action::Escape => {
                if self.menu.is_some() {
                    self.menu = None;
                } else if self.properties.is_some() {
                    self.properties = None;
                } else if self.fullscreen {
                    self.set_fullscreen(ctx, false);
                } else {
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
            Action::Quit => ctx.send_viewport_cmd(ViewportCommand::Close),
        }
    }

    fn navigate(&mut self, f: impl FnOnce(&mut Folder)) {
        if let Some(folder) = &mut self.folder {
            f(folder);
        }
    }

    fn notify(&mut self, ctx: &egui::Context, result: Result<String, String>) {
        let text = match result {
            Ok(msg) => msg,
            Err(e) => tr!("Erreur : {e}", "Error: {e}"),
        };
        self.toast = Some((text, ctx.input(|i| i.time)));
    }

    fn request_decode(&mut self, ctx: &egui::Context, path: &Path) {
        // Chaque décodage HEIC occupe déjà tous les cœurs : au-delà, on attend qu'un se termine
        // (les décodages annulés s'arrêtent vite).
        const MAX_JOBS: usize = 3;
        if self.cache.contains_key(path) || self.pending.contains_key(path) || self.pending.len() >= MAX_JOBS {
            return;
        }
        let cancel = Arc::new(Cancel::default());
        self.pending.insert(path.to_path_buf(), cancel.clone());
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let path = path.to_path_buf();
        let max_side = ctx.input(|i| i.max_texture_side) as u32;
        // Une nouvelle image s'affiche ajustée à la vue actuelle : sa version Lanczos peut être
        // calculée tout de suite (si la vue change d'ici là, elle ne servira simplement pas).
        let view = self.view_rect.is_positive().then(|| self.view_rect.size());
        let pixels_per_point = ctx.pixels_per_point();
        std::thread::spawn(move || {
            let result = decode_file(&path, max_side, &cancel);
            let prepared = match (&result, view) {
                (Ok(img), Some(view)) if !cancel.is_cancelled() => prepare_fit(img, view, pixels_per_point),
                _ => None,
            };
            let _ = tx.send((path, Arc::new(result), prepared));
            ctx.request_repaint();
        });
    }

    /// Réceptionne les décodages terminés, affiche l'image courante dès qu'elle
    /// est prête et précharge ses voisines.
    fn sync_images(&mut self, ctx: &egui::Context) {
        while let Ok((path, result, prepared)) = self.rx.try_recv() {
            if self.pending.remove(&path).is_some_and(|c| !c.is_cancelled()) {
                if let Some(prepared) = prepared {
                    self.prepared.insert(path.clone(), prepared);
                }
                self.cache.insert(path, result);
            }
        }

        let Some(folder) = &self.folder else { return };
        let current = folder.current().to_path_buf();
        let n = folder.len();
        let at = |offset: isize| folder.files[(folder.index as isize + offset).rem_euclid(n as isize) as usize].clone();
        // Voisines préchargées, la plus utile d'abord. Défilement en cours (image demandée ou touche
        // maintenue) : deux images d'avance dans son sens, pour absorber les plus lourdes
        // (distinctes dès 4 images).
        let neighbours: Vec<PathBuf> = match self.nav_direction() {
            Some(dir) if n > 3 => vec![at(dir), at(2 * dir), at(-dir)],
            _ if n > 1 => vec![at(1), at(-1)],
            _ => Vec::new(),
        };

        // Saut (Début / Fin, autre dossier) : les images qui ne sont plus voisines ne sont plus
        // utiles, on abandonne leur décodage.
        for (path, cancel) in &self.pending {
            if *path != current && !neighbours.contains(path) {
                cancel.cancel();
            }
        }
        // On n'occupe le CPU avec les voisines qu'une fois l'image courante prête.
        self.request_decode(ctx, &current);
        if self.cache.contains_key(&current) {
            for p in &neighbours {
                self.request_decode(ctx, p);
            }
        }
        self.cache.retain(|p, _| *p == current || neighbours.contains(p));
        self.prepared.retain(|p, _| *p == current || neighbours.contains(p));

        let already_shown = self.shown.as_ref().is_some_and(|s| s.path == current && !s.outdated);
        if !already_shown && let Some(result) = self.cache.get(&current).cloned() {
            let texture = result.as_ref().as_ref().ok().map(|img| {
                let color =
                    egui::ColorImage::from_rgba_unmultiplied([img.width as usize, img.height as usize], &img.rgba);
                // Au-delà de 100 % : pixels nets (pas de lissage) ; en dessous : réduction
                // lissée avec mipmaps, le temps que la version Lanczos (`sharp`) soit prête.
                let options = TextureOptions {
                    magnification: egui::TextureFilter::Nearest,
                    ..TextureOptions::LINEAR.with_mipmap_mode(Some(egui::TextureFilter::Linear))
                };
                ctx.load_texture(current.to_string_lossy(), color, options)
            });
            // Les propriétés ouvertes suivent l'image affichée.
            if self.properties.is_some() {
                self.properties = Some(Properties::read(&current, result.as_ref().as_ref().ok()));
            }
            if let Some(prepared) = self.prepared.remove(&current) {
                self.sharp.preload(ctx, &current, prepared);
            }
            self.shown = Some(Shown {
                path: current.clone(),
                texture,
                result,
                outdated: false,
            });
            self.reset_view();
            self.quarter_turns = 0;
            self.osd_since = ctx.input(|i| i.time);
            // Le délai d'affichage démarre à l'image suivante, une fois celle-ci à l'écran.
            self.shown_at = None;
            ctx.request_repaint();
            set_title(ctx, Some(&current));
        }
    }

    /// Suit le dossier de l'image affichée et applique ses changements.
    fn sync_folder(&mut self, ctx: &egui::Context) {
        let dir = self.current_dir().map(Path::to_path_buf);
        if let Some(dir) = dir
            && self.watcher.as_ref().is_none_or(|w| w.dir() != dir)
        {
            self.watcher = DirWatcher::new(&dir, ctx);
        }
        let now = ctx.input(|i| i.time);
        if let Some(changes) = self.watcher.as_mut().and_then(|w| w.poll(ctx, now)) {
            self.apply_changes(ctx, &changes);
        }
    }

    /// Changements du dossier, signalés par le système ou faits par la visionneuse (renommage,
    /// suppression).
    fn apply_changes(&mut self, ctx: &egui::Context, changes: &Changes) {
        for path in &changes.modified {
            self.forget(path);
        }
        let Some(folder) = &mut self.folder else {
            // Dossier vidé : une image qui y revient (restaurée de la corbeille…) le rouvre.
            if let Some(dir) = self.watcher.as_ref().map(|w| w.dir().to_path_buf())
                && Folder::open(&dir).is_some()
            {
                self.open_path(&dir);
            }
            return;
        };
        let before = folder.current().to_path_buf();
        if !folder.refresh(&changes.renamed) {
            let dir = before.parent().unwrap_or(&before).display();
            self.message = Some(tr!("Plus aucune image dans {dir}", "No more images in {dir}"));
            self.folder = None;
            self.shown = None;
            self.properties = None;
            set_title(ctx, None);
            return;
        }
        let after = folder.current().to_path_buf();
        if after != before && changes.renamed.iter().any(|(from, _)| *from == before) {
            // Image affichée renommée : elle reste affichée telle quelle, sous son nouveau nom.
            if let Some(result) = self.cache.remove(&before) {
                self.cache.insert(after.clone(), result);
            }
            if let Some(prepared) = self.prepared.remove(&before) {
                self.prepared.insert(after.clone(), prepared);
            }
            if let Some(shown) = self.shown.as_mut().filter(|s| s.path == before) {
                shown.path = after.clone();
            }
            set_title(ctx, Some(&after));
        }
        if self.properties.is_some()
            && let Some(image) = self.current_image()
        {
            self.properties = Some(Properties::read(&after, image.as_ref().as_ref().ok()));
        }
        ctx.request_repaint();
    }

    /// Fichier modifié : son décodage n'est plus valable.
    fn forget(&mut self, path: &Path) {
        self.cache.remove(path);
        self.prepared.remove(path);
        if let Some(cancel) = self.pending.remove(path) {
            cancel.cancel();
        }
        if let Some(shown) = self.shown.as_mut().filter(|s| s.path == path) {
            shown.outdated = true;
        }
    }

    /// Renomme l'image affichée ; l'erreur est montrée dans la fenêtre de renommage.
    fn rename(&mut self, ctx: &egui::Context, path: &Path, name: &str) -> Result<(), String> {
        let name = name.trim();
        let forbidden: &[char] = if cfg!(windows) {
            &['\\', '/', ':', '*', '?', '"', '<', '>', '|']
        } else {
            &['/']
        };
        if name.is_empty() || name == "." || name == ".." || name.contains(forbidden) {
            let chars: String = forbidden.iter().collect();
            return Err(tr!(
                "Nom invalide (il ne peut pas contenir {chars})",
                "Invalid name (it cannot contain {chars})"
            ));
        }
        let new = path.with_file_name(name);
        if new == path {
            return Ok(());
        }
        // Un simple changement de casse désigne le même fichier sous Windows.
        if new.exists() && !same_file(&new, path) {
            return Err(tr!("« {name} » existe déjà.", "“{name}” already exists."));
        }
        std::fs::rename(path, &new).map_err(|e| e.to_string())?;
        let changes = Changes {
            renamed: vec![(path.to_path_buf(), new)],
            modified: Vec::new(),
        };
        self.apply_changes(ctx, &changes);
        Ok(())
    }

    #[cfg(windows)]
    fn delete(&mut self, ctx: &egui::Context, path: &Path) {
        match actions::move_to_recycle_bin(path, self.hwnd) {
            Ok(()) => {
                self.apply_changes(ctx, &Changes::default());
                let name = file_name(path);
                self.notify(
                    ctx,
                    Ok(tr!(
                        "« {name} » mis à la Corbeille",
                        "“{name}” moved to the Recycle Bin"
                    )),
                );
            }
            Err(e) => self.notify(ctx, Err(e)),
        }
    }

    fn is_loading(&self) -> bool {
        match (&self.folder, &self.shown) {
            (Some(f), Some(s)) => s.path != f.current(),
            (Some(_), None) => true,
            _ => false,
        }
    }

    fn handle_keyboard(&mut self, ctx: &egui::Context) {
        let (hovering, now) = ctx.input(|i| (!i.raw.hovered_files.is_empty(), i.time));
        let dragging = self.drag.update(hovering, OsCursor::button_down(), now);
        // Fenêtre modale ouverte : les touches sont les siennes (saisie d'un nom, Entrée, Échap).
        if self.dialog.is_some() {
            self.held = None;
            return;
        }
        let mut actions = Vec::new();
        let mut held = None;
        ctx.input_mut(|i| {
            let mut on = |pressed: bool, action| {
                if pressed {
                    actions.push(action);
                }
            };
            let m = i.modifiers;
            let plain = !m.command && !m.alt;
            on(i.consume_key(egui::Modifiers::COMMAND, Key::O), Action::Open);
            on(i.consume_key(egui::Modifiers::ALT, Key::Enter), Action::ImageProperties);
            // Navigation et rotation : un appui agit tout de suite ; si la touche reste enfoncée,
            // c'est `HeldKey` qui prend le relais à la place de la répétition du clavier.
            // Image zoomée : les flèches déplacent la vue ; sinon ←/→ changent d'image.
            let arrows = !self.overflows;
            let repeatable = [
                (Key::ArrowLeft, arrows, Action::Previous),
                (Key::PageUp, true, Action::Previous),
                (Key::Backspace, true, Action::Previous),
                (Key::ArrowRight, arrows, Action::Next),
                (Key::PageDown, true, Action::Next),
                (Key::Space, true, Action::Next),
                (Key::R, plain && !m.shift, Action::RotateRight),
                (Key::R, plain && m.shift, Action::RotateLeft),
                (Key::L, plain && !m.shift, Action::RotateLeft),
                (Key::L, plain && m.shift, Action::RotateRight),
            ];
            for (key, enabled, action) in repeatable {
                if enabled && first_press(i, key) {
                    on(true, action);
                    held = Some(HeldKey {
                        key,
                        action,
                        repeating: false,
                        last: i.time,
                    });
                }
            }
            if let Some(h) = &mut self.held {
                h.repeating |= i
                    .events
                    .iter()
                    .any(|e| matches!(e, egui::Event::Key { key, pressed: true, repeat: true, .. } if *key == h.key));
            }
            on(i.key_pressed(Key::Home), Action::First);
            on(i.key_pressed(Key::End), Action::Last);
            on(
                i.key_pressed(Key::F11) || plain && (i.key_pressed(Key::F) || i.key_pressed(Key::Enter)),
                Action::ToggleFullscreen,
            );
            on(!dragging && i.key_pressed(Key::Escape), Action::Escape);
            on(plain && i.key_pressed(Key::Q), Action::Quit);
            on(plain && i.key_pressed(Key::W), Action::RevealInFolder);
            on(plain && i.key_pressed(Key::F2), Action::Rename);
            #[cfg(windows)]
            on(plain && i.key_pressed(Key::Delete), Action::Delete);
            on(plain && i.key_pressed(Key::Z), Action::Fit);
            // Chiffres lus sur la touche physique : sur un clavier AZERTY, la rangée du haut
            // donne « & é " ' ( - … » sans Maj (et « - » ne doit pas dézoomer).
            for event in &i.events {
                let egui::Event::Key {
                    key,
                    physical_key,
                    pressed: true,
                    repeat,
                    modifiers,
                    ..
                } = event
                else {
                    continue;
                };
                let digit = physical_key.and_then(|k| NUM_KEYS.iter().position(|n| *n == k));
                match digit {
                    Some(_) if modifiers.command || modifiers.alt || *repeat => {}
                    Some(0) => actions.push(Action::Fit),
                    Some(d) => actions.push(Action::ZoomPreset(d as u8)),
                    None => match key {
                        Key::Plus | Key::Equals => actions.push(Action::ZoomBy(1.25)),
                        Key::Minus => actions.push(Action::ZoomBy(0.8)),
                        _ => {}
                    },
                }
            }
        });
        for action in actions {
            self.run(ctx, action);
        }
        if held.is_some() {
            self.held = held;
        }
        self.repeat_held_key(ctx);
        if self.overflows {
            self.pan_with_arrows(ctx);
        }
    }

    /// Touche maintenue (après le délai de répétition du clavier) : pour la navigation, l'image
    /// suivante est demandée dès que la précédente est servie, `advance` en réglant le rythme ;
    /// pour la rotation, un quart de tour toutes les `PACE`.
    fn repeat_held_key(&mut self, ctx: &egui::Context) {
        let Some(mut held) = self.held.take() else { return };
        let (down, now) = ctx.input(|i| (i.key_down(held.key), i.time));
        if !down {
            return;
        }
        if held.repeating {
            match held.action {
                Action::Previous | Action::Next if self.step == 0 => self.run(ctx, held.action),
                Action::Previous | Action::Next => {}
                _ => {
                    let wait = held.last + PACE - now;
                    if wait <= 0.0 {
                        self.run(ctx, held.action);
                        held.last = now;
                        ctx.request_repaint_after_secs(PACE as f32);
                    } else {
                        ctx.request_repaint_after_secs(wait as f32);
                    }
                }
            }
        }
        self.held = Some(held);
    }

    /// Navigation demandée : on passe à l'image suivante (ou précédente) une fois la courante
    /// affichée depuis au moins `PACE`. Aucune image n'est sautée, même avec des appuis
    /// rapides, et une image lourde reste visible aussi longtemps qu'une légère.
    fn advance(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        let shown_at = *self.shown_at.get_or_insert(now);
        if self.step == 0 || self.is_loading() {
            // En chargement : le décodage terminé redemande un affichage.
            return;
        }
        let wait = shown_at + PACE - now;
        if wait > 0.0 {
            ctx.request_repaint_after_secs(wait as f32);
            return;
        }
        let dir = std::mem::take(&mut self.step);
        self.navigate(|f| f.step(dir));
    }

    /// Sens du défilement en cours (image demandée ou touche maintenue), pour précharger plus
    /// loin.
    fn nav_direction(&self) -> Option<isize> {
        if self.step != 0 {
            return Some(self.step);
        }
        match self.held.as_ref().filter(|h| h.repeating)?.action {
            Action::Next => Some(1),
            Action::Previous => Some(-1),
            _ => None,
        }
    }

    /// Déplacement continu tant que les flèches sont maintenues.
    fn pan_with_arrows(&mut self, ctx: &egui::Context) {
        const SPEED: f32 = 1200.0; // points par seconde
        let (dir, dt) = ctx.input(|i| {
            let axis = |neg: Key, pos: Key| i.key_down(pos) as i32 as f32 - i.key_down(neg) as i32 as f32;
            (
                vec2(
                    axis(Key::ArrowLeft, Key::ArrowRight),
                    axis(Key::ArrowUp, Key::ArrowDown),
                ),
                i.stable_dt.min(0.05),
            )
        });
        if dir != Vec2::ZERO {
            // → montre ce qui est à droite : l'image part vers la gauche.
            self.pan -= dir * SPEED * dt;
            ctx.request_repaint();
        }
    }

    /// Molette (clic) = plein écran, boutons latéraux = précédente / suivante.
    fn handle_mouse_buttons(&mut self, ctx: &egui::Context) {
        if self.dialog.is_some() {
            return;
        }
        let (middle, back, forward) = ctx.input(|i| {
            (
                i.pointer.button_clicked(PointerButton::Middle),
                i.pointer.button_pressed(PointerButton::Extra1),
                i.pointer.button_pressed(PointerButton::Extra2),
            )
        });
        if middle {
            self.run(ctx, Action::ToggleFullscreen);
        }
        if back {
            self.run(ctx, Action::Previous);
        }
        if forward {
            self.run(ctx, Action::Next);
        }
    }

    /// En sortant du plein écran, winit remet la fenêtre à sa position « normale » : pour une
    /// fenêtre ancrée (Windows Snap), celle d'avant l'ancrage. On la remet plutôt exactement où
    /// elle était (dans sa zone d'ancrage), sauf si elle était agrandie (winit la restaure alors
    /// bien).
    fn set_fullscreen(&mut self, ctx: &egui::Context, on: bool) {
        if on == self.fullscreen {
            return;
        }
        if on {
            self.windowed = ctx.input(|i| {
                let viewport = i.viewport();
                if viewport.maximized == Some(true) {
                    return None;
                }
                Some((viewport.outer_rect?.min, viewport.inner_rect?.size()))
            });
        }
        self.fullscreen = on;
        ctx.send_viewport_cmd(ViewportCommand::Fullscreen(on));
        if !on && let Some((position, size)) = self.windowed.take() {
            ctx.send_viewport_cmd(ViewportCommand::OuterPosition(position));
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(size));
        }
    }

    fn rotate(&mut self, quarter_turns: u8) {
        self.quarter_turns = (self.quarter_turns + quarter_turns) % 4;
        self.reset_view();
    }

    fn reset_view(&mut self) {
        self.zoom = None;
        self.pan = Vec2::ZERO;
        self.zoom_preset = None;
    }

    /// Change l'échelle en gardant fixe le point `anchor` de l'écran (défaut : centre de la vue).
    fn zoom_around(&mut self, new_scale: f32, anchor: Option<Pos2>) {
        let rect = self.view_rect;
        let old_scale = self.zoom.unwrap_or(self.last_fit);
        let new_scale = new_scale.clamp(0.01, 64.0);
        if rect.is_positive() && old_scale > 0.0 {
            let anchor = anchor.unwrap_or(rect.center());
            let center = rect.center() + self.pan;
            let new_center = anchor - (anchor - center) * (new_scale / old_scale);
            self.pan = new_center - rect.center();
        }
        self.zoom = Some(new_scale);
    }

    /// Touches 1 à 9 : zoom à N × la taille réelle ; la même touche une 2ᵉ fois
    /// revient à la vue d'avant.
    fn toggle_zoom_preset(&mut self, level: u8) {
        if let Some(preset) = self.zoom_preset.take_if(|p| p.level == level) {
            self.zoom = preset.previous_zoom;
            self.pan = preset.previous_pan;
            return;
        }
        let (previous_zoom, previous_pan) = match self.zoom_preset.take() {
            Some(p) => (p.previous_zoom, p.previous_pan),
            None => (self.zoom, self.pan),
        };
        self.zoom_around(self.one_to_one_scale() * level as f32, None);
        self.zoom_preset = Some(ZoomPreset {
            level,
            previous_zoom,
            previous_pan,
        });
    }

    /// Échelle (écran / texture) correspondant à 1 pixel photo = 1 pixel écran.
    fn one_to_one_scale(&self) -> f32 {
        self.shown
            .as_ref()
            .and_then(|s| s.result.as_ref().as_ref().ok())
            .map(|img| img.original_size[0] as f32 / img.width as f32)
            .unwrap_or(1.0)
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) -> Option<Action> {
        let mut action = None;
        ui.horizontal(|ui| {
            let mut button = |ui: &mut egui::Ui, enabled: bool, text: String, hint: String, a: Action| {
                if ui.add_enabled(enabled, egui::Button::new(text)).on_hover_text(hint).clicked() {
                    action = Some(a);
                }
            };
            button(ui, true, tr!("📂 Ouvrir…", "📂 Open…"), tr!("Ouvrir une image (Ctrl+O)", "Open an image (Ctrl+O)"), Action::Open);
            ui.separator();
            let has_folder = self.folder.as_ref().is_some_and(|f| f.len() > 1);
            button(ui, has_folder, tr!("◀ Précédente", "◀ Previous"), tr!("Image précédente\nFlèche gauche, clic sur la moitié gauche de l'image, bouton « précédent » de la souris", "Previous image\nLeft arrow, click on the left half of the image, mouse “back” button"), Action::Previous);
            button(ui, has_folder, tr!("Suivante ▶", "Next ▶"), tr!("Image suivante\nFlèche droite, clic sur la moitié droite de l'image, bouton « suivant » de la souris", "Next image\nRight arrow, click on the right half of the image, mouse “forward” button"), Action::Next);
            ui.separator();
            let has_image = self.shown.as_ref().is_some_and(|s| s.texture.is_some());
            button(ui, has_image, "⟲".into(), tr!("Pivoter à gauche (L, ou Maj+R)", "Rotate left (L, or Shift+R)"), Action::RotateLeft);
            button(ui, has_image, "⟳".into(), tr!("Pivoter à droite (R, ou Maj+L)", "Rotate right (R, or Shift+L)"), Action::RotateRight);
            button(ui, has_image, tr!("Ajuster", "Fit"), tr!("Ajuster à la fenêtre (0)", "Fit to window (0)"), Action::Fit);
            button(ui, has_image, "100 %".into(), tr!("Taille réelle (1, de nouveau 1 pour revenir)\n2 à 9 : zoom ×2 à ×9", "Actual size (1, 1 again to go back)\n2 to 9: zoom ×2 to ×9"), Action::ZoomPreset(1));
            ui.separator();
            button(ui, true, tr!("⛶ Plein écran", "⛶ Full screen"), tr!("F11 ou clic molette (Échap pour sortir)", "F11 or middle click (Esc to exit)"), Action::ToggleFullscreen);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                button(ui, true, "ℹ".into(), tr!("À propos", "About"), Action::About);
                button(ui, true, "⚙".into(), tr!("Options (langue)", "Preferences (language)"), Action::Options);
            });
        });
        action
    }

    fn status_bar(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if let Some(folder) = &self.folder {
                ui.label(format!("{} / {}", folder.index + 1, folder.len()));
                ui.separator();
                ui.label(file_name(folder.current()));
            }
            if let Some(result) = self.current_image()
                && let Ok(img) = result.as_ref()
            {
                ui.separator();
                ui.label(format!("{} × {}", img.original_size[0], img.original_size[1]));
                if let Some(pct) = self.zoom_percent(img) {
                    ui.separator();
                    ui.label(format!("{pct:.0} %"));
                }
                ui.separator();
                let ms = img.elapsed.as_millis();
                ui.label(tr!("décodé en {ms} ms", "decoded in {ms} ms"));
            }
            if self.is_loading() {
                ui.separator();
                ui.spinner();
                ui.label(tr!("Chargement…", "Loading…"));
            }
        });
    }

    /// Zoom affiché, par rapport à la taille réelle de la photo.
    fn zoom_percent(&self, img: &DecodedImage) -> Option<f32> {
        self.last_scale
            .map(|scale| scale * img.width as f32 / img.original_size[0] as f32 * 100.0)
    }

    /// Plein écran (sans barre d'état) : rappel de la position, du nom et du zoom après un
    /// changement d'image ou de zoom, et pendant un chargement.
    fn fullscreen_indicator(&self, ctx: &egui::Context) {
        const VISIBLE: f64 = 1.5;
        const FADE: f64 = 0.3;
        if !self.fullscreen {
            return;
        }
        let Some(folder) = &self.folder else { return };
        let elapsed = ctx.input(|i| i.time) - self.osd_since;
        let loading = self.is_loading();
        if !loading && elapsed > VISIBLE {
            return;
        }
        let opacity = if loading {
            1.0
        } else {
            ((VISIBLE - elapsed) / FADE).min(1.0) as f32
        };
        let mut text = format!(
            "{} / {}  ·  {}",
            folder.index + 1,
            folder.len(),
            file_name(folder.current())
        );
        if let Some(Ok(img)) = self.current_image().as_deref()
            && let Some(pct) = self.zoom_percent(img)
        {
            text.push_str(&format!("  ·  {pct:.0} %"));
        }
        egui::Area::new(egui::Id::new("fullscreen_indicator"))
            .order(egui::Order::Tooltip)
            .anchor(egui::Align2::CENTER_TOP, vec2(0.0, 16.0))
            .interactable(false)
            .show(ctx, |ui| {
                ui.set_opacity(opacity);
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        // Sablier à la hauteur du texte : le bandeau garde la même hauteur, et
                        // la même que les autres messages, pendant un chargement.
                        if loading {
                            ui.add(egui::Spinner::new().size(ui.text_style_height(&egui::TextStyle::Body)));
                        }
                        ui.add(egui::Label::new(text).extend());
                    });
                });
            });
        ctx.request_repaint();
    }

    /// Dessine l'image et gère zoom/déplacement/clics. Retourne l'échelle effective.
    fn image_view(&mut self, ui: &mut egui::Ui) -> Option<f32> {
        let rect = ui.max_rect();
        self.view_rect = rect;
        let response = ui.allocate_rect(rect, Sense::click_and_drag());
        let ctx = ui.ctx().clone();

        // Clic droit : menu contextuel.
        if response.secondary_clicked() {
            self.menu = response.interact_pointer_pos();
        }
        // Clic gauche (sans glisser) : moitié gauche = précédente, moitié droite = suivante.
        // (un clic qui ferme le menu ne fait rien d'autre)
        if response.clicked()
            && !(self.menu.take().is_some() | std::mem::take(&mut self.swallow_click))
            && let Some(p) = response.interact_pointer_pos()
        {
            if self.shown.is_none() {
                self.run(&ctx, Action::Open);
            } else {
                let action = if p.x < rect.center().x {
                    Action::Previous
                } else {
                    Action::Next
                };
                self.run(&ctx, action);
            }
        }

        let Some(shown) = &self.shown else {
            let text = self.message.clone().unwrap_or_else(|| {
                tr!(
                    "Glissez-déposez une photo ici\nou cliquez sur « Ouvrir… » (Ctrl+O)",
                    "Drag and drop a photo here\nor click “Open…” (Ctrl+O)"
                )
            });
            centered_text(ui, rect, &text, Color32::GRAY);
            return None;
        };
        let texture = match (&shown.texture, shown.result.as_ref()) {
            (Some(t), _) => t.clone(),
            (None, Err(e)) => {
                let name = file_name(&shown.path);
                let text = tr!("Impossible d'afficher {name}\n\n{e}", "Cannot display {name}\n\n{e}");
                centered_text(ui, rect, &text, Color32::LIGHT_RED);
                return None;
            }
            (None, Ok(_)) => return None,
        };
        let (path, decoded) = (shown.path.clone(), shown.result.clone());

        let tex_size = texture.size_vec2();
        let rotated = if self.quarter_turns % 2 == 1 {
            vec2(tex_size.y, tex_size.x)
        } else {
            tex_size
        };
        // Même calcul que la version préparée au décodage (`prepare_fit`).
        let fit = fit_scale(rect.size(), rotated, self.one_to_one_scale());
        self.last_fit = fit;

        // Molette = zoom centré sur le curseur (Ctrl+molette / pincement aussi).
        if response.hover_pos().is_some() {
            let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
            let factor = (scroll * 0.002).exp() * pinch;
            if (factor - 1.0).abs() > 1e-4 {
                let scale = self.zoom.unwrap_or(fit);
                self.zoom_around((scale * factor).max(fit.min(0.01)), response.hover_pos());
                self.zoom_preset = None;
            }
        }
        let scale = self.zoom.unwrap_or(fit);
        if response.dragged_by(PointerButton::Primary) {
            self.pan += response.drag_delta();
            if self.zoom.is_none() {
                self.zoom = Some(scale);
            }
        }
        if self.zoom.is_none() {
            self.pan = Vec2::ZERO;
        }

        let size = rotated * scale;
        self.pan = clamp_pan(self.pan, size, rect.size());
        let center = rect.center() + self.pan;

        // Sous 100 % (moins d'un pixel écran par pixel de texture) : version Lanczos dès qu'elle
        // est prête, affichée pixel pour pixel. Les deux versions occupent exactement le même
        // cadre, en pixels entiers et calé sur la grille de l'écran : à la bascule, seule la
        // netteté change.
        let ppp = ctx.pixels_per_point();
        let sharp_size = lanczos_size(tex_size, scale, ppp);
        let sharp = self.sharp.update(&ctx, &path, &decoded, sharp_size);
        let display_size = match sharp_size {
            Some([w, h]) if self.quarter_turns % 2 == 1 => vec2(h as f32, w as f32) / ppp,
            Some([w, h]) => vec2(w as f32, h as f32) / ppp,
            None => size,
        };
        let img_rect = Rect::from_min_size(((center - display_size / 2.0) * ppp).round() / ppp, display_size);
        let texture_id = sharp.unwrap_or_else(|| texture.id());
        ui.painter_at(rect)
            .add(rotated_mesh(texture_id, img_rect, self.quarter_turns));

        self.overflows = size.x > rect.width() + 0.5 || size.y > rect.height() + 0.5;
        let can_navigate = self.folder.as_ref().is_some_and(|f| f.len() > 1);
        if response.dragged() {
            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        } else if can_navigate
            && self.menu.is_none()
            && let Some(p) = response.hover_pos()
        {
            // Curseur flèche indiquant l'image (précédente / suivante) qu'ouvrira un clic.
            ctx.set_cursor_icon(egui::CursorIcon::None);
            let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, "nav_cursor".into()));
            paint_arrow_cursor(&painter, p, p.x >= rect.center().x);
        } else if self.overflows {
            response.on_hover_cursor(egui::CursorIcon::Grab);
        }
        Some(scale)
    }

    fn context_menu(&mut self, ctx: &egui::Context) {
        let Some(pos) = self.menu else { return };
        let has_image = matches!(self.current_image().as_deref(), Some(Ok(_)));
        let has_path = self.folder.is_some();
        let mut action = None;

        let area = egui::Area::new(egui::Id::new("context_menu"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .constrain(true)
            .show(ctx, |ui| {
                egui::Frame::menu(ui.style()).show(ui, |ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                    ui.set_min_width(220.0);
                    let mut item = |ui: &mut egui::Ui, enabled: bool, text: String, shortcut: String, a: Action| {
                        let button = egui::Button::new(text).shortcut_text(shortcut).frame(false);
                        if ui.add_enabled(enabled, button).clicked() {
                            action = Some(a);
                        }
                    };
                    let none = String::new;
                    item(
                        ui,
                        has_image,
                        tr!("Copier l'image", "Copy image"),
                        "Ctrl+C".into(),
                        Action::CopyImage,
                    );
                    item(
                        ui,
                        has_path,
                        tr!("Copier le chemin", "Copy path"),
                        none(),
                        Action::CopyPath,
                    );
                    ui.separator();
                    item(
                        ui,
                        has_path,
                        tr!("Afficher dans le dossier", "Show in folder"),
                        "W".into(),
                        Action::RevealInFolder,
                    );
                    item(ui, true, tr!("Ouvrir…", "Open…"), "Ctrl+O".into(), Action::Open);
                    item(ui, has_path, tr!("Renommer…", "Rename…"), "F2".into(), Action::Rename);
                    #[cfg(windows)]
                    item(
                        ui,
                        has_path,
                        tr!("Supprimer…", "Delete…"),
                        tr!("Suppr", "Del"),
                        Action::Delete,
                    );
                    ui.separator();
                    item(
                        ui,
                        has_image,
                        tr!("Pivoter à gauche", "Rotate left"),
                        "L".into(),
                        Action::RotateLeft,
                    );
                    item(
                        ui,
                        has_image,
                        tr!("Pivoter à droite", "Rotate right"),
                        "R".into(),
                        Action::RotateRight,
                    );
                    let fs = if self.fullscreen {
                        tr!("Quitter le plein écran", "Exit full screen")
                    } else {
                        tr!("Plein écran", "Full screen")
                    };
                    item(ui, true, fs, "F11".into(), Action::ToggleFullscreen);
                    ui.separator();
                    item(
                        ui,
                        has_path,
                        tr!("Propriétés de l'image", "Image properties"),
                        tr!("Alt+Entrée", "Alt+Enter"),
                        Action::ImageProperties,
                    );
                    #[cfg(windows)]
                    item(
                        ui,
                        has_path,
                        tr!("Propriétés du fichier", "File properties"),
                        none(),
                        Action::FileProperties,
                    );
                    ui.separator();
                    item(ui, true, tr!("Options…", "Preferences…"), none(), Action::Options);
                    item(ui, true, tr!("À propos…", "About…"), none(), Action::About);
                });
            })
            .response;

        if let Some(a) = action {
            self.menu = None;
            self.run(ctx, a);
        } else if ctx.input(|i| i.pointer.any_pressed())
            && !ctx.pointer_interact_pos().is_some_and(|p| area.rect.contains(p))
        {
            // Clic ailleurs : ferme le menu ; un clic gauche ne doit pas en plus changer d'image.
            self.menu = None;
            self.swallow_click = ctx.input(|i| i.pointer.primary_pressed());
        }
    }

    fn properties_window(&mut self, ctx: &egui::Context) {
        let Some(props) = &self.properties else { return };
        let mut open = true;
        let mut action = None;
        egui::Window::new(tr!("Propriétés de l'image", "Image properties"))
            .id(egui::Id::new("properties"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(460.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(ctx.content_rect().height() * 0.7)
                    .show(ui, |ui| {
                        egui::Grid::new("props")
                            .num_columns(2)
                            .striped(true)
                            .spacing([16.0, 6.0])
                            .show(ui, |ui| {
                                for (k, v) in &props.summary {
                                    ui.strong(k);
                                    ui.add(egui::Label::new(v).selectable(true).wrap());
                                    ui.end_row();
                                }
                            });
                        if props.all_exif.is_empty() {
                            ui.add_space(6.0);
                            ui.weak(tr!("Aucune donnée EXIF.", "No EXIF data."));
                        } else {
                            ui.add_space(6.0);
                            let count = props.all_exif.len();
                            egui::CollapsingHeader::new(tr!(
                                "Toutes les données EXIF ({count})",
                                "All EXIF data ({count})"
                            ))
                            .id_salt("all_exif")
                            .show(ui, |ui| {
                                egui::Grid::new("exif").num_columns(2).striped(true).show(ui, |ui| {
                                    for (k, v) in &props.all_exif {
                                        ui.label(k);
                                        ui.add(egui::Label::new(v).selectable(true).wrap());
                                        ui.end_row();
                                    }
                                });
                            });
                        }
                    });
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button(tr!("Afficher dans le dossier", "Show in folder")).clicked() {
                        action = Some(Action::RevealInFolder);
                    }
                    if ui.button(tr!("Copier le chemin", "Copy path")).clicked() {
                        action = Some(Action::CopyPath);
                    }
                    if let Some((lat, lon)) = props.gps
                        && ui
                            .button(tr!("Voir sur la carte", "Show on map"))
                            .on_hover_text(tr!(
                                "OpenStreetMap, à l'endroit de la prise de vue",
                                "OpenStreetMap, where the photo was taken"
                            ))
                            .clicked()
                    {
                        action = Some(Action::ShowOnMap { lat, lon });
                    }
                    #[cfg(windows)]
                    if ui.button(tr!("Propriétés du fichier", "File properties")).clicked() {
                        action = Some(Action::FileProperties);
                    }
                });
            });
        if !open {
            self.properties = None;
        }
        if let Some(a) = action {
            self.run(ctx, a);
        }
    }

    fn show_toast(&mut self, ctx: &egui::Context) {
        const DURATION: f64 = 2.5;
        let Some((text, since)) = &self.toast else { return };
        let now = ctx.input(|i| i.time);
        if now - since > DURATION {
            self.toast = None;
            return;
        }
        let screen = ctx.content_rect();
        egui::Area::new(egui::Id::new("toast"))
            .order(egui::Order::Tooltip)
            .anchor(egui::Align2::CENTER_BOTTOM, vec2(0.0, -40.0))
            .interactable(false)
            .constrain_to(screen)
            .show(ctx, |ui| {
                // Sans `extend`, la largeur mémorisée de la zone fait passer le texte à la ligne.
                egui::Frame::popup(ui.style()).show(ui, |ui| ui.add(egui::Label::new(text.as_str()).extend()));
            });
        ctx.request_repaint_after_secs((since + DURATION - now) as f32);
    }

    /// Ctrl+C arrive sous forme d'`Event::Copy` (pas de touche) : on copie l'image, sauf si
    /// du texte est sélectionné dans les propriétés. Ce texte, egui le copie lui-même en fin
    /// de passe (un simple clic sur une valeur la sélectionne : elle est copiée en entier).
    fn handle_copy(&mut self, ctx: &egui::Context) {
        if !ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy))) {
            return;
        }
        // Fenêtre modale : la copie concerne sa zone de saisie.
        if self.dialog.is_some() {
            return;
        }
        if text_is_selected(ctx) {
            self.notify(
                ctx,
                Ok(tr!(
                    "Texte copié dans le presse-papiers",
                    "Text copied to the clipboard"
                )),
            );
        } else {
            self.run(ctx, Action::CopyImage);
        }
    }

    /// Fenêtre modale ouverte : `Entrée` la valide, `Échap` ou un clic à côté la ferme.
    fn dialogs(&mut self, ctx: &egui::Context) {
        enum Outcome {
            Keep,
            Close,
            Rename(PathBuf, String),
            #[cfg(windows)]
            Delete(PathBuf),
            Options(Language),
        }
        let Some(dialog) = &mut self.dialog else { return };
        let enter = ctx.input(|i| i.key_pressed(Key::Enter));
        let mut outcome = Outcome::Keep;
        let id = egui::Id::new(("dialog", std::mem::discriminant(dialog)));
        let modal = egui::Modal::new(id).show(ctx, |ui| {
            ui.set_max_width(440.0);
            let buttons = |ui: &mut egui::Ui, ok: String| {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let ok = ui.button(ok).clicked();
                    let cancel = ui.button(tr!("Annuler", "Cancel")).clicked();
                    (ok || enter, cancel)
                })
                .inner
            };
            match dialog {
                Dialog::Rename {
                    path,
                    name,
                    error,
                    select,
                } => {
                    ui.heading(tr!("Renommer", "Rename"));
                    let mut edit = egui::TextEdit::singleline(name).desired_width(420.0).show(ui);
                    if std::mem::take(select) {
                        let stem = Path::new(name.as_str())
                            .file_stem()
                            .map_or(0, |s| s.to_string_lossy().chars().count());
                        let range =
                            egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(stem));
                        edit.state.cursor.set_char_range(Some(range));
                        edit.state.store(ui.ctx(), edit.response.id);
                        edit.response.request_focus();
                    }
                    if let Some(error) = error {
                        ui.colored_label(Color32::LIGHT_RED, error.as_str());
                    }
                    match buttons(ui, tr!("Renommer", "Rename")) {
                        (true, _) => outcome = Outcome::Rename(path.clone(), name.clone()),
                        (_, true) => outcome = Outcome::Close,
                        _ => {}
                    }
                }
                #[cfg(windows)]
                Dialog::Delete(path) => {
                    let name = file_name(path);
                    ui.heading(tr!("Supprimer", "Delete"));
                    ui.label(tr!(
                        "Mettre « {name} » à la Corbeille ?",
                        "Move “{name}” to the Recycle Bin?"
                    ));
                    match buttons(ui, tr!("Supprimer", "Delete")) {
                        (true, _) => outcome = Outcome::Delete(path.clone()),
                        (_, true) => outcome = Outcome::Close,
                        _ => {}
                    }
                }
                Dialog::Options(language) => {
                    ui.heading(tr!("Options", "Preferences"));
                    ui.horizontal(|ui| {
                        ui.label(tr!("Langue", "Language"));
                        egui::ComboBox::from_id_salt("language")
                            .selected_text(language_label(*language))
                            .show_ui(ui, |ui| {
                                for l in [Language::Auto, Language::English, Language::French] {
                                    ui.selectable_value(language, l, language_label(l));
                                }
                            });
                    });
                    match buttons(ui, "OK".into()) {
                        (true, _) => outcome = Outcome::Options(*language),
                        (_, true) => outcome = Outcome::Close,
                        _ => {}
                    }
                }
                Dialog::About => {
                    let version = env!("CARGO_PKG_VERSION");
                    ui.label(egui::RichText::new(format!("{APP_NAME} {version}")).strong());
                    ui.label(tr!(
                        "Visionneuse de photos HEIC/HEIF, sans extension payante, décodées en Rust pur.",
                        "HEIC/HEIF photo viewer, with no paid extension, decoded in pure Rust."
                    ));
                    ui.label(tr!("Licence : AGPL 3.0.", "License: AGPL 3.0."));
                    ui.add_space(8.0);
                    if ui.button("OK").clicked() || enter {
                        outcome = Outcome::Close;
                    }
                }
            }
        });
        if modal.should_close() && matches!(outcome, Outcome::Keep) {
            outcome = Outcome::Close;
        }
        match outcome {
            Outcome::Keep => {}
            Outcome::Close => self.dialog = None,
            Outcome::Rename(path, name) => match self.rename(ctx, &path, &name) {
                Ok(()) => self.dialog = None,
                Err(e) => {
                    if let Some(Dialog::Rename { error, .. }) = &mut self.dialog {
                        *error = Some(e);
                    }
                }
            },
            #[cfg(windows)]
            Outcome::Delete(path) => {
                self.dialog = None;
                self.delete(ctx, &path);
            }
            Outcome::Options(language) => {
                self.dialog = None;
                self.config.display.language = language;
                crate::i18n::set(language);
                if let Err(e) = self.config.save() {
                    self.notify(ctx, Err(e));
                }
            }
        }
    }

    fn handle_drop(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.first().map(|f| f.path().to_path_buf()));
        if let Some(path) = dropped {
            self.open_path(&path);
            ctx.send_viewport_cmd(ViewportCommand::Focus);
        }
    }
}

impl eframe::App for ViewerApp {
    /// Retour par Alt+Tab, changement de bureau virtuel, plein écran sous un curseur immobile… :
    /// la position du curseur n'est connue qu'au premier mouvement de la souris. Sans elle, clics
    /// et molette sont sans effet et le curseur flèche n'est pas dessiné. On l'annonce donc au
    /// retour du focus, quand le pointeur est déclaré parti alors qu'il est toujours au-dessus
    /// de la fenêtre (les deux peuvent arriver dans n'importe quel ordre), au premier coup de
    /// molette, et tant qu'elle reste inconnue alors que la fenêtre a le focus (voir
    /// `CURSOR_POLL`) : juste après un changement de bureau, par exemple, la fenêtre n'est pas
    /// encore considérée comme sous le curseur. Jamais pendant ni juste après un glisser-déposer
    /// (`drag_cursor`).
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        if !raw_input.hovered_files.is_empty() {
            self.drag_cursor = true;
        } else if raw_input
            .events
            .iter()
            .any(|e| matches!(e, egui::Event::PointerMoved(_) | egui::Event::WindowFocused(true)))
        {
            self.drag_cursor = false;
        }
        let position_unknown = ctx.input(|i| i.pointer.latest_pos().is_none());
        let needs_pointer = (raw_input.focused && position_unknown)
            || raw_input.events.iter().any(|e| match e {
                egui::Event::WindowFocused(true) | egui::Event::PointerGone => true,
                egui::Event::MouseWheel { .. } => position_unknown,
                _ => false,
            });
        if needs_pointer
            && !self.drag_cursor
            && let Some(pos) = self.cursor.announce(ctx.pixels_per_point())
        {
            // En fin de lot, après un éventuel « pointeur parti » ; l'annonce à la fenêtre
            // vaut pour les clics qui suivent.
            raw_input.events.push(egui::Event::PointerMoved(pos));
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        #[cfg(windows)]
        if let Some(relaunch) = &self.relaunch {
            relaunch.clear();
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_drop(&ctx);
        self.sync_folder(&ctx);
        self.handle_keyboard(&ctx);
        self.handle_mouse_buttons(&ctx);
        self.advance(&ctx);
        self.sync_images(&ctx);

        if !self.fullscreen {
            let toolbar = egui::Panel::top("toolbar").show(ui, |ui| self.toolbar(ui)).inner;
            if let Some(action) = toolbar {
                self.run(&ctx, action);
            }
            egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        }
        let scale = egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(Color32::from_gray(18)))
            .show(ui, |ui| self.image_view(ui))
            .inner;
        // La barre d'état est dessinée avant l'image : on la rafraîchit si l'échelle a changé.
        if scale != self.last_scale {
            // Le passage d'une image à l'autre est déjà signalé ; ici, seul le zoom compte.
            if self.last_scale.is_some() && scale.is_some() && !self.is_loading() {
                self.osd_since = ctx.input(|i| i.time);
            }
            self.last_scale = scale;
            ctx.request_repaint();
        }

        self.properties_window(&ctx);
        self.dialogs(&ctx);
        self.context_menu(&ctx);
        self.handle_copy(&ctx);
        self.fullscreen_indicator(&ctx);
        self.show_toast(&ctx);
        // Position du curseur inconnue alors que la fenêtre a le focus : on revérifie bientôt
        // (`raw_input_hook`), sans attendre un mouvement de la souris.
        #[cfg(windows)]
        if !self.drag_cursor && ctx.input(|i| i.focused && i.pointer.latest_pos().is_none()) {
            ctx.request_repaint_after_secs(CURSOR_POLL);
        }
        if std::mem::take(&mut self.open_requested) {
            self.open_dialog(frame);
        }
        if ctx.input(|i| i.pointer.any_released()) && self.menu.is_none() {
            self.swallow_click = false;
        }

        // Aperçu pendant le survol d'un fichier ou d'un dossier glissé.
        let hovered = ctx.input(|i| i.raw.hovered_files.first().map(|f| f.path.clone()));
        if let Some(hovered) = hovered {
            let text = match hovered {
                Some(p) if p.is_dir() => tr!("Déposez le dossier pour l'ouvrir", "Drop the folder to open it"),
                _ => tr!("Déposez l'image pour l'ouvrir", "Drop the image to open it"),
            };
            let screen = ctx.content_rect();
            let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, "drop".into()));
            painter.rect_filled(screen, 0.0, Color32::from_black_alpha(180));
            painter.text(
                screen.center(),
                egui::Align2::CENTER_CENTER,
                text,
                egui::FontId::proportional(28.0),
                Color32::WHITE,
            );
        }
    }
}

/// Intervalle de revérification de la position du curseur, quand elle est inconnue alors que la
/// fenêtre a le focus.
#[cfg(windows)]
const CURSOR_POLL: f32 = 0.25;

/// Durée minimale d'affichage d'une image lors d'un défilement (et entre deux quarts de tour
/// d'une rotation maintenue) : au plus 16 par seconde. En dessous, on ne distingue plus
/// vraiment chaque image. La durée réelle s'arrondit au rafraîchissement suivant de l'écran
/// (toutes les 16,7 ms à 60 Hz) : 0,07 s y donnerait le même rythme que 0,08 s.
const PACE: f64 = 0.06;

/// Touche de navigation ou de rotation maintenue : remplace la répétition du clavier, trop
/// rapide et irrégulière.
struct HeldKey {
    key: Key,
    action: Action,
    /// La répétition du clavier a commencé : c'est un maintien, pas un simple appui.
    repeating: bool,
    /// Dernière rotation (la navigation, elle, est rythmée par `advance`).
    last: f64,
}

/// Premier appui sur `key` dans ce lot d'événements (les répétitions du clavier exclues).
fn first_press(input: &egui::InputState, key: Key) -> bool {
    input
        .events
        .iter()
        .any(|e| matches!(e, egui::Event::Key { key: k, pressed: true, repeat: false, .. } if *k == key))
}

/// Échap pendant un glisser-déposer : c'est l'application d'origine (l'Explorateur) qui annule
/// le glisser ; si notre fenêtre a le focus, la touche nous parvient aussi et ne doit pas
/// quitter. Le glisser peut aussi être ressorti de la fenêtre sans être déposé : aucun
/// événement n'en signale alors la fin, mais il dure tant que le bouton de la souris est
/// enfoncé. Enfin l'annulation (fin du survol) peut arriver dans la même image que la touche,
/// d'où une courte marge.
#[derive(Default)]
struct DragGuard {
    hovering: bool,
    /// Ressorti de la fenêtre sans être déposé, bouton encore enfoncé.
    outside: bool,
    /// Dernier instant où le glisser était encore en cours.
    ended_at: Option<f64>,
}

impl DragGuard {
    const GRACE: f64 = 0.5;

    /// À appeler à chaque image ; indique si un glisser-déposer est (ou vient d'être) en cours.
    fn update(&mut self, hovering: bool, button_down: bool, now: f64) -> bool {
        if self.hovering && !hovering {
            self.outside = true;
        }
        if hovering || !button_down {
            // Le glisser revient, ou il est terminé (déposé ou annulé).
            self.outside = false;
        }
        if self.hovering || self.outside {
            self.ended_at = Some(now);
        }
        self.hovering = hovering;
        hovering || self.ended_at.is_some_and(|t| now - t < Self::GRACE)
    }
}

/// Dessine une flèche → (ou ←) centrée sur `p`, blanche cernée de noir.
fn paint_arrow_cursor(painter: &egui::Painter, p: Pos2, right: bool) {
    let s = if right { 1.0 } else { -1.0 };
    let pt = |x: f32, y: f32| p + vec2(x * s, y);
    let segments = [
        [pt(-11.0, 0.0), pt(10.0, 0.0)],
        [pt(10.0, 0.0), pt(2.0, -8.0)],
        [pt(10.0, 0.0), pt(2.0, 8.0)],
    ];
    for (width, color) in [(6.0, Color32::BLACK), (2.5, Color32::WHITE)] {
        for [a, b] in segments {
            painter.line_segment([a, b], egui::Stroke::new(width, color));
            painter.circle_filled(a, width / 2.0, color);
            painter.circle_filled(b, width / 2.0, color);
        }
    }
}

/// Chaque langue dans sa propre langue, avec sa région.
fn language_label(language: Language) -> String {
    match language {
        Language::Auto => tr!(
            "Automatique (celle du système, sinon anglais)",
            "Automatic (system language, otherwise English)"
        ),
        Language::English => "English (United States)".into(),
        Language::French => "Français (France)".into(),
    }
}

/// Titre de la fenêtre : nom de l'image affichée, s'il y en a une.
fn set_title(ctx: &egui::Context, path: Option<&Path>) {
    let title = match path {
        Some(path) => format!("{} — {APP_NAME}", file_name(path)),
        None => APP_NAME.to_owned(),
    };
    ctx.send_viewport_cmd(ViewportCommand::Title(title));
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn centered_text(ui: &egui::Ui, rect: Rect, text: &str, color: Color32) {
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::proportional(18.0),
        color,
    );
}

/// Empêche de faire sortir l'image de la vue ; centre l'axe si elle y tient.
fn clamp_pan(pan: Vec2, image: Vec2, view: Vec2) -> Vec2 {
    let limit = ((image - view) * 0.5).max(Vec2::ZERO);
    vec2(pan.x.clamp(-limit.x, limit.x), pan.y.clamp(-limit.y, limit.y))
}

/// Rectangle texturé, avec la texture tournée de `quarter_turns` × 90° (horaire).
fn rotated_mesh(texture: egui::TextureId, rect: Rect, quarter_turns: u8) -> Mesh {
    // Coins écran : haut-gauche, haut-droit, bas-droit, bas-gauche.
    let corners = [
        rect.left_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.left_bottom(),
    ];
    let uvs = [pos2(0.0, 0.0), pos2(1.0, 0.0), pos2(1.0, 1.0), pos2(0.0, 1.0)];
    let mut mesh = Mesh::with_texture(texture);
    for (i, corner) in corners.iter().enumerate() {
        // Rotation horaire de l'image ⇒ le coin écran i affiche l'UV (i - n) mod 4.
        let uv: Pos2 = uvs[(i + 4 - quarter_turns as usize % 4) % 4];
        mesh.vertices.push(egui::epaint::Vertex {
            pos: *corner,
            uv,
            color: Color32::WHITE,
        });
    }
    mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
    mesh
}

fn text_is_selected(ctx: &egui::Context) -> bool {
    ctx.plugin_opt::<egui::text_selection::LabelSelectionState>()
        .is_some_and(|p| p.lock().has_selection())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Une passe egui avec un libellé sélectionnable, comme dans les propriétés.
    fn frame(ctx: &egui::Context, events: Vec<egui::Event>) -> Rect {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))),
            events,
            ..Default::default()
        };
        let mut label_rect = Rect::NOTHING;
        let _ = ctx.run_ui(input, |ui| {
            label_rect = ui.add(egui::Label::new("Apple iPhone 15 Pro").selectable(true)).rect;
        });
        label_rect
    }

    fn click(pos: Pos2) -> Vec<egui::Event> {
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        vec![egui::Event::PointerMoved(pos), button(true), button(false)]
    }

    #[test]
    fn escape_is_ignored_during_and_just_after_a_drag() {
        let mut drag = DragGuard::default();
        assert!(!drag.update(false, false, 0.0));
        assert!(drag.update(true, true, 1.0));
        // Survol long sans nouvelle image, puis Échap : l'annulation arrive avec la touche.
        assert!(drag.update(false, false, 10.0));
        assert!(drag.update(false, false, 10.3));
        assert!(
            !drag.update(false, false, 10.6),
            "Échap doit de nouveau quitter après la marge"
        );
    }

    #[test]
    fn escape_is_ignored_while_the_drag_continues_outside() {
        let mut drag = DragGuard::default();
        drag.update(true, true, 1.0);
        // Sorti de la fenêtre sans déposer : bouton toujours enfoncé, bien après la marge.
        assert!(drag.update(false, true, 2.0));
        assert!(drag.update(false, true, 30.0));
        // Bouton relâché (glisser terminé ou annulé) : Échap quitte de nouveau après la marge.
        assert!(drag.update(false, false, 30.2));
        assert!(!drag.update(false, false, 31.0));
        // Un simple clic plus tard, sans glisser, ne bloque pas Échap.
        assert!(!drag.update(false, true, 40.0));
    }

    #[test]
    fn click_on_label_selects_its_text() {
        let ctx = egui::Context::default();
        let label = frame(&ctx, vec![]);
        assert!(!text_is_selected(&ctx));

        frame(&ctx, click(label.center()));
        frame(&ctx, vec![egui::Event::Copy]);
        assert!(text_is_selected(&ctx), "Ctrl+C doit copier le texte, pas l'image");

        // Un clic ailleurs (sur l'image) désélectionne : Ctrl+C copie de nouveau l'image.
        frame(&ctx, click(pos2(700.0, 500.0)));
        frame(&ctx, vec![egui::Event::Copy]);
        assert!(!text_is_selected(&ctx));
    }
}
