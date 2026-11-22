//! Décodage d'un fichier image vers un buffer RGBA 8 bits.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use image::{DynamicImage, ImageDecoder, ImageReader};

/// Extensions reconnues (HEIF et AVIF via le crate `heic`, les autres via le crate `image`).
pub const EXTENSIONS: &[&str] = &[
    "heic", "heif", "hif", "avif", "jpg", "jpeg", "png", "gif", "bmp", "webp", "tif", "tiff",
];

pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// Taille réelle de la photo (avant une éventuelle réduction pour le GPU).
    pub original_size: [u32; 2],
    /// Profil couleur d'origine (converti en sRGB pour l'affichage).
    pub color_profile: Option<String>,
    pub elapsed: Duration,
}

impl DecodedImage {
    fn fit_within(&mut self, max_side: u32) {
        if self.width <= max_side && self.height <= max_side {
            return;
        }
        let ratio = max_side as f64 / self.width.max(self.height) as f64;
        let w = ((self.width as f64 * ratio).round() as u32).max(1);
        let h = ((self.height as f64 * ratio).round() as u32).max(1);
        let Some(buf) = image::RgbaImage::from_raw(self.width, self.height, std::mem::take(&mut self.rgba)) else {
            return;
        };
        let resized = image::imageops::resize(&buf, w, h, image::imageops::FilterType::Triangle);
        self.width = w;
        self.height = h;
        self.rgba = resized.into_raw();
    }
}

/// Réduction de qualité (Lanczos3) aux dimensions exactes d'affichage, plus nette que le
/// filtrage de la carte graphique. `None` si les dimensions sont invalides.
pub fn lanczos_resize(img: &DecodedImage, width: u32, height: u32) -> Option<Vec<u8>> {
    use fast_image_resize::images::{Image, ImageRef};
    use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};

    let src = ImageRef::new(img.width, img.height, &img.rgba, PixelType::U8x4).ok()?;
    let mut dst = Image::new(width, height, PixelType::U8x4);
    // Alpha non prémultiplié (comme la texture egui) : le crate s'occupe de la (dé)multiplication.
    let options = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3));
    Resizer::new().resize(&src, &mut dst, &options).ok()?;
    Some(dst.into_vec())
}

pub fn is_supported(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Annulation d'un décodage devenu inutile (image passée avant d'être affichée).
#[derive(Default)]
pub struct Cancel(AtomicBool);

impl Cancel {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    fn bail_if_cancelled(&self) -> Result<(), String> {
        if self.is_cancelled() {
            Err(tr!("décodage annulé", "decoding canceled"))
        } else {
            Ok(())
        }
    }
}

