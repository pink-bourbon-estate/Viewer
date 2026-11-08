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
mod browse;
mod canvas;
mod color;
mod develop;
mod export;
mod extract;
mod filters;
mod geom;
mod library;
mod loader;
#[cfg(feature = "mockwin")]
mod mockwin;
mod palette;
mod panels;
mod presets;
mod settings;
mod theme;
mod updater;

use std::path::PathBuf;

const HELP: &str = "Viewer — image viewer and painting study tool

USAGE:
  viewer [IMAGE_OR_FOLDER]          open the viewer
  viewer --register                 make Viewer the default image viewer (this user)
  viewer --unregister               remove the file associations
  viewer --study IMAGE [OUT] [K]    headless: write every study view, a palette card
                                     and mixing recipes for K colours (default 8) to OUT
  viewer --update                    download and install the newest build
  viewer --version
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
    updater::cleanup();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--version" | "-V") => {
            println!("Viewer {}", updater::version_label());
            return std::process::ExitCode::SUCCESS;
        }
        Some("--update") => {
            return match updater::check() {
                Ok(Some(rel)) => match updater::install(&rel) {
                    Ok(_) => {
                        report(&format!("Updated to build {}.", rel.build), true);
                        std::process::ExitCode::SUCCESS
                    }
                    Err(e) => {
                        report(&e, false);
                        std::process::ExitCode::FAILURE
                    }
                },
                Ok(None) => {
                    report(&format!("Viewer is up to date ({}).", updater::version_label()), true);
                    std::process::ExitCode::SUCCESS
                }
                Err(e) => {
                    report(&e, false);
                    std::process::ExitCode::FAILURE
                }
            };
        }
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
        Some("--condense") => {
            // atelier --condense <preset folder> <out.json> [target]
            let (Some(dir), Some(out)) = (args.get(1), args.get(2)) else {
                eprintln!("usage: viewer --condense <preset folder> <out.json> [target count]");
                return std::process::ExitCode::FAILURE;
            };
            let k = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(45);
            return match presets::condense(std::path::Path::new(dir), k) {
                Ok((list, total)) => {
                    let s = serde_json::to_string_pretty(&list).unwrap_or_default();
                    if let Err(e) = std::fs::write(out, s) {
                        eprintln!("error: {e}");
                        return std::process::ExitCode::FAILURE;
                    }
                    println!("{total} files -> {} looks written to {out}", list.len());
                    std::process::ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::ExitCode::FAILURE
                }
            };
        }
        Some("--preset-sheet") => {
            // viewer --preset-sheet <image> <out.png>: every built-in look on one sheet.
            let (Some(inp), Some(out)) = (args.get(1), args.get(2)) else {
                return std::process::ExitCode::FAILURE;
            };
            let Ok(img) = loader::decode(std::path::Path::new(inp)) else {
                return std::process::ExitCode::FAILURE;
            };
            let small = image::imageops::thumbnail(&img, 300, 300);
            let list = presets::builtin();
            let cols = 6u32;
            let (tw, th) = (small.width(), small.height() + 14);
            let rows = (list.len() as u32).div_ceil(cols);
            let mut sheet = image::RgbaImage::from_pixel(tw * cols, th * rows, image::Rgba([30, 30, 30, 255]));
            for (i, p) in list.iter().enumerate() {
                let t = develop::apply(&small, &p.dev, 300.0 / img.width().max(1) as f32);
                let (x, y) = ((i as u32 % cols) * tw, (i as u32 / cols) * th);
                image::imageops::overlay(&mut sheet, &t, x as i64, y as i64);
                println!("{:>2}  {:<20} {}", i + 1, p.category, p.name);
                export::draw_number(&mut sheet, x as f32 + 14.0, (y + th - 7) as f32, i + 1, 2, [220, 220, 220]);
            }
            let _ = sheet.save(out);
            return std::process::ExitCode::SUCCESS;
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
