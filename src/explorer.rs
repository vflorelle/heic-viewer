//! « Afficher dans le dossier » sous Windows : réutilise l'onglet de l'Explorateur qui
//! montre déjà le dossier (en l'activant), sinon ouvre une nouvelle fenêtre.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize,
    IServiceProvider,
};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationSelectionItemPattern, TreeScope_Descendants, UIA_ControlTypePropertyId,
    UIA_SelectionItemPatternId, UIA_TabItemControlTypeId,
};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    IFolderView, ILCreateFromPathW, ILFindLastID, ILFree, IPersistFolder2, IShellBrowser, IShellWindows,
    SHGetNameFromIDList, SHGetPathFromIDListW, SHOpenFolderAndSelectItems, SID_STopLevelBrowser, SIGDN_NORMALDISPLAY,
    SVSI_DESELECTOTHERS, SVSI_ENSUREVISIBLE, SVSI_FOCUSED, SVSI_SELECT, ShellWindows,
};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, GA_ROOT, GetAncestor, IsIconic, SW_RESTORE, SetForegroundWindow, ShowWindow,
};
use windows::core::{Interface, PCWSTR, w};

/// Lance l'opération dans un thread dédié (COM y est initialisé ; l'Explorateur peut être lent).
pub fn reveal(path: &Path) {
    let Ok(file) = std::path::absolute(path) else { return };
    std::thread::spawn(move || {
        // SAFETY: COM initialisé pour ce thread et libéré à la fin ; les PIDL sont libérées par `Pidl`.
        unsafe {
            let com = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let Some(pidl) = Pidl::from_path(&file) else { return };
            if !select_in_open_tab(&file, &pidl).unwrap_or(false) {
                let _ = SHOpenFolderAndSelectItems(pidl.0, None, 0);
            }
            drop(pidl);
            if com.is_ok() {
                CoUninitialize();
            }
        }
    });
}

/// Cherche un onglet (ou une fenêtre) de l'Explorateur ouvert sur le dossier du fichier :
/// l'active, y sélectionne le fichier et passe la fenêtre au premier plan.
unsafe fn select_in_open_tab(file: &Path, file_pidl: &Pidl) -> windows::core::Result<bool> {
    let Some(dir) = file.parent() else { return Ok(false) };
    let shell_windows: IShellWindows = unsafe { CoCreateInstance(&ShellWindows, None, CLSCTX_ALL)? };

    // Avec les onglets (Windows 11), chaque onglet est une entrée distincte.
    for i in 0..unsafe { shell_windows.Count()? } {
        let Ok(browser) = (unsafe { tab_browser(&shell_windows, i) }) else {
            continue;
        };
        let Ok(view) = (unsafe { browser.QueryActiveShellView() }) else {
            continue;
        };
        let Ok(folder) = (unsafe {
            view.cast::<IFolderView>()
                .and_then(|v| v.GetFolder::<IPersistFolder2>())
        }) else {
            continue;
        };
        let Ok(folder_pidl) = (unsafe { folder.GetCurFolder() }) else {
            continue;
        };
        let folder_pidl = Pidl(folder_pidl);
        if !folder_pidl.path().is_some_and(|p| same_path(&p, dir)) {
            continue;
        }

        let tab = unsafe { browser.GetWindow()? };
        let window = unsafe { GetAncestor(tab, GA_ROOT) };
        unsafe { activate_tab(window, tab, &folder_pidl) };
        unsafe {
            view.SelectItem(
                ILFindLastID(file_pidl.0),
                (SVSI_SELECT.0 | SVSI_DESELECTOTHERS.0 | SVSI_ENSUREVISIBLE.0 | SVSI_FOCUSED.0) as u32,
            )?;
            if IsIconic(window).as_bool() {
                let _ = ShowWindow(window, SW_RESTORE);
            }
            let _ = SetForegroundWindow(window);
        }
        return Ok(true);
    }
    Ok(false)
}

unsafe fn tab_browser(shell_windows: &IShellWindows, index: i32) -> windows::core::Result<IShellBrowser> {
    let item = unsafe { shell_windows.Item(&VARIANT::from(index))? };
    unsafe { item.cast::<IServiceProvider>()?.QueryService(&SID_STopLevelBrowser) }
}

/// L'onglet actif est le premier `ShellTabWindowClass` de la fenêtre. Aucune API ne permet
/// d'en changer : on passe par l'automatisation d'interface (comme un clic sur l'onglet),
/// en essayant les onglets portant le nom du dossier jusqu'à tomber sur le bon.
unsafe fn activate_tab(window: HWND, tab: HWND, folder: &Pidl) {
    let active_tab = || unsafe { FindWindowExW(Some(window), None, w!("ShellTabWindowClass"), PCWSTR::null()) };
    if active_tab().is_ok_and(|t| t == tab) || window == tab {
        return;
    }
    let Some(name) = folder.display_name() else { return };
    let Ok(uia) = (unsafe { CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_ALL) }) else {
        return;
    };
    let Ok(items) = (unsafe {
        uia.ElementFromHandle(window).and_then(|root| {
            let condition =
                uia.CreatePropertyCondition(UIA_ControlTypePropertyId, &VARIANT::from(UIA_TabItemControlTypeId.0))?;
            root.FindAll(TreeScope_Descendants, &condition)
        })
    }) else {
        return;
    };
    for i in 0..unsafe { items.Length() }.unwrap_or(0) {
        let Ok(item) = (unsafe { items.GetElement(i) }) else {
            continue;
        };
        if unsafe { item.CurrentName() }.is_ok_and(|n| n == name.as_str())
            && let Ok(pattern) =
                unsafe { item.GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(UIA_SelectionItemPatternId) }
            && unsafe { pattern.Select() }.is_ok()
            && active_tab().is_ok_and(|t| t == tab)
        {
            return;
        }
    }
}

/// PIDL (identifiant d'élément du Shell) libérée automatiquement.
struct Pidl(*mut ITEMIDLIST);

impl Pidl {
    /// `path` doit être absolu.
    fn from_path(path: &Path) -> Option<Self> {
        let wide = to_wide(path.as_os_str());
        // SAFETY: chaîne UTF-16 terminée par 0.
        let pidl = unsafe { ILCreateFromPathW(PCWSTR(wide.as_ptr())) };
        (!pidl.is_null()).then_some(Self(pidl))
    }

    fn path(&self) -> Option<std::path::PathBuf> {
        let mut buf = [0u16; 260];
        // SAFETY: PIDL valide, tampon de MAX_PATH caractères.
        unsafe { SHGetPathFromIDListW(self.0, &mut buf) }.as_bool().then(|| {
            let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            String::from_utf16_lossy(&buf[..len]).into()
        })
    }

    /// Nom affiché du dossier (celui de l'onglet).
    fn display_name(&self) -> Option<String> {
        // SAFETY: PIDL valide ; la chaîne renvoyée est libérée avec CoTaskMemFree.
        unsafe {
            let name = SHGetNameFromIDList(self.0, SIGDN_NORMALDISPLAY).ok()?;
            let text = name.to_string().ok();
            CoTaskMemFree(Some(name.0 as _));
            text
        }
    }
}

impl Drop for Pidl {
    fn drop(&mut self) {
        // SAFETY: PIDL allouée par le Shell, libérée une seule fois.
        unsafe { ILFree(Some(self.0)) };
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| p.to_string_lossy().trim_end_matches('\\').to_lowercase();
    norm(a) == norm(b)
}

pub fn to_wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}
