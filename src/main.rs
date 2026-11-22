#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod core_page;
mod page_system;
mod page_users;
mod cpu_topology;
mod addons;
mod file_activity;
mod inspector;
mod list_pages;
mod metrics;
mod page_perf;
mod page_processes;
mod pages;
mod perfdata;
mod pages2;
mod path_cache;
mod platform;
mod pro;
mod proc_info;
mod process_view;
mod sampler;
mod sensors;
mod settings;
mod summary_view;
mod system_lists;
mod summary_tiles;
mod system_actions;
mod signature;
#[cfg(test)]
mod test_ui;
mod theme;
mod version_info;
mod wait_chain;
mod widgets;
mod winsys;

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let screenshot = args
        .iter()
        .position(|a| a == "--screenshot")
        .and_then(|i| args.get(i + 1).cloned());

    settings::move_old_data_folders();

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Ultimate PC Manager")
        .with_inner_size([1335.0, 812.0])
        .with_min_inner_size([760.0, 480.0])
        .with_icon(std::sync::Arc::new(
            eframe::icon_data::from_png_bytes(include_bytes!("../assets/brand/icon-256.png")).expect("bundled icon"),
        ));
    if screenshot.is_some() {
        viewport = viewport.with_active(false);
        #[cfg(windows)]
        {
            viewport = viewport.with_window_level(egui::WindowLevel::AlwaysOnBottom);
        }
    }

    let mut options = eframe::NativeOptions { viewport, ..Default::default() };
    #[cfg(target_os = "macos")]
    if screenshot.is_some() {
        options.event_loop_builder = Some(Box::new(|b| {
            use winit::platform::macos::EventLoopBuilderExtMacOS;
            b.with_activate_ignoring_other_apps(false);
        }));
    }
    #[cfg(not(target_os = "macos"))]
    let _ = &mut options;

    eframe::run_native(
        "Ultimate PC Manager",
        options,
        Box::new(move |cc| Ok(Box::new(app::TaskApp::new(cc, screenshot)))),
    )
}
