// Pas de console noire derrière la fenêtre sous Windows (en release).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

// En premier : la macro `tr!` sert dans tous les modules.
#[macro_use]
mod i18n;

mod actions;
mod app;
mod config;
mod cursor;
mod decode;
#[cfg(windows)]
mod explorer;
mod folder;
mod properties;
mod sharp;
#[cfg(windows)]
mod taskbar;
mod watch;
#[cfg(windows)]
mod win;

use std::path::PathBuf;

use eframe::egui;

fn main() {
    // Chemin passé en argument : « Ouvrir avec » / association de fichiers Windows.
    let initial = std::env::args_os().nth(1).map(PathBuf::from);
    let loaded = config::Config::load();
    i18n::set(loaded.0.display.language);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(app::APP_NAME)
            .with_inner_size([1200.0, 850.0])
            .with_min_inner_size([400.0, 300.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };

    let result = eframe::run_native(
        app::APP_NAME,
        options,
        Box::new(|cc| Ok(Box::new(app::ViewerApp::new(cc, initial, loaded)))),
    );
    if let Err(e) = result {
        // En mode fenêtré il n'y a pas de console : on affiche l'erreur dans une boîte de dialogue.
        rfd::MessageDialog::new()
            .set_title(app::APP_NAME)
            .set_description(tr!("Impossible de démarrer : {e}", "Unable to start: {e}"))
            .set_level(rfd::MessageLevel::Error)
            .show();
        std::process::exit(1);
    }
}
