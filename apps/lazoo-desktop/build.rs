fn main() {
    let icon_path = std::path::PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"),
    )
    .join("icons")
    .join("icon.ico");
    let icon = lazoo_icon();
    if std::fs::read(&icon_path).ok().as_deref() != Some(icon.as_slice()) {
        std::fs::create_dir_all(icon_path.parent().expect("icon parent"))
            .expect("generated icon directory must be writable");
        std::fs::write(&icon_path, icon).expect("generated Lazoo icon must be writable");
    }
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .windows_attributes(tauri_build::WindowsAttributes::new().window_icon_path(icon_path))
            .app_manifest(tauri_build::AppManifest::new().commands(&[
                "daemon_call",
                "events_subscribe",
                "events_unsubscribe",
                "terminal_stream_start",
                "terminal_stream_stop",
                "terminal_stream_focus",
                "terminal_write",
                "voice_state",
                "voice_settings_set",
                "voice_key_set",
                "voice_key_clear",
                "voice_whisper_install",
                "voice_whisper_remove",
                "voice_transcribe",
                "voice_speak",
                "voice_insert_text",
                "voice_copy_text",
                "voice_overlay",
                "voice_open_microphone_settings",
            ])),
    )
    .expect("Tauri build configuration must be valid");
}

/// The app icon: the sidebar wordmark's mark (a 14-unit iris checkerboard with a solid 6-unit
/// core, `.wordmark__mark` in `ui/src/styles/app.css`) on a slate rounded square, as a
/// multi-size ICO. Each size scales the mark by whole pixels per unit so the dither stays crisp.
// Pixel coordinates and colour channels are tiny (<= 256), so the float casts are exact.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn lazoo_icon() -> Vec<u8> {
    // (size in px, pixels per mark unit)
    const SIZES: [(u32, u32); 7] = [
        (16, 1),
        (24, 1),
        (32, 2),
        (48, 2),
        (64, 3),
        (128, 6),
        (256, 11),
    ];
    const SLATE: [f32; 3] = [23.0, 28.0, 36.0]; // --lz-slate-100
    const IRIS: [f32; 3] = [141.0, 154.0, 244.0]; // --lz-iris-500
    const CORE: [f32; 3] = [168.0, 179.0, 249.0]; // --lz-iris-400

    let mix = |a: [f32; 3], b: [f32; 3], t: f32| -> [f32; 3] {
        [0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * t)
    };
    let images: Vec<Vec<u8>> = SIZES
        .iter()
        .map(|&(size, unit)| {
            let side = size as f32;
            let radius = (side * 0.22).max(2.0);
            let mark = 14 * unit;
            let origin = (size - mark) / 2;
            let mut pixels = Vec::with_capacity((size * size * 4) as usize);
            // BMP rows run bottom-up.
            for y in (0..size).rev() {
                for x in 0..size {
                    // Rounded-square coverage from a 4x4 supersample, for smooth corners.
                    let mut covered = 0_u32;
                    for sy in 0..4 {
                        for sx in 0..4 {
                            let px = x as f32 + (sx as f32 + 0.5) / 4.0;
                            let py = y as f32 + (sy as f32 + 0.5) / 4.0;
                            let dx = (radius - px).max(px - (side - radius)).max(0.0);
                            let dy = (radius - py).max(py - (side - radius)).max(0.0);
                            if dx * dx + dy * dy <= radius * radius {
                                covered += 1;
                            }
                        }
                    }
                    let inside = |v: u32| v >= origin && v < origin + mark;
                    let colour = if inside(x) && inside(y) {
                        let (ux, uy) = ((x - origin) / unit, (y - origin) / unit);
                        if (4..10).contains(&ux) && (4..10).contains(&uy) {
                            CORE
                        } else if (ux + uy) % 2 == 0 {
                            mix(SLATE, IRIS, 0.55)
                        } else {
                            mix(SLATE, IRIS, 0.18)
                        }
                    } else {
                        SLATE
                    };
                    let [r, g, b] = colour.map(|c| c.round() as u8);
                    let alpha = (covered * 255 / 16) as u8;
                    pixels.extend_from_slice(&[b, g, r, alpha]);
                }
            }
            let mask_row = (size.div_ceil(32) * 4) as usize;
            let mut image = Vec::new();
            image.extend_from_slice(&40_u32.to_le_bytes());
            image.extend_from_slice(&size.to_le_bytes());
            image.extend_from_slice(&(size * 2).to_le_bytes());
            image.extend_from_slice(&1_u16.to_le_bytes());
            image.extend_from_slice(&32_u16.to_le_bytes());
            image.extend_from_slice(&[0; 4]);
            image.extend_from_slice(
                &u32::try_from(pixels.len() + mask_row * size as usize)
                    .unwrap()
                    .to_le_bytes(),
            );
            image.extend_from_slice(&[0; 16]);
            image.extend_from_slice(&pixels);
            image.resize(image.len() + mask_row * size as usize, 0);
            image
        })
        .collect();

    let mut icon = Vec::new();
    icon.extend_from_slice(&0_u16.to_le_bytes());
    icon.extend_from_slice(&1_u16.to_le_bytes());
    icon.extend_from_slice(&u16::try_from(images.len()).unwrap().to_le_bytes());
    let mut offset = 6 + 16 * images.len();
    for (&(size, _), image) in SIZES.iter().zip(&images) {
        let byte = u8::try_from(size).unwrap_or(0); // 256 is written as 0
        icon.extend_from_slice(&[byte, byte, 0, 0]);
        icon.extend_from_slice(&1_u16.to_le_bytes());
        icon.extend_from_slice(&32_u16.to_le_bytes());
        icon.extend_from_slice(&u32::try_from(image.len()).unwrap().to_le_bytes());
        icon.extend_from_slice(&u32::try_from(offset).unwrap().to_le_bytes());
        offset += image.len();
    }
    for image in images {
        icon.extend_from_slice(&image);
    }
    icon
}
