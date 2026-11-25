#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod startup;
mod theme;
mod thumbs;
mod ui;
mod util;
mod workers;

fn main() -> eframe::Result {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let paths = gamut_core::Paths::resolve().expect("cannot create data folder");
    // One instance at a time: two would fight over the library and settings.
    let Some(_lock) = paths.lock() else {
        rfd::MessageDialog::new()
            .set_title("Gamut")
            .set_description("Gamut is already running.")
            .show();
        return Ok(());
    };
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Gamut")
        .with_inner_size([1440.0, 880.0])
        .with_min_inner_size([720.0, 480.0]);
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
        Box::new(move |cc| Ok(Box::new(ui::GamutApp::new(cc, paths)?))),
    )
}
