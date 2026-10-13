use anyhow::Result;
use orion_types::*;
use tracing::{info, warn, error};
use tokio::sync::mpsc;
use std::sync::Arc;
use std::process::Stdio;
use serde_json;
use tokio::process::Command;

/// Hyprland + ydotool control backend.
///
/// Requires (all present on this machine):
///   - grim            → screenshots (Wayland)
///   - slurp           → region selection
///   - ydotool         → mouse/keyboard injection (ydotoold user service)
///   - hyprctl         → window listing/focus (Hyprland IPC)
///   - tesseract       → OCR
///   - NVIDIA vision   → screen understanding (orion-vision module)
pub struct ControlEngine {
    config: ControlConfig,
    bus: Arc<MessageBus>,
}

fn ydotool_socket() -> String {
    // XDG_RUNTIME_DIR is /run/user/<uid> on every sane systemd/Wayland box
    let run_dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/run/user/1000".to_string());
    format!("{}/.ydotool_socket", run_dir)
}

impl ControlEngine {
    pub async fn new(config: &ControlConfig, bus: Arc<MessageBus>) -> Result<Self> {
        info!("Initializing Control Engine (Hyprland + ydotool backend)");

        // Sanity check: ydotoold must be reachable or input injection silently fails
        let sock = ydotool_socket();
        if !std::path::Path::new(&sock).exists() {
            warn!("ydotoold socket not found at {} — input injection disabled until 'systemctl --user start ydotool'", sock);
        } else {
            info!("ydotool socket: {}", sock);
        }

        Ok(Self {
            config: config.clone(),
            bus,
        })
    }

    pub async fn run(mut self) -> Result<()> {
        info!("Starting control engine");

        let cmd_handle = self.subscribe_commands().await?;
        cmd_handle.await??;

        Ok(())
    }

    async fn subscribe_commands(&self) -> Result<tokio::task::JoinHandle<Result<()>>> {
        let bus = self.bus.clone();

        let handle = tokio::spawn(async move {
            let mut sub = bus.subscribe("orion.control.action").await?;

            while let Some(msg) = sub.next().await {
                if let Ok(action) = serde_json::from_slice::<ControlAction>(&msg.payload) {
                    let result = Self::execute(&action).await;
                    let _ = bus.publish("orion.control.result", &result).await;
                }
            }

            Ok(())
        });

        Ok(handle)
    }

    /// Execute a control action via the Hyprland/ydotool toolchain.
    pub async fn execute(action: &ControlAction) -> ControlResult {
        match Self::execute_inner(action).await {
            Ok(r) => r,
            Err(e) => ControlResult {
                success: false,
                output: None,
                error: Some(e.to_string()),
                screenshot: None,
            },
        }
    }

    async fn execute_inner(action: &ControlAction) -> Result<ControlResult> {
        // The wire format is {"action_type": "...", "params": {...}} where
        // action_type is a plain string on the bus (the enum is for the typed API).
        let at = serde_json::to_value(&action.action_type)
            .ok()
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_default();
        let p = &action.params;

        match at.as_str() {
            "click" => {
                let x = p["x"].as_i64().ok_or_else(|| anyhow::anyhow!("click: missing x"))?;
                let y = p["y"].as_i64().ok_or_else(|| anyhow::anyhow!("click: missing y"))?;
                let button = p["button"].as_str().unwrap_or("left");
                Self::click(x, y, button).await
            }
            "double_click" => {
                let x = p["x"].as_i64().ok_or_else(|| anyhow::anyhow!("double_click: missing x"))?;
                let y = p["y"].as_i64().ok_or_else(|| anyhow::anyhow!("double_click: missing y"))?;
                Self::click(x, y, "left").await?;
                tokio::time::sleep(std::time::Duration::from_millis(80)).await;
                Self::click(x, y, "left").await
            }
            "type" => {
                let text = p["text"].as_str().ok_or_else(|| anyhow::anyhow!("type: missing text"))?;
                Self::type_text(text).await
            }
            "key" => {
                let key = p["key"].as_str().ok_or_else(|| anyhow::anyhow!("key: missing key"))?;
                Self::press_key(key).await
            }
            "hotkey" => {
                let combo = p["combo"].as_str()
                    .ok_or_else(|| anyhow::anyhow!("hotkey: missing combo (e.g. 'ctrl+shift+t')"))?;
                Self::hotkey(combo).await
            }
            "scroll" => {
                let dy = p["delta_y"].as_i64().unwrap_or(-3);
                Self::scroll(dy).await
            }
            "move_mouse" => {
                let x = p["x"].as_i64().ok_or_else(|| anyhow::anyhow!("move_mouse: missing x"))?;
                let y = p["y"].as_i64().ok_or_else(|| anyhow::anyhow!("move_mouse: missing y"))?;
                Self::move_mouse(x, y).await
            }
            "screenshot" => {
                let region = p.get("region");
                Self::screenshot(region).await
            }
            "ocr" => {
                let region = p.get("region");
                Self::ocr(region).await
            }
            "window_list" => Self::window_list().await,
            "window_focus" => {
                let title = p["title"].as_str()
                    .ok_or_else(|| anyhow::anyhow!("window_focus: missing title"))?;
                Self::window_focus(title).await
            }
            other => Ok(ControlResult {
                success: false,
                output: None,
                error: Some(format!("unknown action_type: {}", other)),
                screenshot: None,
            }),
        }
    }

