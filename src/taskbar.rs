//! Barre des tâches Windows : un clic molette sur le bouton de la visionneuse doit en lancer une
//! nouvelle instance. L'Explorateur se sert pour cela des propriétés « AppUserModel » de la
//! fenêtre (identifiant et commande de relance), qu'on renseigne explicitement.

use raw_window_handle::HasWindowHandle;
use windows::Win32::Foundation::{HWND, PROPERTYKEY};
use windows::Win32::Storage::EnhancedStorage::{
    PKEY_AppUserModel_ID, PKEY_AppUserModel_RelaunchCommand, PKEY_AppUserModel_RelaunchDisplayNameResource,
};
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Variant::VT_LPWSTR;
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow};
use windows::Win32::UI::Shell::SHStrDupW;
use windows::core::HSTRING;

const APP_ID: &str = "HeicViewer.HeicViewer";

/// La commande de relance n'est prise en compte que si l'identifiant est défini : on pose
/// l'identifiant en premier.
const KEYS: [PROPERTYKEY; 3] = [
    PKEY_AppUserModel_ID,
    PKEY_AppUserModel_RelaunchCommand,
    PKEY_AppUserModel_RelaunchDisplayNameResource,
];

/// Propriétés posées sur la fenêtre, à retirer avant sa fermeture (sinon Windows ne libère pas
/// les ressources associées).
pub struct Relaunch(HWND);

impl Relaunch {
    /// `None` si les propriétés n'ont pas pu être posées : le clic molette ne fera alors rien.
    pub fn enable(window: &impl HasWindowHandle) -> Option<Self> {
        let hwnd = crate::win::hwnd(window)?;
        let exe = std::env::current_exe().ok()?;
        let command = format!("\"{}\"", exe.display());
        let values = [APP_ID, &command, crate::app::APP_NAME];
        let store = store(hwnd)?;
        for (key, value) in KEYS.iter().zip(values) {
            // SAFETY: clé et valeur valides pendant l'appel ; la fenêtre copie la valeur.
            unsafe { store.SetValue(key, &lpwstr(value)?) }.ok()?;
        }
        Some(Self(hwnd))
    }

    /// À appeler à la fermeture, tant que la fenêtre existe encore.
    pub fn clear(&self) {
        let Some(store) = store(self.0) else { return };
        for key in KEYS.iter().rev() {
            // SAFETY: un PROPVARIANT vide (VT_EMPTY) retire la propriété.
            let _ = unsafe { store.SetValue(key, &PROPVARIANT::default()) };
        }
    }
}

/// Les valeurs sont enregistrées sur la fenêtre dès `SetValue` : `Commit` est inutile.
fn store(hwnd: HWND) -> Option<IPropertyStore> {
    // SAFETY: `hwnd` est la fenêtre de l'application.
    unsafe { SHGetPropertyStoreForWindow(hwnd) }.ok()
}

/// Ces propriétés doivent être de type `VT_LPWSTR` (la conversion `From<&str>` du crate
/// produit un `VT_BSTR`).
fn lpwstr(text: &str) -> Option<PROPVARIANT> {
    // SAFETY: type et pointeur cohérents ; à la destruction, `PropVariantClear` libère la
    // chaîne avec `CoTaskMemFree`, l'allocateur de `SHStrDupW`.
    unsafe {
        let value = SHStrDupW(&HSTRING::from(text)).ok()?;
        let mut variant = PROPVARIANT::default();
        let inner = &mut *variant.Anonymous.Anonymous;
        inner.vt = VT_LPWSTR;
        inner.Anonymous.pwszVal = value;
        Some(variant)
    }
}
