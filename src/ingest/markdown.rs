//! Markdown and Obsidian-flavored Markdown parser.

use super::{DocumentParser, ParsedDocument, Section};
use anyhow::Result;
use std::path::Path;

pub struct MarkdownParser;

impl DocumentParser for MarkdownParser {
    fn supports(&self, extension: &str) -> bool {
        matches!(extension, "md" | "markdown")
    }

    fn parse(&self, path: &Path) -> Result<ParsedDocument> {
        let content = std::fs::read_to_string(path)?;
        let title = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("untitled")
            .to_string();

        let mut warnings = Vec::new();
        let mut tags = Vec::new();
        let mut aliases = Vec::new();
        let mut wikilinks = Vec::new();
        let mut sections = Vec::new();
        let mut body_start = 0usize;
        let mut frontmatter_done = false;
        let mut in_frontmatter = false;
        let mut frontmatter_lines = Vec::new();

        // Parse line by line
        let lines: Vec<&str> = content.lines().collect();

        for (i, line) in lines.iter().enumerate() {
            let trimmed = line.trim();

            // Detect frontmatter boundaries
            if i == 0 && trimmed == "---" {
                in_frontmatter = true;
                continue;
            }
            if in_frontmatter && trimmed == "---" {
                in_frontmatter = false;
                frontmatter_done = true;
                body_start = i + 1;
                // Parse frontmatter YAML
                if let Ok(yaml) =
                    serde_yaml::from_str::<serde_yaml::Value>(&frontmatter_lines.join("\n"))
                {
                    if let Some(t) = yaml.get("tags") {
                        match t {
                            serde_yaml::Value::Sequence(seq) => {
                                for item in seq {
                                    if let Some(s) = item.as_str() {
                                        tags.push(s.to_string());
                                    }
                                }
                            }
                            serde_yaml::Value::String(s) => {
                                tags.push(s.clone());
                            }
                            _ => {}
                        }
                    }
                    if let Some(a) = yaml.get("aliases") {
                        if let Some(seq) = a.as_sequence() {
                            for item in seq {
                                if let Some(s) = item.as_str() {
                                    aliases.push(s.to_string());
                                }
                            }
                        }
                    }
                } else {
                    warnings.push("Failed to parse YAML frontmatter".to_string());
                }
                continue;
            }
            if in_frontmatter {
                frontmatter_lines.push(*line);
                continue;
            }

            // Extract #tags from lines
            for word in trimmed.split_whitespace() {
                if word.starts_with('#') && word.len() > 1 && !word.contains("://") {
                    let tag = word.trim_start_matches('#').to_string();
                    if !tags.contains(&tag) {
                        tags.push(tag);
                    }
                }
            }

            // Extract [[wikilinks]]
            let mut remaining = trimmed;
            while let Some(start) = remaining.find("[[") {
                let rest = &remaining[start + 2..];
                if let Some(end) = rest.find("]]") {
                    let link_text = &rest[..end];
                    let target = if let Some(pipe) = link_text.find('|') {
                        link_text[..pipe].to_string()
                    } else {
                        link_text.to_string()
                    };
                    if !wikilinks.contains(&target) {
                        wikilinks.push(target);
                    }
                    remaining = &rest[end + 2..];
                } else {
                    break;
                }
            }
        }

        // Build sections from headings
        let _body = lines[body_start..].join("\n");
        build_sections(&lines, body_start, &mut sections);

        let plain_text = lines.join("\n");

        Ok(ParsedDocument {
            title,
            plain_text,
            sections,
            tags,
            aliases,
            wikilinks,
            metadata: serde_json::json!({"frontmatter_parsed": frontmatter_done}),
            warnings,
        })
    }
}

/// Split body text into sections by heading level.
fn build_sections(lines: &[&str], offset: usize, sections: &mut Vec<Section>) {
    let mut current_heading = String::new();
    let mut current_path: Vec<String> = Vec::new();
    let mut current_lines: Vec<String> = Vec::new();
    let mut section_start = offset + 1;

    for (i, line) in lines.iter().enumerate().skip(offset) {
        let trimmed = line.trim();
        if let Some(heading) = trimmed.strip_prefix("# ") {
            // Save previous section
            if !current_lines.is_empty() {
                sections.push(Section {
                    heading: current_heading.clone(),
                    heading_path: current_path.clone(),
                    content: current_lines.join("\n"),
                    start_line: section_start,
                    end_line: i,
                });
            }
            current_heading = heading.to_string();
            current_path = vec![heading.to_string()];
            current_lines = Vec::new();
            section_start = i + 1;
        } else if let Some(heading) = trimmed.strip_prefix("## ") {
            if !current_lines.is_empty() {
                sections.push(Section {
                    heading: current_heading.clone(),
                    heading_path: current_path.clone(),
                    content: current_lines.join("\n"),
                    start_line: section_start,
                    end_line: i,
                });
            }
            let mut new_path = current_path.clone();
            if current_path.len() > 1 {
                new_path.truncate(1);
            }
            new_path.push(heading.to_string());
            current_heading = heading.to_string();
            current_path = new_path;
            current_lines = Vec::new();
            section_start = i + 1;
        } else {
            current_lines.push(line.to_string());
        }
    }

    // Don't forget the last section
    if !current_lines.is_empty() {
        sections.push(Section {
            heading: current_heading,
            heading_path: current_path,
            content: current_lines.join("\n"),
            start_line: section_start,
            end_line: lines.len(),
        });
    }
}
