//! Souris côté système, au-delà de ce que voit egui. Après un retour par Alt+Tab, un changement
//! de bureau virtuel ou un passage en plein écran sous un curseur immobile, Windows ne signale
//! la position du curseur qu'au premier mouvement : jusque-là egui-winit ignore les clics, egui
//! ne sait pas où zoomer et le curseur flèche n'est pas dessiné. On lit donc la position réelle
//! et on l'annonce.

#[cfg(windows)]
use windows::Win32::Foundation::HWND;

use eframe::egui::Pos2;

pub struct OsCursor {
    #[cfg(windows)]
    hwnd: Option<HWND>,
}

impl OsCursor {
    #[cfg(windows)]
    pub fn new(window: &impl raw_window_handle::HasWindowHandle) -> Self {
        Self {
            hwnd: crate::win::hwnd(window),
        }
    }

    #[cfg(not(windows))]
    pub fn new<W>(_window: &W) -> Self {
        Self {}
    }

    /// Si le curseur est au-dessus de la fenêtre (et non d'une fenêtre qui la recouvre) :
    /// l'annonce à la fenêtre, comme un mouvement de la souris, pour que les clics suivants
    /// soient pris en compte, et renvoie sa position en points pour egui.
    pub fn announce(&self, pixels_per_point: f32) -> Option<Pos2> {
        #[cfg(windows)]
        {
            use windows::Win32::Foundation::{LPARAM, POINT, RECT, WPARAM};
            use windows::Win32::Graphics::Gdi::ScreenToClient;
            use windows::Win32::UI::WindowsAndMessaging::{
                GA_ROOT, GetAncestor, GetClientRect, GetCursorPos, PostMessageW, WM_MOUSEMOVE, WindowFromPoint,
            };

            let hwnd = self.hwnd?;
            let mut point = POINT::default();
            let mut client = RECT::default();
            // SAFETY: fenêtre de l'application ; POINT et RECT locaux.
            unsafe {
                GetCursorPos(&mut point).ok()?;
                let over_us = GetAncestor(WindowFromPoint(point), GA_ROOT) == hwnd;
                if !over_us || !ScreenToClient(hwnd, &mut point).as_bool() {
                    return None;
                }
                GetClientRect(hwnd, &mut client).ok()?;
            }
            let inside =
                (client.left..client.right).contains(&point.x) && (client.top..client.bottom).contains(&point.y);
            if !inside {
                return None;
            }
            // winit ignore un WM_MOUSEMOVE à la position qu'il connaît déjà (faux mouvements de
            // Windows), et il la garde quand le curseur quitte la fenêtre : on passe d'abord par
            // un pixel voisin pour que la vraie position soit bien transmise.
            let neighbour = if point.x + 1 < client.right {
                point.x + 1
            } else {
                point.x - 1
            };
            for x in [neighbour, point.x] {
                // Coordonnées client dans les 16 bits bas / hauts, comme un vrai WM_MOUSEMOVE.
                let lparam = ((point.y as u32 & 0xFFFF) << 16) | (x as u32 & 0xFFFF);
                // SAFETY: message posté à notre propre fenêtre.
                let _ = unsafe { PostMessageW(Some(hwnd), WM_MOUSEMOVE, WPARAM(0), LPARAM(lparam as isize)) };
            }
            Some(Pos2::new(point.x as f32, point.y as f32) / pixels_per_point)
        }
        #[cfg(not(windows))]
        {
            let _ = pixels_per_point;
            None
        }
    }

    /// Un bouton de la souris est-il enfoncé, même hors de la fenêtre ?
    pub fn button_down() -> bool {
        #[cfg(windows)]
        {
            use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON};
            // SAFETY: simple lecture de l'état des boutons. Bit de poids fort = enfoncé.
            [VK_LBUTTON, VK_RBUTTON]
                .iter()
                .any(|vk| unsafe { GetAsyncKeyState(i32::from(vk.0)) } < 0)
        }
        #[cfg(not(windows))]
        false
    }
}
