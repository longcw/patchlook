#![cfg(target_os = "windows")]

use std::os::windows::ffi::OsStrExt;
use windows::{
    Win32::{
        Foundation::FreeLibrary,
        System::LibraryLoader::{
            LOAD_LIBRARY_AS_DATAFILE, LOAD_LIBRARY_AS_IMAGE_RESOURCE, LoadLibraryExW,
        },
        UI::WindowsAndMessaging::{DestroyIcon, HICON, IMAGE_ICON, LR_DEFAULTCOLOR, LoadImageW},
    },
    core::PCWSTR,
};

#[test]
fn executable_contains_icons_loadable_by_the_windows_backend() {
    let path: Vec<u16> = std::path::Path::new(env!("CARGO_BIN_EXE_patchlook"))
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    unsafe {
        // Load only the built application's resources, without starting its UI.
        let module = LoadLibraryExW(
            PCWSTR(path.as_ptr()),
            None,
            LOAD_LIBRARY_AS_DATAFILE | LOAD_LIBRARY_AS_IMAGE_RESOURCE,
        )
        .expect("Cannot read the application resources");
        for size in [16, 24, 32, 48, 64, 128, 256] {
            // Use the same resource ID that GPUI loads for its window class.
            let icon = LoadImageW(
                Some(module.into()),
                PCWSTR(std::ptr::without_provenance(1)),
                IMAGE_ICON,
                size,
                size,
                LR_DEFAULTCOLOR,
            )
            .expect("The application icon must be loadable at every supported size");
            DestroyIcon(HICON(icon.0)).unwrap();
        }
        FreeLibrary(module).unwrap();
    }
}
