#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]
#![forbid(unsafe_code)]

mod app;
mod assoc;
mod batch;
mod color;
mod export;
mod extract;
mod filters;
mod loader;
#[cfg(feature = "mockwin")]
mod mockwin;
mod palette;
mod settings;
mod theme;

use std::path::PathBuf;

const HELP: &str = "Viewer — image viewer and painting study tool

USAGE:
  viewer [IMAGE_OR_FOLDER]          open the viewer
  viewer --register                 make Viewer the default image viewer (this user)
  viewer --unregister               remove the file associations
  viewer --study IMAGE [OUT] [K]    headless: write every study view, a palette card
                                     and mixing recipes for K colours (default 8) to OUT
  viewer --help
";

/// Show a result when started without a console (Windows double-click / file association).
fn report(msg: &str, ok: bool) {
    println!("{msg}");
    #[cfg(windows)]
    {
        let _ = rfd::MessageDialog::new()
            .set_title("Viewer")
            .set_description(msg)
            .set_level(if ok { rfd::MessageLevel::Info } else { rfd::MessageLevel::Error })
            .show();
    }
    let _ = ok;
}

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--help" | "-h" | "/?") => {
            println!("{HELP}");
            return std::process::ExitCode::SUCCESS;
        }
        Some("--register") => {
            let r = assoc::register();
            let ok = r.is_ok();
            report(&r.unwrap_or_else(|e| e), ok);
            return if ok { std::process::ExitCode::SUCCESS } else { std::process::ExitCode::FAILURE };
        }
        Some("--unregister") => {
            let r = assoc::unregister();
            let ok = r.is_ok();
            report(&r.unwrap_or_else(|e| e), ok);
            return if ok { std::process::ExitCode::SUCCESS } else { std::process::ExitCode::FAILURE };
        }
        Some("--study") => {
            let Some(input) = args.get(1).map(PathBuf::from) else {
                eprintln!("{HELP}");
                return std::process::ExitCode::FAILURE;
            };
            let out = args.get(2).map(PathBuf::from).unwrap_or_else(|| {
                input.parent().map(|p| p.join("viewer-study")).unwrap_or_else(|| PathBuf::from("viewer-study"))
            });
            let k = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(8);
            let (pal, note) = settings::load_palette();
            if let Some(n) = note {
                eprintln!("{n}");
            }
            let solver = palette::Solver::new(pal);
            return match export::study_cli(&input, &out, k, &solver) {
                Ok(text) => {
                    println!("{text}\nWrote study files to {}", out.display());
                    std::process::ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::ExitCode::FAILURE
                }
            };
        }
        _ => {}
    }

    let open: Vec<PathBuf> = args.iter().filter(|a| !a.starts_with("--")).map(PathBuf::from).collect();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Viewer")
            .with_app_id("viewer")
            .with_inner_size([1400.0, 880.0])
            .with_min_inner_size([480.0, 320.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    let r = eframe::run_native(
        "Viewer",
        options,
        Box::new(move |cc| match app::App::new(cc, open) {
            Ok(a) => Ok(Box::new(a) as Box<dyn eframe::App>),
            Err(e) => Err(e.into()),
        }),
    );
    match r {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            report(&format!("Viewer could not start: {e}"), false);
            std::process::ExitCode::FAILURE
        }
    }
}
