//! Window-level pieces of voice: the dictation overlay window and the microphone permission.
use tauri::{AppHandle, PhysicalPosition, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

pub const OVERLAY: &str = "voice-overlay";
const WIDTH: f64 = 280.0;
const HEIGHT: f64 = 56.0;
const BOTTOM_MARGIN: f64 = 64.0;

/// Creates the overlay hidden. It loads the same front end, which renders only the overlay for
/// this label. `focusable(false)` makes Windows treat it as `WS_EX_NOACTIVATE`, so showing it
/// never takes the keyboard focus away from the application being dictated into.
pub fn create_overlay(app: &AppHandle) -> tauri::Result<()> {
    WebviewWindowBuilder::new(app, OVERLAY, WebviewUrl::App("index.html".into()))
        .title("Lazoo Voice")
        .inner_size(WIDTH, HEIGHT)
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .focusable(false)
        .focused(false)
        .resizable(false)
        .shadow(false)
        .visible(false)
        .build()?;
    Ok(())
}

/// Bottom-centre of the primary monitor's work area, 64 logical px above its bottom edge.
pub fn place(app: &AppHandle, window: &WebviewWindow) {
    let Ok(Some(monitor)) = app.primary_monitor() else {
        return;
    };
    let work = monitor.work_area();
    let scale = monitor.scale_factor();
    let (width, height) = ((WIDTH * scale).round(), (HEIGHT * scale).round());
    let x = f64::from(work.position.x) + (f64::from(work.size.width) - width) / 2.0;
    let y =
        f64::from(work.position.y) + f64::from(work.size.height) - height - BOTTOM_MARGIN * scale;
    #[allow(clippy::cast_possible_truncation)] // Screen coordinates are far inside i32.
    let position = PhysicalPosition::new(x.round() as i32, y.round() as i32);
    let _ = window.set_position(position);
}

/// Allows microphone requests from the app's own origin in `window` without a prompt.
///
/// wry (0.55) registers a `WebView2` `PermissionRequested` handler only for clipboard reads, so a
/// microphone request falls through to `WebView2`'s default: a browser-style prompt. That prompt
/// is not silent but is wrong for push-to-talk (it can appear over another application), so the
/// app's own origin is allowed here. Other origins and permissions keep `WebView2`'s default.
#[cfg(windows)]
pub fn allow_microphone(window: &WebviewWindow) {
    use webview2_com::{
        Microsoft::Web::WebView2::Win32::{
            COREWEBVIEW2_PERMISSION_KIND, COREWEBVIEW2_PERMISSION_KIND_MICROPHONE,
            COREWEBVIEW2_PERMISSION_STATE_ALLOW,
        },
        PermissionRequestedEventHandler, take_pwstr,
    };

    let _ = window.with_webview(|webview| {
        let handler = PermissionRequestedEventHandler::create(Box::new(|_, args| {
            let Some(args) = args else { return Ok(()) };
            let mut kind = COREWEBVIEW2_PERMISSION_KIND::default();
            // The PWSTR type belongs to the windows 0.61 that webview2-com uses, so it cannot be named.
            #[allow(clippy::default_trait_access)]
            let mut uri = Default::default();
            // SAFETY: both out-parameters are valid for the duration of the calls and the
            // returned string is owned (and freed) by `take_pwstr`.
            let origin = unsafe {
                args.PermissionKind(&raw mut kind)?;
                args.Uri(&raw mut uri)?;
                take_pwstr(uri)
            };
            if kind == COREWEBVIEW2_PERMISSION_KIND_MICROPHONE && is_own_origin(&origin) {
                unsafe { args.SetState(COREWEBVIEW2_PERMISSION_STATE_ALLOW)? };
            }
            Ok(())
        }));
        let mut token = 0_i64;
        // SAFETY: runs on the UI thread that owns the WebView2; the handler outlives the call.
        unsafe {
            if let Ok(core) = webview.controller().CoreWebView2() {
                let _ = core.add_PermissionRequested(&handler, &raw mut token);
            }
        }
    });
}

/// True when Windows privacy settings deny the microphone: "Microphone access" for the device
/// (HKLM) or this user (HKCU), or "Let desktop apps access your microphone" (`NonPackaged`).
/// Capture then fails with a device error that would otherwise read as a broken microphone.
/// Lazoo never changes these settings; it only says which one is off.
#[cfg(windows)]
pub fn microphone_blocked() -> bool {
    use windows::{
        Win32::System::Registry::{
            HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW,
        },
        core::{PCWSTR, w},
    };
    const STORE: &str = r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone";
    let denied = |root: HKEY, subkey: &str| {
        let subkey: Vec<u16> = subkey.encode_utf16().chain([0]).collect();
        let mut value = [0_u16; 16];
        let mut bytes = u32::try_from(size_of_val(&value)).unwrap_or(0);
        // SAFETY: the subkey is NUL-terminated and the buffer and its byte count are valid.
        let status = unsafe {
            RegGetValueW(
                root,
                PCWSTR(subkey.as_ptr()),
                w!("Value"),
                RRF_RT_REG_SZ,
                None,
                Some(value.as_mut_ptr().cast()),
                Some(&raw mut bytes),
            )
        };
        let len = (bytes as usize / 2).saturating_sub(1).min(value.len());
        status.is_ok() && String::from_utf16_lossy(&value[..len]).eq_ignore_ascii_case("Deny")
    };
    denied(HKEY_LOCAL_MACHINE, STORE)
        || denied(HKEY_CURRENT_USER, STORE)
        || denied(HKEY_CURRENT_USER, &format!(r"{STORE}\NonPackaged"))
}

/// Opens Windows Settings at Privacy & security > Microphone for the user to decide.
pub fn open_microphone_settings() -> std::io::Result<()> {
    std::process::Command::new("explorer.exe")
        .arg("ms-settings:privacy-microphone")
        .spawn()
        .map(drop)
}

/// The packaged app is served from `http(s)://tauri.localhost`; the dev server from the dev URL.
fn is_own_origin(uri: &str) -> bool {
    let own = ["http://tauri.localhost/", "https://tauri.localhost/"];
    let dev = cfg!(debug_assertions) && uri.starts_with("http://127.0.0.1:1420/");
    dev || own.iter().any(|prefix| uri.starts_with(prefix)) || uri == "http://tauri.localhost"
}

#[cfg(test)]
mod tests {
    use super::is_own_origin;

    #[test]
    fn only_the_apps_own_origin_counts() {
        assert!(is_own_origin("http://tauri.localhost/"));
        assert!(is_own_origin("http://tauri.localhost/index.html"));
        assert!(is_own_origin("https://tauri.localhost/"));
        assert!(!is_own_origin("https://tauri.localhost.evil.com/"));
        assert!(!is_own_origin("http://evil.com/"));
        assert!(!is_own_origin(
            "https://example.com/http://tauri.localhost/"
        ));
        assert!(!is_own_origin(""));
    }
}
