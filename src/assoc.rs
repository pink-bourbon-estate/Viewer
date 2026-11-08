//! Register Viewer as the image viewer for the current user.
//!
//! Windows: per-user (HKCU) ProgId, OpenWithProgids for each extension, and
//! a RegisteredApplications capability so it appears in Settings → Default
//! apps. Windows does not let programs silently take over the default (the
//! choice is hash-protected), so after registering we open the Settings page
//! where one click sets it for every image type.
//!
//! Linux: a .desktop file in ~/.local/share/applications plus
//! `xdg-mime default` for every image MIME type.

pub const EXTENSIONS: &[(&str, &str)] = &[
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("jpe", "image/jpeg"),
    ("jfif", "image/jpeg"),
    ("png", "image/png"),
    ("webp", "image/webp"),
    ("gif", "image/gif"),
    ("bmp", "image/bmp"),
    ("tif", "image/tiff"),
    ("tiff", "image/tiff"),
    ("tga", "image/x-tga"),
    ("ico", "image/vnd.microsoft.icon"),
    ("qoi", "image/qoi"),
    ("pbm", "image/x-portable-bitmap"),
    ("pgm", "image/x-portable-graymap"),
    ("ppm", "image/x-portable-pixmap"),
    ("pnm", "image/x-portable-anymap"),
];

pub fn is_image(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            let e = e.to_ascii_lowercase();
            EXTENSIONS.iter().any(|(x, _)| *x == e)
        })
        .unwrap_or(false)
}

fn exe_path() -> Result<String, String> {
    std::env::current_exe()
        .map_err(|e| format!("cannot find own executable: {e}"))
        .map(|p| p.to_string_lossy().into_owned())
}

#[cfg(any(windows, feature = "mockwin"))]
mod win {
    use super::{exe_path, EXTENSIONS};
    #[cfg(feature = "mockwin")]
    use crate::mockwin as winreg;
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    const PROGID: &str = "PinkBourbon.Viewer.Image";
    const APP: &str = "Viewer";
    const CAPS: &str = "Software\\PinkBourbon\\Viewer";

    fn e(err: std::io::Error) -> String {
        format!("registry: {err}")
    }

