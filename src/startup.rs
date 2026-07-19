use std::env;
use windows::Win32::System::Registry::{
    RegSetValueExW, RegCreateKeyW, RegCloseKey,
    HKEY_CURRENT_USER, REG_SZ,
};
use windows::core::PCWSTR;

const RUN_KEY_PATH: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const VALUE_NAME: &str = "GoshietaOcrService";

/// Register the current executable to auto-start with Windows.
/// Writes to HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run
pub fn register_startup() -> Result<(), String> {
    let exe_path = env::current_exe()
        .map_err(|e| format!("Failed to get current exe path: {}", e))?;

    let exe_path_str = exe_path
        .to_str()
        .ok_or_else(|| "Executable path contains invalid UTF-8".to_string())?;

    // Encode strings as UTF-16 for Windows API
    let key_path_wide: Vec<u16> = RUN_KEY_PATH.encode_utf16().chain(std::iter::once(0)).collect();
    let value_name_wide: Vec<u16> = VALUE_NAME.encode_utf16().chain(std::iter::once(0)).collect();
    let exe_path_wide: Vec<u16> = exe_path_str.encode_utf16().chain(std::iter::once(0)).collect();

    unsafe {
        let mut hkey = std::mem::zeroed();

        let result = RegCreateKeyW(
            HKEY_CURRENT_USER,
            PCWSTR::from_raw(key_path_wide.as_ptr()),
            &mut hkey,
        );

        if result.is_err() {
            return Err(format!("RegCreateKeyW failed: {:?}", result));
        }

        // Convert wide string to bytes for the registry value
        let exe_bytes: &[u8] = std::slice::from_raw_parts(
            exe_path_wide.as_ptr() as *const u8,
            exe_path_wide.len() * 2,
        );

        let set_result = RegSetValueExW(
            hkey,
            PCWSTR::from_raw(value_name_wide.as_ptr()),
            0,
            REG_SZ,
            Some(exe_bytes),
        );

        let _ = RegCloseKey(hkey);

        if set_result.is_err() {
            return Err(format!("RegSetValueExW failed: {:?}", set_result));
        }
    }

    Ok(())
}