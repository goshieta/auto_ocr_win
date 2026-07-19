#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod capture;
mod ocr;
mod sender;
mod startup;

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{interval, Duration};

use config::Config;
use capture::ScreenCapture;
use ocr::OcrEngineWrapper;
use sender::DataSender;
use startup::register_startup;

/// Maximum number of failed send attempts to buffer in memory before dropping oldest.
const MAX_RETRY_BUFFER: usize = 10;

/// Interval between screen captures in seconds.
const CAPTURE_INTERVAL_SECS: u64 = 10;

#[tokio::main]
async fn main() {
    eprintln!("[auto_ocr_win] Starting...");

    // ── Config ──────────────────────────────────────────────────────
    let config = match Config::load() {
        Ok(c) => {
            eprintln!("[auto_ocr_win] Config loaded. server_url={}", c.server_url);
            c
        }
        Err(e) => {
            eprintln!("[auto_ocr_win] FATAL: Failed to load config: {}", e);
            log_error(&format!("Failed to load config: {}", e));
            return;
        }
    };

    // ── Single-instance check ───────────────────────────────────────
    let _instance_guard = match ensure_single_instance() {
        Ok(g) => {
            eprintln!("[auto_ocr_win] Single instance check passed.");
            g
        }
        Err(e) => {
            eprintln!("[auto_ocr_win] Another instance already running: {}", e);
            return;
        }
    };

    // ── Startup registration (non-fatal) ────────────────────────────
    let _ = std::panic::catch_unwind(|| {
        match register_startup() {
            Ok(()) => eprintln!("[auto_ocr_win] Startup registration succeeded."),
            Err(e) => {
                eprintln!("[auto_ocr_win] Startup registration failed (non-fatal): {}", e);
                log_error(&format!("Failed to register startup: {}", e));
            }
        }
    });

    // ── Initialize components ───────────────────────────────────────
    let capture = ScreenCapture::new();

    let ocr_engine = match OcrEngineWrapper::new() {
        Ok(e) => {
            eprintln!("[auto_ocr_win] OCR engine initialized.");
            e
        }
        Err(e) => {
            eprintln!("[auto_ocr_win] FATAL: Failed to create OCR engine: {}", e);
            log_error(&format!("Failed to create OCR engine: {}", e));
            return;
        }
    };

    let sender = DataSender::new(&config);

    // ── Retry buffer ────────────────────────────────────────────────
    let retry_buffer: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    // Per-monitor hash map for change detection.
    // Key: monitor_index, Value: previous XXH3 hash of that monitor's pixel data.
    let mut prev_hashes: HashMap<usize, u64> = HashMap::new();

    let mut ticker = interval(Duration::from_secs(CAPTURE_INTERVAL_SECS));
    // Consume the immediate first tick so we wait the full interval first.
    ticker.tick().await;

    eprintln!(
        "[auto_ocr_win] Main loop started. Interval: {}s",
        CAPTURE_INTERVAL_SECS
    );
    log_info("Auto OCR service started.");

    // ── Main loop ───────────────────────────────────────────────────
    // This loop MUST NOT exit under any circumstances (except process termination).
    // Every iteration is wrapped in catch_unwind-like recovery.
    loop {
        ticker.tick().await;

        // Wrap the entire iteration body to catch any unexpected panics.
        run_iteration_safe(
            &capture,
            &ocr_engine,
            &sender,
            &retry_buffer,
            &mut prev_hashes,
        )
        .await;
    }
}