    pub fn register() -> Result<String, String> {
        let exe = exe_path()?;
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let open_cmd = format!("\"{exe}\" \"%1\"");

        let (pid, _) = hkcu.create_subkey(format!("Software\\Classes\\{PROGID}")).map_err(e)?;
        pid.set_value("", &"Image (Viewer)").map_err(e)?;
        let (icon, _) = pid.create_subkey("DefaultIcon").map_err(e)?;
        icon.set_value("", &format!("\"{exe}\",0")).map_err(e)?;
        let (cmd, _) = pid.create_subkey("shell\\open\\command").map_err(e)?;
        cmd.set_value("", &open_cmd).map_err(e)?;

        let (app, _) = hkcu.create_subkey("Software\\Classes\\Applications\\viewer.exe").map_err(e)?;
        app.set_value("FriendlyAppName", &APP).map_err(e)?;
        let (acmd, _) = app.create_subkey("shell\\open\\command").map_err(e)?;
        acmd.set_value("", &open_cmd).map_err(e)?;
        let (types, _) = app.create_subkey("SupportedTypes").map_err(e)?;

        let (cap, _) = hkcu.create_subkey(format!("{CAPS}\\Capabilities")).map_err(e)?;
        cap.set_value("ApplicationName", &APP).map_err(e)?;
        cap.set_value("ApplicationDescription", &"Image viewer and painting study tool").map_err(e)?;
        let (fa, _) = cap.create_subkey("FileAssociations").map_err(e)?;

        for (ext, _) in EXTENSIONS {
            let dot = format!(".{ext}");
            let (k, _) = hkcu.create_subkey(format!("Software\\Classes\\{dot}\\OpenWithProgids")).map_err(e)?;
            k.set_value(PROGID, &"").map_err(e)?;
            fa.set_value(&dot, &PROGID).map_err(e)?;
            types.set_value(&dot, &"").map_err(e)?;
        }
        let (reg, _) = hkcu.create_subkey("Software\\RegisteredApplications").map_err(e)?;
        reg.set_value(APP, &format!("{CAPS}\\Capabilities")).map_err(e)?;

        // Ask Explorer to refresh associations, then open the Default apps page.
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "", "ms-settings:defaultapps?registeredAppUser=Viewer"])
            .spawn();
        Ok("Registered for your user. In the Settings page that just opened, click \"Set default\" \
            next to Viewer (Windows requires you to confirm this yourself)."
            .into())
    }

    pub fn unregister() -> Result<String, String> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let _ = hkcu.delete_subkey_all(format!("Software\\Classes\\{PROGID}"));
        let _ = hkcu.delete_subkey_all("Software\\Classes\\Applications\\viewer.exe");
        let _ = hkcu.delete_subkey_all(CAPS);
        if let Ok((reg, _)) = hkcu.create_subkey("Software\\RegisteredApplications") {
            let _ = reg.delete_value(APP);
        }
        for (ext, _) in EXTENSIONS {
            if let Ok((k, _)) = hkcu.create_subkey(format!("Software\\Classes\\.{ext}\\OpenWithProgids")) {
                let _ = k.delete_value(PROGID);
            }
        }
        Ok("Removed Viewer's file associations for your user.".into())
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod xdg {
    use super::{exe_path, EXTENSIONS};
    use std::process::Command;

    const DESKTOP: &str = "io.github.pink_bourbon_estate.Viewer.desktop";

    fn apps_dir() -> Result<std::path::PathBuf, String> {
        let base = directories::BaseDirs::new().ok_or("no home directory")?;
        Ok(base.data_dir().join("applications"))
    }

    pub fn register() -> Result<String, String> {
        let exe = exe_path()?;
        let dir = apps_dir()?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let mut mimes: Vec<&str> = EXTENSIONS.iter().map(|(_, m)| *m).collect();
        mimes.sort_unstable();
        mimes.dedup();
        let desktop = format!(
            "[Desktop Entry]\nType=Application\nName=Viewer\nGenericName=Image Viewer\n\
             Comment=Fast image viewer and painting study tool\nExec=\"{exe}\" %F\n\
             Terminal=false\nCategories=Graphics;Viewer;RasterGraphics;\n\
             Keywords=image;photo;viewer;painting;palette;\nMimeType={};\n",
            mimes.join(";")
        );
        std::fs::write(dir.join(DESKTOP), desktop).map_err(|e| e.to_string())?;
        let _ = Command::new("update-desktop-database").arg(&dir).status();
        let mut failed = Vec::new();
        for m in &mimes {
            let ok = Command::new("xdg-mime")
                .args(["default", DESKTOP, m])
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !ok {
                failed.push(*m);
            }
        }
        if failed.len() == mimes.len() {
            return Err("Wrote viewer.desktop, but `xdg-mime` was not found or failed. \
                        Install xdg-utils, then run `viewer --register` again."
                .into());
        }
        Ok(format!("Viewer is now the default for {} image types.", mimes.len() - failed.len()))
    }

    pub fn unregister() -> Result<String, String> {
        let dir = apps_dir()?;
        let _ = std::fs::remove_file(dir.join(DESKTOP));
        let _ = Command::new("update-desktop-database").arg(&dir).status();
        Ok("Removed the Viewer desktop entry. Pick another default viewer in your desktop settings.".into())
    }
}

pub fn register() -> Result<String, String> {
    #[cfg(windows)]
    {
        win::register()
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        xdg::register()
    }
    #[cfg(target_os = "macos")]
    {
        Err("On macOS use Finder → Get Info → Open with → Change All.".into())
    }
}

pub fn unregister() -> Result<String, String> {
    #[cfg(windows)]
    {
        win::unregister()
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        xdg::unregister()
    }
    #[cfg(target_os = "macos")]
    {
        Err("Nothing to undo on macOS.".into())
    }
}

/// Reveal a file in the system file manager.
pub fn reveal(path: &std::path::Path) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer").arg(format!("/select,{}", path.display())).spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg("-R").arg(path).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let dir = path.parent().unwrap_or(path);
        let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
    }
}

/// Open a file with the system's default handler (used for palette.json).
pub fn open_external(path: &std::path::Path) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("cmd").args(["/C", "start", ""]).arg(path).spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(path).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(path).spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detects_images() {
        assert!(is_image(std::path::Path::new("a/b/C.JPG")));
        assert!(!is_image(std::path::Path::new("a/b/notes.txt")));
        assert!(!is_image(std::path::Path::new("noext")));
    }
}
