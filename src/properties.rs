//! Contenu de la fenêtre « Propriétés de l'image » : infos fichier + métadonnées EXIF
//! (lisibles aussi pour les HEIC, ce que l'Explorateur Windows ne sait pas faire sans
//! l'extension HEIF).

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use exif::{In, Tag, Value};

use crate::decode::DecodedImage;

pub struct Properties {
    /// Champs principaux (libellé, valeur).
    pub summary: Vec<(String, String)>,
    /// Coordonnées GPS en degrés décimaux, si présentes.
    pub gps: Option<(f64, f64)>,
    /// Tous les champs EXIF bruts.
    pub all_exif: Vec<(String, String)>,
}

impl Properties {
    pub fn read(path: &Path, image: Option<&DecodedImage>) -> Self {
        let mut summary = Vec::new();
        let mut push = |k: String, v: String| summary.push((k, v));

        push(
            tr!("Nom", "Name"),
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        );
        push(
            tr!("Dossier", "Folder"),
            path.parent().map(|p| p.display().to_string()).unwrap_or_default(),
        );
        if let Ok(meta) = std::fs::metadata(path) {
            push(tr!("Taille", "Size"), human_size(meta.len()));
            if let Ok(modified) = meta.modified() {
                push(tr!("Modifié le", "Modified"), format_time(modified));
            }
        }
        if let Some(img) = image {
            let [w, h] = img.original_size;
            let megapixels = format_decimal((w as f64 * h as f64) / 1e6);
            push(
                tr!("Dimensions", "Dimensions"),
                tr!("{w} × {h} ({megapixels} Mpx)", "{w} × {h} ({megapixels} MP)"),
            );
            push(
                tr!("Profil couleur", "Color profile"),
                img.color_profile
                    .clone()
                    .unwrap_or_else(|| tr!("aucun (sRGB supposé)", "none (sRGB assumed)")),
            );
        }

        let mut gps = None;
        let mut all_exif = Vec::new();
        let exif = File::open(path)
            .ok()
            .and_then(|f| exif::Reader::new().read_from_container(&mut BufReader::new(f)).ok());
        if let Some(exif) = &exif {
            let get = |tag: Tag| {
                exif.get_field(tag, In::PRIMARY).map(|f| {
                    f.display_value()
                        .with_unit(exif)
                        .to_string()
                        .trim_matches('"')
                        .to_owned()
                })
            };
            let fields = [
                (tr!("Prise de vue", "Taken"), get(Tag::DateTimeOriginal)),
                (
                    tr!("Appareil", "Camera"),
                    match (get(Tag::Make), get(Tag::Model)) {
                        (Some(make), Some(model)) if model.starts_with(&make) => Some(model),
                        (Some(make), Some(model)) => Some(format!("{make} {model}")),
                        (a, b) => a.or(b),
                    },
                ),
                (tr!("Objectif", "Lens"), get(Tag::LensModel)),
                (tr!("Exposition", "Exposure"), get(Tag::ExposureTime)),
                (tr!("Ouverture", "Aperture"), get(Tag::FNumber)),
                ("ISO".to_owned(), get(Tag::PhotographicSensitivity)),
                (
                    tr!("Focale", "Focal length"),
                    match (get(Tag::FocalLength), get(Tag::FocalLengthIn35mmFilm)) {
                        (Some(f), Some(eq)) => Some(tr!("{f} (éq. 24×36 : {eq})", "{f} (35 mm equiv.: {eq})")),
                        (a, b) => a.or(b),
                    },
                ),
                ("Flash".to_owned(), get(Tag::Flash)),
                (tr!("Logiciel", "Software"), get(Tag::Software)),
                ("Altitude".to_owned(), get(Tag::GPSAltitude)),
            ];
            for (label, value) in fields {
                if let Some(v) = value {
                    push(label, v);
                }
            }

            gps = gps_coordinates(exif);
            if let Some((lat, lon)) = gps {
                push(tr!("Position GPS", "GPS location"), format!("{lat:.6}, {lon:.6}"));
            }

            for f in exif.fields() {
                if f.tag == Tag::MakerNote {
                    continue;
                }
                let mut value = f.display_value().with_unit(exif).to_string();
                if value.len() > 200 {
                    value.truncate(value.floor_char_boundary(200));
                    value.push('…');
                }
                all_exif.push((format!("{} ({})", f.tag, f.ifd_num), value));
            }
        }

        Self { summary, gps, all_exif }
    }
}

fn gps_coordinates(exif: &exif::Exif) -> Option<(f64, f64)> {
    let coord = |tag: Tag, ref_tag: Tag, negative: u8| {
        let Value::Rational(v) = &exif.get_field(tag, In::PRIMARY)?.value else {
            return None;
        };
        let deg = v.first()?.to_f64()
            + v.get(1).map_or(0.0, |m| m.to_f64() / 60.0)
            + v.get(2).map_or(0.0, |s| s.to_f64() / 3600.0);
        let is_negative = match &exif.get_field(ref_tag, In::PRIMARY)?.value {
            Value::Ascii(a) => a.first().and_then(|s| s.first()) == Some(&negative),
            _ => false,
        };
        Some(if is_negative { -deg } else { deg })
    };
    Some((
        coord(Tag::GPSLatitude, Tag::GPSLatitudeRef, b'S')?,
        coord(Tag::GPSLongitude, Tag::GPSLongitudeRef, b'W')?,
    ))
}

fn human_size(bytes: u64) -> String {
    let b = bytes as f64;
    if bytes < 1024 {
        return tr!("{bytes} octets", "{bytes} bytes");
    }
    let s = if b >= 1024.0 * 1024.0 {
        let size = format_decimal(b / 1024.0 / 1024.0);
        tr!("{size} Mo", "{size} MB")
    } else {
        let size = (b / 1024.0).round();
        tr!("{size} Ko", "{size} KB")
    };
    tr!("{s} ({bytes} octets)", "{s} ({bytes} bytes)")
}

/// Un chiffre après la virgule, avec le séparateur décimal de la langue.
fn format_decimal(value: f64) -> String {
    let text = format!("{value:.1}");
    if crate::i18n::french() {
        text.replace('.', ",")
    } else {
        text
    }
}

fn format_time(time: std::time::SystemTime) -> String {
    let format = if crate::i18n::french() {
        "%d/%m/%Y %H:%M:%S"
    } else {
        "%m/%d/%Y %-I:%M:%S %p"
    };
    jiff::Timestamp::try_from(time)
        .map(|t| t.to_zoned(jiff::tz::TimeZone::system()).strftime(format).to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(human_size(500), "500 octets");
        assert_eq!(human_size(2_500_000), "2,4 Mo (2500000 octets)");
    }
}
