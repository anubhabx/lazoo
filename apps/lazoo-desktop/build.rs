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
                "workflows_load",
                "workflows_save",
            ])),
    )
    .expect("Tauri build configuration must be valid");
}

/// The app icon: the Lazoo brand mark (`design-system/brand/lazoo-mark.svg`) as a multi-size ICO.
/// On a 16-unit grid a `#171c24` tile with 3-unit rounded corners, an `#e8ecf1` "L" (rects
/// 3,3,3,10 and 3,10,6,3) and an `#8d9af4` cursor block (rect 10,5,3,8). Each size scales the
/// grid by nearest pixel (rect edges rounded to whole pixels); only the tile corners are
/// antialiased.
// Pixel coordinates and colour channels are tiny (<= 256), so the float casts are exact.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn lazoo_icon() -> Vec<u8> {
    const SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];
    const TILE: [u8; 3] = [0x17, 0x1c, 0x24];
    const LETTER: [u8; 3] = [0xe8, 0xec, 0xf1];
    const CURSOR: [u8; 3] = [0x8d, 0x9a, 0xf4];
    // (x, y, width, height) in grid units, with the colour they paint.
    const RECTS: [([u32; 4], [u8; 3]); 3] = [
        ([3, 3, 3, 10], LETTER),
        ([3, 10, 6, 3], LETTER),
        ([10, 5, 3, 8], CURSOR),
    ];

    let images: Vec<Vec<u8>> = SIZES
        .iter()
        .map(|&size| {
            let side = size as f32;
            let radius = side * 3.0 / 16.0;
            // Grid unit to the nearest pixel edge.
            let edge = |unit: u32| (unit * size + 8) / 16;
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
                    let mut colour = TILE;
                    for ([rx, ry, rw, rh], paint) in RECTS {
                        if (edge(rx)..edge(rx + rw)).contains(&x)
                            && (edge(ry)..edge(ry + rh)).contains(&y)
                        {
                            colour = paint;
                        }
                    }
                    let [r, g, b] = colour;
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
    for (&size, image) in SIZES.iter().zip(&images) {
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
