//! File scanner — walks watched roots with include/exclude glob filtering.

use anyhow::Result;
use globset::{Glob, GlobSet, GlobSetBuilder};
use std::path::Path;
use walkdir::WalkDir;

/// Info about a scanned file.
#[derive(Debug, Clone)]
pub struct ScannedFile {
    pub absolute_path: std::path::PathBuf,
    pub relative_path: String,
    pub size: u64,
    pub mtime: i64,
}

/// Scan a directory with include/exclude glob patterns.
///
/// Returns a list of matching files, sorted by relative path for
/// deterministic processing order.
pub fn scan(
    root: &Path,
    include_globs: &[String],
    exclude_globs: &[String],
) -> Result<Vec<ScannedFile>> {
    let include_set = build_glob_set(include_globs)?;
    let exclude_set = build_glob_set(exclude_globs)?;

    let mut files = Vec::new();

    let root_canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());

    for entry in WalkDir::new(&root_canonical)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            // Skip hidden dirs and excluded paths early
            let path = e.path();
            if let Ok(rel) = path.strip_prefix(&root_canonical) {
                let rel_str = rel.to_string_lossy();
                let with_slash = if path.is_dir() {
                    format!("{}/", rel_str)
                } else {
                    rel_str.to_string()
                };
                !exclude_set.is_match(&with_slash) && !exclude_set.is_match(&*rel_str)
            } else {
                true
            }
        })
    {
        let entry = entry?;
        let path = entry.path();

        if !path.is_file() {
            continue;
        }

        let rel = path
            .strip_prefix(&root_canonical)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();

        // Apply include filter
        let rel_with_slash = format!("{}/", rel);
        if !include_set.is_match(&rel) && !include_set.is_match(&rel_with_slash) {
            continue;
        }

        let metadata = path.metadata()?;
        let mtime = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        files.push(ScannedFile {
            absolute_path: path.to_path_buf(),
            relative_path: rel,
            size: metadata.len(),
            mtime,
        });
    }

    // Deterministic order
    files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));

    Ok(files)
}

fn build_glob_set(patterns: &[String]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for p in patterns {
        if p.is_empty() {
            continue;
        }
        let glob = Glob::new(p)?;
        builder.add(glob);
    }
    Ok(builder.build()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_scan_basic() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join("readme.md"), "# Hello").unwrap();
        fs::write(dir.path().join("notes.txt"), "notes").unwrap();
        fs::write(dir.path().join("sub/doc.md"), "doc").unwrap();
        fs::write(dir.path().join("script.sh"), "sh").unwrap();

        let files = scan(dir.path(), &["**/*.md".into(), "**/*.txt".into()], &[]).unwrap();

        let names: Vec<&str> = files.iter().map(|f| f.relative_path.as_str()).collect();
        assert!(names.contains(&"readme.md"));
        assert!(names.contains(&"notes.txt"));
        assert!(names.contains(&"sub/doc.md"));
        assert!(!names.contains(&"script.sh"));
    }

    #[test]
    fn test_scan_excludes() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join(".git")).unwrap();
        fs::write(dir.path().join("readme.md"), "").unwrap();
        fs::write(dir.path().join(".git/config"), "").unwrap();

        let files = scan(dir.path(), &["**/*".into()], &["**/.git/**".into()]).unwrap();

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].relative_path, "readme.md");
    }
}
