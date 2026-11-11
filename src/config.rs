use std::env;

/// Application configuration loaded from .env file and environment variables.
#[derive(Clone)]
pub struct Config {
    /// The URL to which OCR results are POSTed.
    pub server_url: String,
    /// HTTP request timeout in seconds.
    pub request_timeout_secs: u64,
    /// Device name sent with every payload. `OCR_DEVICE_NAME` overrides the
    /// `COMPUTERNAME` fallback.
    pub device_name: String,
}

impl Config {
    /// Load configuration from the .env file located next to the executable,
    /// falling back to environment variables.
    pub fn load() -> Result<Self, String> {
        // Try to load .env from the executable's directory
        if let Ok(exe_path) = env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                let env_path = exe_dir.join(".env");
                if env_path.exists() {
                    dotenvy::from_path(&env_path)
                        .map_err(|e| format!("Failed to load .env from {:?}: {}", env_path, e))?;
                } else {
                    // Also try the current working directory
                    let cwd_env = std::env::current_dir().unwrap_or_default().join(".env");
                    if cwd_env.exists() {
                        dotenvy::from_path(&cwd_env).map_err(|e| {
                            format!("Failed to load .env from {:?}: {}", cwd_env, e)
                        })?;
                    } else {
                        // Try default dotenvy (looks in current dir)
                        let _ = dotenvy::dotenv();
                    }
                }
            }
        } else {
            let _ = dotenvy::dotenv();
        }

        let server_url = env::var("OCR_SERVER_URL").map_err(|_| {
            "OCR_SERVER_URL is not set. Please set it in .env file or environment.".to_string()
        })?;

        let request_timeout_secs = env::var("OCR_REQUEST_TIMEOUT_SECS")
            .unwrap_or_else(|_| "3".to_string())
            .parse::<u64>()
            .unwrap_or(3);

        let device_name = env::var("OCR_DEVICE_NAME")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| env::var("COMPUTERNAME").ok())
            .unwrap_or_else(|| "windows".to_string());

        Ok(Config {
            server_url,
            request_timeout_secs,
            device_name,
        })
    }
}