impl heic::Stop for Cancel {
    fn check(&self) -> Result<(), heic::StopReason> {
        if self.is_cancelled() {
            Err(heic::StopReason::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// Décode `path` en RGBA sRGB, réduit si un côté dépasse `max_side` (taille max d'une texture GPU).
pub fn decode_file(path: &Path, max_side: u32, cancel: &Cancel) -> Result<DecodedImage, String> {
    let start = Instant::now();
    let data = std::fs::read(path).map_err(|e| tr!("Lecture impossible : {e}", "Cannot read the file: {e}"))?;
    cancel.bail_if_cancelled()?;

    // On se fie aux octets plutôt qu'à l'extension : certains .jpg exportés
    // depuis un iPhone sont en réalité des HEIC (et inversement).
    let (width, height, mut rgba, profile) = if looks_like_heif(&data) {
        decode_heif(&data, cancel)?
    } else {
        decode_other(&data)?
    };
    cancel.bail_if_cancelled()?;
    let color_profile = profile.and_then(|p| to_srgb(&mut rgba, &p));

    let mut img = DecodedImage {
        width,
        height,
        rgba,
        original_size: [width, height],
        color_profile,
        elapsed: Duration::ZERO,
    };
    img.fit_within(max_side);
    img.elapsed = start.elapsed();
    Ok(img)
}

/// Espace couleur source déclaré par le fichier.
enum SourceProfile {
    Icc(Vec<u8>),
    /// HEIF sans ICC mais avec des primaires CICP Display P3 (code 12).
    DisplayP3,
}

/// Convertit les pixels en sRGB (ce qu'attend l'affichage) depuis le profil du fichier :
/// sans ça, une photo iPhone en Display P3 paraît terne. Retourne le nom du profil,
/// ou `None` si aucune conversion n'a été faite.
fn to_srgb(rgba: &mut Vec<u8>, source: &SourceProfile) -> Option<String> {
    use moxcms::{ColorProfile, Layout, TransformOptions};
    use rayon::prelude::*;

    let profile = match source {
        SourceProfile::Icc(icc) => ColorProfile::new_from_slice(icc).ok()?,
        SourceProfile::DisplayP3 => ColorProfile::new_display_p3(),
    };
    let name = match source {
        SourceProfile::DisplayP3 => "Display P3".to_owned(),
        SourceProfile::Icc(_) => profile_name(&profile).unwrap_or_else(|| tr!("ICC intégré", "embedded ICC")),
    };
    // Déjà en sRGB : rien à faire.
    if name.to_ascii_lowercase().contains("srgb") {
        return Some(name);
    }
    let transform = profile
        .create_transform_8bit(
            Layout::Rgba,
            &ColorProfile::new_srgb(),
            Layout::Rgba,
            TransformOptions::default(),
        )
        .ok()?;
    let mut out = vec![0u8; rgba.len()];
    const CHUNK: usize = 4 * 64 * 1024;
    let ok = rgba
        .par_chunks(CHUNK)
        .zip(out.par_chunks_mut(CHUNK))
        .all(|(src, dst)| transform.transform(src, dst).is_ok());
    if !ok {
        return None;
    }
    *rgba = out;
    Some(tr!("{name} (converti en sRGB)", "{name} (converted to sRGB)"))
}

fn profile_name(profile: &moxcms::ColorProfile) -> Option<String> {
    use moxcms::ProfileText;
    let text = match profile.description.as_ref()? {
        ProfileText::PlainString(s) => s.clone(),
        ProfileText::Localizable(v) => v.first()?.value.clone(),
        ProfileText::Description(d) => d.ascii_string.clone(),
    };
    let text = text.trim_matches(char::from(0)).trim().to_owned();
    (!text.is_empty()).then_some(text)
}

/// Détecte un conteneur ISOBMFF de type HEIF (`ftyp` + brand heic/heix/avif/mif1...).
fn looks_like_heif(data: &[u8]) -> bool {
    if data.len() < 12 || &data[4..8] != b"ftyp" {
        return false;
    }
    let box_len = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
    let end = box_len.clamp(12, data.len().min(256));
    // major brand (8..12) puis compatible brands à partir de 16
    std::iter::once(&data[8..12])
        .chain(
            data.get(16..end)
                .unwrap_or(&[])
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| &c[..]),
        )
        .any(|brand| {
            matches!(
                brand,
                b"heic" | b"heix" | b"heim" | b"heis" | b"hevc" | b"hevx" | b"avif" | b"avis" | b"mif1" | b"msf1"
            )
        })
}

type Decoded = (u32, u32, Vec<u8>, Option<SourceProfile>);

fn decode_heif(data: &[u8], cancel: &Cancel) -> Result<Decoded, String> {
    let decoder = heic::DecoderConfig::new();
    let out = decoder
        .decode_request(data)
        .with_output_layout(heic::PixelLayout::Rgba8)
        .with_stop(cancel)
        .decode()
        .map_err(|e| tr!("Décodage HEIF impossible : {e}", "Cannot decode the HEIF image: {e}"))?;
    let profile = match decoder.extract_icc(data) {
        Ok(Some(icc)) => Some(SourceProfile::Icc(icc)),
        _ => heic::ImageInfo::from_bytes(data)
            .ok()
            .filter(|info| info.color_primaries == 12)
            .map(|_| SourceProfile::DisplayP3),
    };
    Ok((out.width, out.height, out.data, profile))
}

fn decode_other(data: &[u8]) -> Result<Decoded, String> {
    let reader = ImageReader::new(std::io::Cursor::new(data))
        .with_guessed_format()
        .map_err(|e| unknown_format(&e))?;
    let mut decoder = reader.into_decoder().map_err(|e| unknown_format(&e))?;
    let orientation = decoder.orientation().ok();
    let profile = decoder.icc_profile().ok().flatten().map(SourceProfile::Icc);
    let mut img = DynamicImage::from_decoder(decoder)
        .map_err(|e| tr!("Décodage impossible : {e}", "Cannot decode the image: {e}"))?;
    if let Some(o) = orientation {
        img.apply_orientation(o);
    }
    let rgba = img.into_rgba8();
    Ok((rgba.width(), rgba.height(), rgba.into_raw(), profile))
}

fn unknown_format(e: &impl std::fmt::Display) -> String {
    tr!("Format non reconnu : {e}", "Unknown format: {e}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_heif_brands() {
        let mut heic = vec![0, 0, 0, 24];
        heic.extend_from_slice(b"ftypheic\0\0\0\0mif1heic");
        assert!(looks_like_heif(&heic));

        let mut mif1_only = vec![0, 0, 0, 20];
        mif1_only.extend_from_slice(b"ftypXXXX\0\0\0\0mif1");
        assert!(looks_like_heif(&mif1_only));

        let mut avif = vec![0, 0, 0, 20];
        avif.extend_from_slice(b"ftypavif\0\0\0\0avif");
        assert!(looks_like_heif(&avif));

        assert!(!looks_like_heif(b"\xFF\xD8\xFF\xE0 jpeg data"));
        let mut mp4 = vec![0, 0, 0, 20];
        mp4.extend_from_slice(b"ftypisom\0\0\0\0mp41");
        assert!(!looks_like_heif(&mp4));
    }

    #[test]
    fn display_p3_is_converted_to_more_saturated_srgb() {
        // Un orange en P3 interprété tel quel en sRGB paraît terne : une fois converti,
        // le rouge monte et le bleu descend.
        let mut px = vec![200, 100, 50, 255];
        let name = to_srgb(&mut px, &SourceProfile::DisplayP3).unwrap();
        assert!(name.starts_with("Display P3"));
        assert!(px[0] > 200 && px[2] < 50, "{px:?}");
        assert_eq!(px[3], 255);
    }

    #[test]
    fn lanczos_keeps_flat_colors_and_target_size() {
        let img = DecodedImage {
            width: 400,
            height: 300,
            rgba: [10, 120, 230, 255].repeat(400 * 300),
            original_size: [400, 300],
            color_profile: None,
            elapsed: Duration::ZERO,
        };
        let out = lanczos_resize(&img, 123, 77).unwrap();
        assert_eq!(out.len(), 123 * 77 * 4);
        assert!(out.as_chunks::<4>().0.iter().all(|p| *p == [10, 120, 230, 255]));
    }

    #[test]
    fn embedded_icc_profile_is_used() {
        let icc = moxcms::ColorProfile::new_display_p3().encode().unwrap();
        let mut a = vec![200, 100, 50, 255];
        let mut b = a.clone();
        to_srgb(&mut a, &SourceProfile::Icc(icc)).unwrap();
        to_srgb(&mut b, &SourceProfile::DisplayP3).unwrap();
        assert_eq!(a, b);
    }

    /// `HEIC_SAMPLE=/chemin/photo.heic cargo test -- --ignored`
    #[test]
    #[ignore]
    fn decodes_sample_file() {
        let path = std::env::var("HEIC_SAMPLE").expect("HEIC_SAMPLE non défini");
        let img = decode_file(Path::new(&path), u32::MAX, &Cancel::default()).unwrap();
        assert_eq!(img.rgba.len(), (img.width * img.height * 4) as usize);
        // Un décodage annulé s'arrête au lieu d'aller au bout.
        let cancel = Cancel::default();
        cancel.cancel();
        let data = std::fs::read(&path).unwrap();
        let err = decode_heif(&data, &cancel).err().expect("décodage annulé attendu");
        assert!(err.contains("cancelled"), "{err}");
        println!(
            "{}x{} en {:?}, profil : {:?}",
            img.width, img.height, img.elapsed, img.color_profile
        );
    }
}
