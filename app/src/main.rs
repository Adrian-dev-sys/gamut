#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ach_tab;
mod achievements;
mod avif;
mod editor;
mod gamepad;
mod keycheck;
mod sessions_view;
mod logging;
mod osk;
mod scan_ui;
mod search_pop;
mod lookup;
mod notify;
mod picker;
mod startup;
mod stats_tab;
mod theme;
mod thumbs;
mod tray;
mod ui;
mod util;
mod video;
mod workers;

fn main() -> eframe::Result {
    if std::env::args().any(|a| a == "--quit") {
        tray::signal_quit();
        return Ok(());
    }
    let paths = gamut_core::Paths::resolve().expect("cannot create data folder");
    // One instance at a time: two would fight over the library and settings.
    let Some(_lock) = paths.lock() else {
        // Running (maybe hidden in the tray): bring it back instead of complaining.
        if tray::signal_running() {
            return Ok(());
        }
        rfd::MessageDialog::new()
            .set_title("Gamut")
            .set_description("Gamut is already running.")
            .show();
        return Ok(());
    };
    // After the lock, so a second instance doesn't rotate the running one's log.
    logging::init(&paths.root);
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Gamut")
        .with_inner_size([1440.0, 880.0])
        .with_min_inner_size([720.0, 480.0]);
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/gamut.png")) {
        viewport = viewport.with_icon(icon);
    }
    // Restore last placement; skip obviously bogus values (e.g. a disconnected monitor far away).
    if let Some(w) = gamut_core::Config::load(&paths.config).window {
        if w.width >= 720.0 && w.height >= 480.0 {
            viewport = viewport.with_inner_size([w.width, w.height]);
        }
        if w.x.abs() < 20_000.0 && w.y.abs() < 20_000.0 {
            viewport = viewport.with_position([w.x, w.y]);
        }
        // Maximizing happens after the first frame (see startup.rs) to avoid a white flash.
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native(
        "Gamut",
        options,
        Box::new(move |cc| {
            // Loaders for images in game descriptions (http URLs).
            avif::register();
            std::thread::spawn(video::remove_old_cache);
            egui_extras::install_image_loaders(&cc.egui_ctx);
            let mut app = ui::GamutApp::new(cc, paths)?;
            if std::env::args().any(|a| a == "--tray") {
                app.start_in_tray();
            }
            Ok(Box::new(app))
        }),
    )
}
