//! Interactions avec le système : presse-papiers, explorateur de fichiers, navigateur.

use std::path::Path;
#[cfg(not(windows))]
use std::process::Command;

use crate::decode::DecodedImage;

/// Le `Clipboard` est conservé par l'appelant : sous Linux, c'est lui qui sert
/// les données aux autres applications tant qu'il est vivant.
fn clipboard(slot: &mut Option<arboard::Clipboard>) -> Result<&mut arboard::Clipboard, String> {
    if slot.is_none() {
        let clipboard = arboard::Clipboard::new().map_err(|e| {
            tr!(
                "impossible d'accéder au presse-papiers ({e})",
                "cannot access the clipboard ({e})"
            )
        })?;
        *slot = Some(clipboard);
    }
    Ok(slot.as_mut().expect("initialisé ci-dessus"))
}

/// Copie les pixels de l'image dans le presse-papiers.
pub fn copy_image(slot: &mut Option<arboard::Clipboard>, img: &DecodedImage) -> Result<(), String> {
    let data = arboard::ImageData {
        width: img.width as usize,
        height: img.height as usize,
        bytes: std::borrow::Cow::Borrowed(&img.rgba),
    };
    clipboard(slot)?
        .set_image(data)
        .map_err(|e| tr!("impossible de copier l'image ({e})", "cannot copy the image ({e})"))
}

pub fn copy_text(slot: &mut Option<arboard::Clipboard>, text: &str) -> Result<(), String> {
    clipboard(slot)?
        .set_text(text)
        .map_err(|e| tr!("impossible de copier le texte ({e})", "cannot copy the text ({e})"))
}

/// Ouvre le dossier contenant `path` dans l'explorateur, avec le fichier sélectionné.
pub fn reveal_in_folder(path: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        crate::explorer::reveal(path);
        Ok(())
    }

    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn()
            .map(drop)
            .map_err(|e| file_manager_error(&e))
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let path = absolute(path)?;
        // Interface freedesktop des gestionnaires de fichiers, sinon on ouvre juste le dossier.
        let shown = Command::new("dbus-send")
            .args([
                "--session",
                "--dest=org.freedesktop.FileManager1",
                "--type=method_call",
                "/org/freedesktop/FileManager1",
                "org.freedesktop.FileManager1.ShowItems",
            ])
            .arg(format!("array:string:{}", file_uri(&path)))
            .arg("string:")
            .status()
            .is_ok_and(|s| s.success());
        if shown {
            return Ok(());
        }
        let dir = path.parent().unwrap_or(&path);
        Command::new("xdg-open")
            .arg(dir)
            .spawn()
            .map(drop)
            .map_err(|e| file_manager_error(&e))
    }
}

#[cfg(not(windows))]
fn file_manager_error(e: &std::io::Error) -> String {
    tr!(
        "impossible d'ouvrir le gestionnaire de fichiers ({e})",
        "cannot open the file manager ({e})"
    )
}

fn absolute(path: &Path) -> Result<std::path::PathBuf, String> {
    std::path::absolute(path).map_err(|e| {
        tr!(
            "impossible de déterminer le chemin complet ({e})",
            "cannot determine the full path ({e})"
        )
    })
}

/// Ouvre une URL dans le navigateur par défaut.
pub fn open_url(url: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        use windows::core::{PCWSTR, w};
        let file = crate::explorer::to_wide(url.as_ref());
        // SAFETY: chaînes UTF-16 terminées par 0, valides pendant l'appel.
        let result = unsafe {
            ShellExecuteW(
                None,
                w!("open"),
                PCWSTR(file.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };
        // Valeur > 32 = succès (convention de ShellExecute).
        if result.0 as usize > 32 {
            Ok(())
        } else {
            Err(tr!("impossible d'ouvrir le lien", "cannot open the link"))
        }
    }
    #[cfg(target_os = "macos")]
    let mut cmd = Command::new("open");
    #[cfg(not(any(windows, target_os = "macos")))]
    let mut cmd = Command::new("xdg-open");
    #[cfg(not(windows))]
    cmd.arg(url)
        .spawn()
        .map(drop)
        .map_err(|e| tr!("impossible d'ouvrir le lien ({e})", "cannot open the link ({e})"))
}

/// Boîte de dialogue « Propriétés » de l'Explorateur Windows.
#[cfg(windows)]
pub fn show_file_properties(path: &Path) -> Result<(), String> {
    use windows::Win32::UI::Shell::{SHOP_FILEPATH, SHObjectProperties};
    use windows::core::PCWSTR;

    let path = absolute(path)?;
    let wide = crate::explorer::to_wide(path.as_os_str());
    // SAFETY: `wide` est une chaîne UTF-16 terminée par 0, valide pendant l'appel.
    let ok = unsafe { SHObjectProperties(None, SHOP_FILEPATH, PCWSTR(wide.as_ptr()), PCWSTR::null()) };
    if ok.as_bool() {
        Ok(())
    } else {
        Err(tr!(
            "impossible d'afficher les propriétés du fichier",
            "cannot show the file properties"
        ))
    }
}

/// Met le fichier dans la Corbeille, d'où il peut être restauré. S'il ne peut pas y aller
/// (lecteur réseau…), Windows avertit avant de le supprimer définitivement.
#[cfg(windows)]
pub fn move_to_recycle_bin(path: &Path, window: Option<windows::Win32::Foundation::HWND>) -> Result<(), String> {
    use windows::Win32::UI::Shell::{
        FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_SILENT, FOF_WANTNUKEWARNING, SHFILEOPSTRUCTW,
        SHFileOperationW,
    };
    use windows::core::PCWSTR;

    let path = absolute(path)?;
    // Liste de chemins : chacun terminé par 0, la liste par un 0 de plus.
    let mut from = crate::explorer::to_wide(path.as_os_str());
    from.push(0);
    let mut operation = SHFILEOPSTRUCTW {
        hwnd: window.unwrap_or_default(),
        wFunc: FO_DELETE,
        pFrom: PCWSTR(from.as_ptr()),
        // La confirmation est demandée par la visionneuse.
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_WANTNUKEWARNING).0 as u16,
        ..Default::default()
    };
    // SAFETY: `from` (liste UTF-16 doublement terminée) vit pendant l'appel.
    let result = unsafe { SHFileOperationW(&mut operation) };
    if result == 0 && !operation.fAnyOperationsAborted.as_bool() {
        Ok(())
    } else {
        Err(tr!(
            "impossible de mettre le fichier dans la Corbeille (code {result})",
            "cannot move the file to the Recycle Bin (code {result})"
        ))
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
fn file_uri(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut uri = String::from("file://");
    for &b in path.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            uri.push(b as char);
        } else {
            uri.push_str(&format!("%{b:02X}"));
        }
    }
    uri
}
