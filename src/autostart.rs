use std::path::PathBuf;

fn exe() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|e| e.to_string())
}

pub fn set_enabled(enabled: bool, minimized: bool) -> Result<(), String> {
    if enabled {
        enable(minimized)
    } else {
        disable()
    }
}

#[cfg(target_os = "windows")]
fn enable(minimized: bool) -> Result<(), String> {
    let path = exe()?;
    let mut command = format!("\"{}\"", path.display());
    if minimized {
        command.push_str(" --minimized");
    }
    let status = std::process::Command::new("reg")
        .args([
            "add",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
            "/v",
            "Rask",
            "/t",
            "REG_SZ",
            "/d",
            &command,
            "/f",
        ])
        .status()
        .map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err("could not write the Windows startup entry".into())
    }
}

#[cfg(target_os = "windows")]
fn disable() -> Result<(), String> {
    let _ = std::process::Command::new("reg")
        .args([
            "delete",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
            "/v",
            "Rask",
            "/f",
        ])
        .status();
    Ok(())
}

#[cfg(target_os = "macos")]
fn enable(minimized: bool) -> Result<(), String> {
    let path = exe()?;
    let dir = dirs::home_dir()
        .ok_or("no home directory")?
        .join("Library/LaunchAgents");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let args = if minimized {
        format!(
            "\n    <string>{}</string>\n    <string>--minimized</string>",
            path.display()
        )
    } else {
        format!("\n    <string>{}</string>", path.display())
    };
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>com.abb0r.rask</string>
  <key>RunAtLoad</key><true/>
  <key>ProgramArguments</key><array>{args}
  </array>
</dict></plist>
"#
    );
    std::fs::write(dir.join("com.abb0r.rask.plist"), plist).map_err(|e| e.to_string())
}

#[cfg(target_os = "macos")]
fn disable() -> Result<(), String> {
    if let Some(home) = dirs::home_dir() {
        let path = home.join("Library/LaunchAgents/com.abb0r.rask.plist");
        let _ = std::fs::remove_file(path);
    }
    Ok(())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn enable(minimized: bool) -> Result<(), String> {
    let path = exe()?;
    let dir = dirs::config_dir()
        .ok_or("no config directory")?
        .join("autostart");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let exec = if minimized {
        format!("\"{}\" --minimized", path.display())
    } else {
        format!("\"{}\"", path.display())
    };
    let desktop = format!(
        "[Desktop Entry]\nType=Application\nName=Rask\nComment=Quiet chat for Ollama\nExec={exec}\nTerminal=false\nCategories=Utility;\nX-GNOME-Autostart-enabled=true\n"
    );
    std::fs::write(dir.join("rask.desktop"), desktop).map_err(|e| e.to_string())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn disable() -> Result<(), String> {
    if let Some(dir) = dirs::config_dir() {
        let _ = std::fs::remove_file(dir.join("autostart/rask.desktop"));
    }
    Ok(())
}
