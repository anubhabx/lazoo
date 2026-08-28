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
            .app_manifest(tauri_build::AppManifest::new().commands(&["load_desktop_snapshot"])),
    )
    .expect("Tauri build configuration must be valid");
}

fn lazoo_icon() -> Vec<u8> {
    const SIDE: u32 = 32;
    const PIXEL_BYTES: u32 = SIDE * SIDE * 4;
    const MASK_BYTES: u32 = SIDE * 4;
    const IMAGE_BYTES: u32 = 40 + PIXEL_BYTES + MASK_BYTES;
    let mut icon = Vec::with_capacity((22 + IMAGE_BYTES) as usize);

    icon.extend_from_slice(&0_u16.to_le_bytes());
    icon.extend_from_slice(&1_u16.to_le_bytes());
    icon.extend_from_slice(&1_u16.to_le_bytes());
    icon.extend_from_slice(&[SIDE as u8, SIDE as u8, 0, 0]);
    icon.extend_from_slice(&1_u16.to_le_bytes());
    icon.extend_from_slice(&32_u16.to_le_bytes());
    icon.extend_from_slice(&IMAGE_BYTES.to_le_bytes());
    icon.extend_from_slice(&22_u32.to_le_bytes());

    icon.extend_from_slice(&40_u32.to_le_bytes());
    icon.extend_from_slice(&(SIDE as i32).to_le_bytes());
    icon.extend_from_slice(&((SIDE * 2) as i32).to_le_bytes());
    icon.extend_from_slice(&1_u16.to_le_bytes());
    icon.extend_from_slice(&32_u16.to_le_bytes());
    icon.extend_from_slice(&0_u32.to_le_bytes());
    icon.extend_from_slice(&(PIXEL_BYTES + MASK_BYTES).to_le_bytes());
    icon.extend_from_slice(&0_i32.to_le_bytes());
    icon.extend_from_slice(&0_i32.to_le_bytes());
    icon.extend_from_slice(&0_u32.to_le_bytes());
    icon.extend_from_slice(&0_u32.to_le_bytes());

    for y in (0..SIDE).rev() {
        for x in 0..SIDE {
            let border = x < 2 || y < 2 || x >= SIDE - 2 || y >= SIDE - 2;
            let letter = (8..=12).contains(&x) && (7..=24).contains(&y)
                || (8..=23).contains(&x) && (20..=24).contains(&y);
            let (red, green, blue) = if letter {
                (225, 91, 35)
            } else if border {
                (29, 31, 28)
            } else {
                (247, 243, 235)
            };
            icon.extend_from_slice(&[blue, green, red, 255]);
        }
    }
    icon.resize(icon.len() + MASK_BYTES as usize, 0);
    icon
}
