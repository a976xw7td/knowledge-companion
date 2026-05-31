//! Bundle root detection and configuration loading.
//!
//! Detection order:
//! 1. `KC_BUNDLE_ROOT` env var (override set by start script)
//! 2. Walk up from executable path looking for `config/knowledge-companion.toml`
//! 3. Walk up from current working directory
//!
//! Once found, all relative paths in the config are resolved against this root.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Marker files/directories that indicate a bundle root.
const BUNDLE_MARKERS: &[&str] = &["config", "knowledge", "data"];

/// Detect the bundle root directory.
///
/// Returns the absolute, canonicalized path to the bundle root.
pub fn detect_bundle_root() -> Result<PathBuf> {
    // 1. Environment variable override
    if let Ok(env_root) = std::env::var("KC_BUNDLE_ROOT") {
        let path = PathBuf::from(&env_root);
        if path.is_dir() {
            let canonical = std::fs::canonicalize(&path)
                .with_context(|| format!("KC_BUNDLE_ROOT path does not exist: {}", env_root))?;
            tracing::info!(
                bundle_root = %canonical.display(),
                "Using bundle root from KC_BUNDLE_ROOT env var"
            );
            return Ok(canonical);
        }
        tracing::warn!(
            "KC_BUNDLE_ROOT is set but path does not exist or is not a directory: {}",
            env_root
        );
    }

    // 2. Walk up from executable directory
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(root) = find_bundle_root_upwards(&exe_path) {
            return Ok(root);
        }
    }

    // 3. Walk up from current working directory
    let cwd = std::env::current_dir().context("Failed to get current working directory")?;
    if let Some(root) = find_bundle_root_upwards(&cwd) {
        return Ok(root);
    }

    Err(anyhow::anyhow!(
        "Could not detect bundle root. \
         Set KC_BUNDLE_ROOT environment variable to the KnowledgeSuite directory, \
         or run knowledge-companion from within the bundle."
    ))
}

/// Walk up from `start_path` looking for a bundle root.
fn find_bundle_root_upwards(start_path: &Path) -> Option<PathBuf> {
    let mut current = if start_path.is_dir() {
        start_path.to_path_buf()
    } else {
        start_path.parent()?.to_path_buf()
    };

    // Walk up to at most 8 levels to avoid infinite loops
    for _ in 0..8 {
        if is_bundle_root(&current) {
            if let Ok(canonical) = std::fs::canonicalize(&current) {
                tracing::info!(
                    bundle_root = %canonical.display(),
                    "Detected bundle root by walking up from executable"
                );
                return Some(canonical);
            }
        }
        if !current.pop() {
            break;
        }
    }
    None
}

/// Check if a directory looks like a KnowledgeSuite bundle root.
fn is_bundle_root(dir: &Path) -> bool {
    BUNDLE_MARKERS
        .iter()
        .filter(|m| dir.join(m).is_dir())
        .count()
        >= 2
}

/// Load the application configuration from the bundle root.
///
/// The config file is expected at `<bundle_root>/config/knowledge-companion.toml`.
/// If the file does not exist, returns a default config with paths resolved
/// relative to the bundle root.
pub fn load_config(bundle_root: &Path) -> Result<super::AppConfig> {
    let config_path = std::env::var("KC_CONFIG_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| bundle_root.join("config").join("knowledge-companion.toml"));

    if config_path.exists() {
        let content = std::fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read config file: {}", config_path.display()))?;
        let config: super::AppConfig = toml::from_str(&content)
            .with_context(|| format!("Failed to parse config file: {}", config_path.display()))?;
        tracing::info!(config = %config_path.display(), "Loaded configuration");
        Ok(config)
    } else {
        tracing::warn!(
            "Config file not found at {}, using defaults",
            config_path.display()
        );
        Ok(super::AppConfig::default())
    }
}

/// Persist configuration to the active config path.
pub fn save_config(bundle_root: &Path, config: &super::AppConfig) -> Result<()> {
    let config_path = std::env::var("KC_CONFIG_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| bundle_root.join("config").join("knowledge-companion.toml"));
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = toml::to_string_pretty(config).context("Failed to serialize configuration")?;
    std::fs::write(&config_path, content)
        .with_context(|| format!("Failed to write config file: {}", config_path.display()))
}

/// Resolve a relative path against the bundle root.
#[allow(dead_code)] // Used in future phases
pub fn resolve_path(bundle_root: &Path, relative: &str) -> PathBuf {
    let relative = relative.trim_start_matches("./");
    bundle_root.join(relative)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_fake_bundle() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let bundle_root = dir.path().to_path_buf();
        std::fs::create_dir_all(bundle_root.join("config")).unwrap();
        std::fs::create_dir_all(bundle_root.join("knowledge")).unwrap();
        std::fs::create_dir_all(bundle_root.join("data/logs")).unwrap();
        std::fs::create_dir_all(bundle_root.join("data/cache")).unwrap();
        (dir, bundle_root)
    }

    #[test]
    fn test_is_bundle_root_positive() {
        let (_dir, bundle_root) = create_fake_bundle();
        assert!(is_bundle_root(&bundle_root));
    }

    #[test]
    fn test_is_bundle_root_negative() {
        let dir = TempDir::new().unwrap();
        assert!(!is_bundle_root(dir.path()));
    }

    #[test]
    fn test_detect_bundle_root_from_env() {
        let (_dir, bundle_root) = create_fake_bundle();
        std::env::set_var("KC_BUNDLE_ROOT", bundle_root.to_str().unwrap());
        let detected = detect_bundle_root().unwrap();
        assert_eq!(detected, std::fs::canonicalize(&bundle_root).unwrap());
        std::env::remove_var("KC_BUNDLE_ROOT");
    }

    #[test]
    fn test_detect_bundle_root_env_not_exists() {
        std::env::set_var("KC_BUNDLE_ROOT", "/nonexistent/path/12345");
        // Should warn but not panic — fall back to cwd detection
        // Since cwd might not be a bundle root either, we accept an error
        let _ = detect_bundle_root();
        std::env::remove_var("KC_BUNDLE_ROOT");
    }

    #[test]
    fn test_resolve_relative_paths() {
        let (_dir, bundle_root) = create_fake_bundle();
        assert_eq!(
            resolve_path(&bundle_root, "./knowledge"),
            bundle_root.join("knowledge")
        );
        assert_eq!(
            resolve_path(&bundle_root, "./data/knowledge.db"),
            bundle_root.join("data/knowledge.db")
        );
    }

    #[test]
    fn test_load_config_default() {
        let (_dir, bundle_root) = create_fake_bundle();
        let config = load_config(&bundle_root).unwrap();
        assert_eq!(config.app.name, "KnowledgeCompanion");
        assert_eq!(config.knowledge.roots.len(), 0);
    }
}