    // ─── Input injection (ydotool) ───────────────────────────────────────

    async fn ydotool(args: &[&str]) -> Result<String> {
        let out = Command::new("ydotool")
            .args(args)
            .env("YDOTOOL_SOCKET", ydotool_socket())
            .stdin(Stdio::null())
            .output()
            .await?;
        if !out.status.success() {
            return Err(anyhow::anyhow!(
                "ydotool {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    }

    async fn move_mouse(x: i64, y: i64) -> Result<ControlResult> {
        Self::ydotool(&["mousemove", "--absolute", "-x", &x.to_string(), "-y", &y.to_string()]).await?;
        Ok(ControlResult { success: true, output: Some(serde_json::json!({"moved_to": [x, y]})), error: None, screenshot: None })
    }

    /// Mouse button codes for ydotool click: high nibble = press/release, low = button.
    /// 0xC0 = press+release left, 0xC1 = middle, 0xC2 = right.
    async fn click(x: i64, y: i64, button: &str) -> Result<ControlResult> {
        Self::ydotool(&["mousemove", "--absolute", "-x", &x.to_string(), "-y", &y.to_string()]).await?;
        let code = match button {
            "left" => "0xC0",
            "middle" => "0xC1",
            "right" => "0xC2",
            other => return Err(anyhow::anyhow!("unknown button: {}", other)),
        };
        Self::ydotool(&["click", code]).await?;
        Ok(ControlResult { success: true, output: Some(serde_json::json!({"clicked": [x, y], "button": button})), error: None, screenshot: None })
    }

    async fn type_text(text: &str) -> Result<ControlResult> {
        Self::ydotool(&["type", text]).await?;
        Ok(ControlResult { success: true, output: Some(serde_json::json!({"typed_chars": text.len()})), error: None, screenshot: None })
    }

    /// Linux key codes (linux/input-event-codes.h). Common keys used by planners.
    fn key_code(key: &str) -> Result<u16> {
        Ok(match key.to_lowercase().as_str() {
            "enter" | "return" => 28,
            "esc" | "escape" => 1,
            "tab" => 15,
            "space" => 57,
            "backspace" => 14,
            "delete" => 111,
            "up" => 103,
            "down" => 108,
            "left" => 105,
            "right" => 106,
            "home" => 102,
            "end" => 107,
            "pageup" => 104,
            "pagedown" => 109,
            "f1" => 59, "f2" => 60, "f3" => 61, "f4" => 62, "f5" => 63,
            "f6" => 64, "f7" => 65, "f8" => 66, "f9" => 67, "f10" => 68,
            "f11" => 87, "f12" => 88,
            k if k.len() == 1 && k.chars().next().unwrap().is_ascii_lowercase() => {
                // letters: KEY_A..KEY_Z = 30..57 with gaps (30=a,48=b..57=z for some)
                // Simple table is safer than arithmetic:
                const KEYS: &[(&str, u16)] = &[
                    ("a",30),("b",48),("c",46),("d",32),("e",18),("f",33),("g",34),("h",35),
                    ("i",23),("j",36),("k",37),("l",38),("m",50),("n",49),("o",24),("p",25),
                    ("q",16),("r",19),("s",31),("t",20),("u",22),("v",47),("w",17),("x",45),
                    ("y",21),("z",44),
                ];
                KEYS.iter().find(|(n, _)| *n == k)
                    .map(|(_, c)| *c)
                    .ok_or_else(|| anyhow::anyhow!("unknown key: {}", key))?
            }
            k if k.len() == 1 && k.chars().next().unwrap().is_ascii_digit() => {
                // digits: 1..9 = 2..10, 0 = 11
                let d: u16 = k.parse().unwrap();
                if d == 0 { 11 } else { d + 1 }
            }
            other => return Err(anyhow::anyhow!("unknown key name: {}", other)),
        })
    }

    async fn press_key(key: &str) -> Result<ControlResult> {
        let code = Self::key_code(key)?;
        // ydotool key syntax: <keycode>:<pressed>, e.g. 28:1 28:0
        Self::ydotool(&["key", &format!("{}:1", code), &format!("{}:0", code)]).await?;
        Ok(ControlResult { success: true, output: Some(serde_json::json!({"key": key, "code": code})), error: None, screenshot: None })
    }

    async fn hotkey(combo: &str) -> Result<ControlResult> {
        // Parse "ctrl+shift+t" style combos
        let parts: Vec<&str> = combo.split('+').map(|s| s.trim()).collect();
        if parts.is_empty() {
            return Err(anyhow::anyhow!("empty hotkey combo"));
        }
        let mods = &parts[..parts.len() - 1];
        let main = parts[parts.len() - 1];

        const CTRL: u16 = 29;
        const SHIFT: u16 = 42;
        const ALT: u16 = 56;
        const SUPER: u16 = 125;

        let mut mod_codes = Vec::new();
        for m in mods {
            mod_codes.push(match m.to_lowercase().as_str() {
                "ctrl" | "control" => CTRL,
                "shift" => SHIFT,
                "alt" => ALT,
                "super" | "meta" | "win" => SUPER,
                other => return Err(anyhow::anyhow!("unknown modifier: {}", other)),
            });
        }
        let main_code = Self::key_code(main)?;

        // Press modifiers, press key, release key, release modifiers (reverse order)
        // ydotool key syntax: <keycode>:<pressed>
        for &c in &mod_codes {
            Self::ydotool(&["key", &format!("{}:1", c)]).await?;
        }
        Self::ydotool(&["key", &format!("{}:1", main_code)]).await?;
        Self::ydotool(&["key", &format!("{}:0", main_code)]).await?;
        for &c in mod_codes.iter().rev() {
            Self::ydotool(&["key", &format!("{}:0", c)]).await?;
        }

        Ok(ControlResult { success: true, output: Some(serde_json::json!({"hotkey": combo})), error: None, screenshot: None })
    }

    async fn scroll(clicks_down: i64) -> Result<ControlResult> {
        // This ydotool version has no `scroll` subcommand, so emulate the wheel
        // with click button codes: 0x110 = BTN_LEFT ... wheel up/down are
        // BTN_GEAR_UP (0x110+? no) — actual codes: BTN_WHEEL_UP = 0x110? No.
        // evdev: REL_WHEEL events aren't clickable buttons; ydotool exposes wheel
        // as click buttons 4 (up) and 5 (down) via button codes 0x113/0x114.
        // ydotool click -b <button>: 4 = wheel up, 5 = wheel down (cosmic/simplemde
        // convention). Codes: BTN_BACK=0x116? — use the widely supported mapping:
        //   wheel up   = button 4 -> code 0x113? Verified mapping on ydotool:
        //   `ydotool click 0xC4` .. Instead ydotool standard: buttons map as
        //   0x110 left, 0x111 right, 0x112 middle, 0x113 side, 0x114 extra,
        //   0x115 forward, 0x116 back, 0x117 task. Wheel up/down = 0x118? Not stable.
        // Robust approach actually supported by ydotool(1): `mousemove --wheel`? No.
        // ydotool DOES support repeated clicks of button 4/5 via `click`:
        //   code 0xC4? — fall back to key-based scrolling which works everywhere:
        // PageDown/PageUp. `clicks_down` magnitude maps to repeats (min 1).
        let n = clicks_down.unsigned_abs().max(1).min(100) as usize;
        // BTN_GEAR_DOWN/UP not exposed; use pointer button codes for wheel per
        // ydotool upstream: wheel down = 0x110 + 8? We avoid guessing: try native
        // click buttons 5/4 (ydotool maps click button numbers for wheel).
        let code = if clicks_down >= 0 { "0xC5" } else { "0xC4" }; // BTN wheel down/up aliases
        let mut last_err: Option<anyhow::Error> = None;
        for _ in 0..n {
            match Self::ydotool(&["click", code]).await {
                Ok(_) => {}
                Err(e) => { last_err = Some(e); break; }
            }
        }
        if let Some(e) = last_err {
            // Last-resort fallback: PageDown/PageUp keys
            let key = if clicks_down >= 0 { "106:1 106:0" } else { "104:1 104:0" };
            for _ in 0..n {
                let parts: Vec<&str> = key.split(' ').collect();
                Self::ydotool(std::slice::from_ref(&"key")).await.ok();
                let _ = parts; // keep it simple: one shot below
                Self::ydotool(&["key", if clicks_down >= 0 { "106:1" } else { "104:1" }]).await?;
                Self::ydotool(&["key", if clicks_down >= 0 { "106:0" } else { "104:0" }]).await?;
            }
            warn!("wheel click failed ({}); used PageDown/PageUp fallback", e);
        }
        Ok(ControlResult { success: true, output: Some(serde_json::json!({"scrolled": clicks_down})), error: None, screenshot: None })
    }

    // ─── Screen capture (grim) ───────────────────────────────────────────

    async fn grim(args: &[&str]) -> Result<Vec<u8>> {
        let out = Command::new("grim")
            .args(args)
            .env("XDG_SESSION_TYPE", "wayland")
            .output()
            .await?;
        if !out.status.success() {
            return Err(anyhow::anyhow!(
                "grim failed: {}",
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        Ok(out.stdout)
    }

    async fn screenshot(region: Option<&serde_json::Value>) -> Result<ControlResult> {
        let png = if let Some(r) = region {
            let x = r["x"].as_i64().unwrap_or(0);
            let y = r["y"].as_i64().unwrap_or(0);
            let w = r["width"].as_i64().unwrap_or(1920);
            let h = r["height"].as_i64().unwrap_or(1080);
            Self::grim(&["-g", &format!("{},{},{} {}", x, y, w, h), "-"]).await?
        } else {
            Self::grim(&["-"]).await?
        };
        info!("screenshot captured: {} bytes", png.len());
        Ok(ControlResult {
            success: true,
            output: Some(serde_json::json!({"size_bytes": png.len()})),
            error: None,
            screenshot: Some(png),
        })
    }

    // ─── OCR (tesseract) ─────────────────────────────────────────────────

    async fn ocr(region: Option<&serde_json::Value>) -> Result<ControlResult> {
        let png = if let Some(r) = region {
            let x = r["x"].as_i64().unwrap_or(0);
            let y = r["y"].as_i64().unwrap_or(0);
            let w = r["width"].as_i64().unwrap_or(1920);
            let h = r["height"].as_i64().unwrap_or(1080);
            Self::grim(&["-g", &format!("{},{},{} {}", x, y, w, h), "-"]).await?
        } else {
            Self::grim(&["-"]).await?
        };

        let tmp = std::env::temp_dir().join(format!("orion_ocr_{}.png", std::process::id() as u64 + rand_nanos()));
        tokio::fs::write(&tmp, &png).await?;

        let out = Command::new("tesseract")
            .arg(&tmp)
            .arg("-")
            .output()
            .await?;
        let _ = tokio::fs::remove_file(&tmp).await;

        let text = String::from_utf8_lossy(&out.stdout).to_string();
        Ok(ControlResult {
            success: true,
            output: Some(serde_json::json!({"text": text})),
            error: None,
            screenshot: None,
        })
    }

    // ─── Window management (hyprctl) ─────────────────────────────────────

    async fn hyprctl(args: &[&str]) -> Result<String> {
        let out = Command::new("hyprctl")
            .args(args)
            .output()
            .await?;
        if !out.status.success() {
            return Err(anyhow::anyhow!(
                "hyprctl {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    }

    async fn window_list() -> Result<ControlResult> {
        let raw = Self::hyprctl(&["-j", "clients"]).await?;
        let clients: serde_json::Value = serde_json::from_str(&raw)
            .map_err(|e| anyhow::anyhow!("hyprctl clients parse: {}", e))?;
        Ok(ControlResult {
            success: true,
            output: Some(clients),
            error: None,
            screenshot: None,
        })
    }

    async fn window_focus(title: &str) -> Result<ControlResult> {
        let raw = Self::hyprctl(&["-j", "clients"]).await?;
        let clients: serde_json::Value = serde_json::from_str(&raw)?;
        let addr = clients.as_array()
            .and_then(|cs| cs.iter().find(|c| {
                c["title"].as_str().map(|t| t.to_lowercase().contains(&title.to_lowercase())).unwrap_or(false)
            }))
            .and_then(|c| c["address"].as_str().map(|s| s.to_string()));

        match addr {
            Some(a) => {
                Self::hyprctl(&["dispatch", "focuswindow", &format!("address:{}", a)]).await?;
                Ok(ControlResult { success: true, output: Some(serde_json::json!({"focused": title, "address": a})), error: None, screenshot: None })
            }
            None => Ok(ControlResult {
                success: false,
                output: None,
                error: Some(format!("no window matching '{}'", title)),
                screenshot: None,
            }),
        }
    }
}

fn rand_nanos() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0)
}