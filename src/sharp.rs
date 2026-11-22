//! Affichage net sous 100 % : la carte graphique réduit l'image par filtrage bilinéaire entre
//! mipmaps, un peu flou. Une fois le zoom stable, l'image est recalculée en Lanczos3 aux
//! dimensions exactes de l'écran, puis affichée pixel pour pixel à la place. Pour l'affichage
//! ajusté d'une nouvelle image, ce calcul est fait dès le décodage : elle s'affiche nette
//! d'emblée.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

use eframe::egui::{self, TextureHandle, TextureId, TextureOptions, Vec2};

use crate::app::DecodeResult;
use crate::decode::{DecodedImage, lanczos_resize};

/// Délai sans changement de zoom ni de taille avant le recalcul (le zoom continu reste fluide).
const DELAY: f64 = 0.15;

type Key = (PathBuf, [u32; 2]);

/// Échelle « ajustée à la fenêtre » (points écran par pixel de texture) : l'image tient dans
/// `view` sans dépasser sa taille réelle (`one_to_one`).
pub fn fit_scale(view: Vec2, texture: Vec2, one_to_one: f32) -> f32 {
    (view.x / texture.x).min(view.y / texture.y).min(one_to_one)
}

/// Dimensions en pixels écran de la version Lanczos pour cette échelle ; `None` à 100 % et
/// au-delà (pixels nets, rien à recalculer).
pub fn lanczos_size(texture: Vec2, scale: f32, pixels_per_point: f32) -> Option<[u32; 2]> {
    let texels_to_pixels = scale * pixels_per_point;
    (texels_to_pixels < 0.999).then(|| [texture.x, texture.y].map(|v| (v * texels_to_pixels).round().max(1.0) as u32))
}

/// Version Lanczos calculée dès le décodage, pour l'affichage ajusté à la fenêtre d'une
/// nouvelle image : elle s'affiche nette d'emblée, sans bascule visible.
pub struct Prepared {
    size: [u32; 2],
    pixels: Vec<u8>,
}

/// Calcul fait dans le thread de décodage, avec la taille de la vue au moment de la demande.
pub fn prepare_fit(img: &DecodedImage, view: Vec2, pixels_per_point: f32) -> Option<Prepared> {
    let texture = Vec2::new(img.width as f32, img.height as f32);
    let one_to_one = img.original_size[0] as f32 / img.width as f32;
    let size = lanczos_size(texture, fit_scale(view, texture, one_to_one), pixels_per_point)?;
    let pixels = lanczos_resize(img, size[0], size[1])?;
    Some(Prepared { size, pixels })
}

pub struct SharpView {
    /// Dernière version nette (calculée ou préparée au décodage) : image, dimensions en pixels
    /// écran, texture.
    current: Option<(Key, TextureHandle)>,
    /// Calcul en cours (un seul à la fois).
    pending: Option<Key>,
    /// Dernier calcul en échec, à ne pas relancer en boucle.
    failed: Option<Key>,
    /// Affichage demandé, et depuis quand il n'a pas changé.
    target: Option<(Key, f64)>,
    tx: Sender<(Key, Option<Vec<u8>>)>,
    rx: Receiver<(Key, Option<Vec<u8>>)>,
}

impl Default for SharpView {
    fn default() -> Self {
        let (tx, rx) = channel();
        Self {
            current: None,
            pending: None,
            failed: None,
            target: None,
            tx,
            rx,
        }
    }
}

impl SharpView {
    /// Version préparée au décodage : utilisée telle quelle si elle correspond à l'affichage.
    pub fn preload(&mut self, ctx: &egui::Context, path: &Path, prepared: Prepared) {
        self.current = Some((
            (path.to_path_buf(), prepared.size),
            texture(ctx, prepared.size, &prepared.pixels),
        ));
    }

    /// À chaque image. `size` : dimensions affichées en pixels écran, `None` à 100 % et au-delà
    /// (pixels nets, rien à recalculer). Renvoie la texture nette si elle correspond exactement.
    pub fn update(
        &mut self,
        ctx: &egui::Context,
        path: &Path,
        image: &DecodeResult,
        size: Option<[u32; 2]>,
    ) -> Option<TextureId> {
        while let Ok((key, pixels)) = self.rx.try_recv() {
            if self.pending.as_ref() == Some(&key) {
                self.pending = None;
            }
            match pixels {
                Some(pixels) => {
                    let texture = texture(ctx, key.1, &pixels);
                    self.current = Some((key, texture));
                }
                None => self.failed = Some(key),
            }
        }
        if self.current.as_ref().is_some_and(|((p, _), _)| p != path) {
            self.current = None;
        }

        let size = size?;
        let key = (path.to_path_buf(), size);
        if let Some((k, texture)) = &self.current
            && *k == key
        {
            return Some(texture.id());
        }

        let now = ctx.input(|i| i.time);
        let since = match &self.target {
            Some((k, since)) if *k == key => *since,
            // Zoom ou redimensionnement en cours sur la même image : on attend qu'il se stabilise.
            Some(((p, _), _)) if p == path => {
                self.target = Some((key.clone(), now));
                now
            }
            // Image qui vient d'apparaître (sans version préparée) : rien à attendre.
            _ => {
                self.target = Some((key.clone(), now - DELAY));
                now - DELAY
            }
        };
        let wait = since + DELAY - now;
        if wait > 0.0 {
            ctx.request_repaint_after_secs(wait as f32);
        } else if self.pending.is_none() && self.failed.as_ref() != Some(&key) {
            self.pending = Some(key.clone());
            let (tx, ctx, image) = (self.tx.clone(), ctx.clone(), image.clone());
            std::thread::spawn(move || {
                let pixels = image
                    .as_ref()
                    .as_ref()
                    .ok()
                    .and_then(|img| lanczos_resize(img, size[0], size[1]));
                let _ = tx.send((key, pixels));
                ctx.request_repaint();
            });
        }
        None
    }
}

/// Un texel par pixel écran : aucun filtrage à l'affichage.
fn texture(ctx: &egui::Context, [w, h]: [u32; 2], pixels: &[u8]) -> TextureHandle {
    let color = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], pixels);
    ctx.load_texture("sharp", color, TextureOptions::NEAREST)
}