/// Execute one capture→OCR→send iteration with comprehensive error handling.
/// Any error within this function is logged but never propagated outward.
async fn run_iteration_safe(
    capture: &ScreenCapture,
    ocr_engine: &OcrEngineWrapper,
    sender: &DataSender,
    retry_buffer: &Arc<Mutex<Vec<String>>>,
    prev_hashes: &mut HashMap<usize, u64>,
) {
    // ── 1. Capture all monitors ─────────────────────────────────────
    let images = match capture.capture_all() {
        Ok(imgs) => imgs,
        Err(e) => {
            eprintln!("[auto_ocr_win] Screen capture failed: {}", e);
            log_error(&format!("Screen capture failed: {}", e));
            return; // skip this iteration
        }
    };

    if images.is_empty() {
        eprintln!("[auto_ocr_win] No monitors captured, skipping.");
        return;
    }

    eprintln!(
        "[auto_ocr_win] Captured {} monitor(s) (total monitors: {})",
        images.len(),
        images.len()
    );

    // ── 2. For each monitor: hash check → OCR → aggregate text ──────
    let mut all_text = Vec::new();

    for image in &images {
        let monitor_idx = image.monitor_index;

        eprintln!(
            "[auto_ocr_win] Monitor {}: {}x{}, {} bytes",
            monitor_idx,
            image.width,
            image.height,
            image.data.len()
        );

        // Compute hash for change detection
        let current_hash = compute_hash(&image.data);

        let changed = match prev_hashes.get(&monitor_idx) {
            Some(&prev) => prev != current_hash,
            None => true, // first capture for this monitor → treat as changed
        };

        if !changed {
            eprintln!(
                "[auto_ocr_win] Monitor {}: no change detected, skipping OCR.",
                monitor_idx
            );
            prev_hashes.insert(monitor_idx, current_hash);
            continue;
        }

        prev_hashes.insert(monitor_idx, current_hash);

        eprintln!(
            "[auto_ocr_win] Monitor {}: screen changed, running OCR...",
            monitor_idx
        );

        // Run OCR
        let text = match ocr_engine.recognize(image) {
            Ok(t) => t,
            Err(e) => {
                eprintln!(
                    "[auto_ocr_win] OCR failed for monitor {}: {}",
                    monitor_idx, e
                );
                log_error(&format!("OCR failed for monitor {}: {}", monitor_idx, e));
                continue;
            }
        };

        let trimmed = text.trim().to_string();
        eprintln!(
            "[auto_ocr_win] Monitor {}: OCR result: {} chars",
            monitor_idx,
            trimmed.len()
        );

        if !trimmed.is_empty() {
            let max_display = trimmed.len().min(200);
            let mut display_end = max_display;
            while !trimmed.is_char_boundary(display_end) {
                display_end -= 1;
            }
            eprintln!(
                "[auto_ocr_win] Monitor {} text: {}",
                monitor_idx,
                &trimmed[..display_end]
            );
            all_text.push(trimmed);
        }
    }

    // ── 3. If no monitor produced new text, skip sending ────────────
    if all_text.is_empty() {
        eprintln!("[auto_ocr_win] No new text from any monitor, skipping send.");
        return;
    }

    // ── 4. Combine text from all changed monitors ───────────────────
    let combined_text = all_text.join("\n");

    // ── 5. Build JSON payload ──────────────────────────────────────
    let payload = match sender.build_payload(&combined_text) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[auto_ocr_win] Failed to build payload: {}", e);
            log_error(&format!("Failed to build payload: {}", e));
            return;
        }
    };

    // ── 6. Send (with retry buffer logic) ──────────────────────────
    let sender_clone = sender.clone();
    let buffer_clone = retry_buffer.clone();

    tokio::spawn(async move {
        // Flush buffered retries first
        flush_retry_buffer_safe(&sender_clone, &buffer_clone).await;

        // Send current payload
        match sender_clone.send(&payload).await {
            Ok(()) => eprintln!("[auto_ocr_win] Payload sent successfully."),
            Err(e) => {
                eprintln!("[auto_ocr_win] Send failed: {}", e);
                log_error(&format!("Send failed: {}", e));
                let mut buf = buffer_clone.lock().await;
                if buf.len() >= MAX_RETRY_BUFFER {
                    buf.remove(0);
                }
                buf.push(payload);
            }
        }
    });
}

/// Compute a 64-bit XXH3 hash of the pixel data for change detection.
fn compute_hash(data: &[u8]) -> u64 {
    xxhash_rust::xxh3::xxh3_64(data)
}

/// Ensure only one instance of the application runs at a time.
fn ensure_single_instance() -> Result<SingleInstanceGuard, String> {
    use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, HANDLE};
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::core::PCWSTR;

    let name: Vec<u16> = "Local\\AutoOcrWinService\0".encode_utf16().collect();

    unsafe {
        let handle: HANDLE = CreateMutexW(None, true, PCWSTR::from_raw(name.as_ptr()))
            .map_err(|e| format!("CreateMutexW failed: {:?}", e))?;

        let last_error = windows::Win32::Foundation::GetLastError();
        if last_error == ERROR_ALREADY_EXISTS {
            let _ = CloseHandle(handle);
            return Err("Another instance is already running".to_string());
        }

        Ok(SingleInstanceGuard { handle })
    }
}

struct SingleInstanceGuard {
    handle: windows::Win32::Foundation::HANDLE,
}

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

/// Flush retry buffer with error handling — never panics.
async fn flush_retry_buffer_safe(sender: &DataSender, buffer: &Arc<Mutex<Vec<String>>>) {
    let mut buf = buffer.lock().await;
    while !buf.is_empty() {
        let payload = &buf[0];
        match sender.send(payload).await {
            Ok(_) => {
                eprintln!("[auto_ocr_win] Flushed buffered payload.");
                buf.remove(0);
            }
            Err(e) => {
                eprintln!("[auto_ocr_win] Retry flush failed: {}", e);
                break;
            }
        }
    }
}

// ── Logging utilities ───────────────────────────────────────────────

fn log_info(msg: &str) {
    log_to_file("INFO", msg);
}

fn log_error(msg: &str) {
    log_to_file("ERROR", msg);
}

fn log_to_file(level: &str, msg: &str) {
    use std::fs::OpenOptions;
    use std::io::Write;

    let log_path = std::env::temp_dir().join("auto_ocr_win.log");
    let result = (|| -> Result<(), std::io::Error> {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)?;
        let timestamp = chrono_now();
        writeln!(file, "[{}] [{}] {}", timestamp, level, msg)?;
        Ok(())
    })();

    if let Err(e) = result {
        eprintln!(
            "[auto_ocr_win] Failed to write to log file '{}': {}",
            log_path.display(),
            e
        );
    }
}

fn chrono_now() -> String {
    use std::time::SystemTime;
    match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => {
            let secs = d.as_secs();
            let days = secs / 86400;
            let remaining = secs % 86400;
            let hours = remaining / 3600;
            let minutes = (remaining % 3600) / 60;
            let seconds = remaining % 60;
            format!("day{} {:02}:{:02}:{:02}", days, hours, minutes, seconds)
        }
        Err(_) => "unknown".to_string(),
    }
}