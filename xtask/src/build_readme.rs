use anyhow::{Context, Result};
use quote::ToTokens;
use std::collections::HashMap;
use std::fmt::Write;
use std::fs;
use std::path::Path;
use syn::{Attribute, Field, Fields, Item, ItemStruct, Meta, parse_file};

const TOC_START_MARKER: &str = "<!-- TOC START -->";
const TOC_END_MARKER: &str = "<!-- TOC END -->";
const CONFIG_START_MARKER: &str = "<!-- CONFIG START -->";
const CONFIG_END_MARKER: &str = "<!-- CONFIG END -->";
const KEYS_START_MARKER: &str = "<!-- KEYS START -->";
const KEYS_END_MARKER: &str = "<!-- KEYS END -->";
const TOC_HEADING: &str = "## Contents";
const KEYS_FIELD_NAME: &str = "keys";

pub fn generate_readme(readme_path: &Path, config_path: &Path, check_only: bool) -> Result<()> {
    println!("Processing README file: {}", readme_path.display());

    let readme_content = fs::read_to_string(readme_path).context(format!(
        "Failed to read README file: {}",
        readme_path.display()
    ))?;

    let mut updated_content = generate_contents_table(&readme_content)?;

    if config_path.exists() {
        updated_content = generate_config_docs(&updated_content, config_path)?;
        updated_content = generate_keys_docs(&updated_content)?;
    } else {
        println!(
            "Warning: Config file '{}' not found.",
            config_path.display()
        );
        std::process::exit(1);
    }

    if updated_content != readme_content {
        if check_only {
            println!("README is out of date and needs regenerating");
            std::process::exit(1);
        }
        fs::write(readme_path, updated_content).context(format!(
            "Failed to write README file: {}",
            readme_path.display()
        ))?;

        println!("README file updated successfully");
    } else {
        println!("README file is already up to date");
    }

    Ok(())
}

fn generate_contents_table(content: &str) -> anyhow::Result<String> {
    println!("Generating table of contents...");

    let mut toc = String::new();
    let mut in_code_block = false;
    let mut in_toc_section = false;
    let lines: Vec<&str> = content.lines().collect();

    // First pass: collect headings and generate TOC
    for (i, line) in lines.iter().enumerate() {
        if line.starts_with("```") || line.starts_with("~~~") {
            in_code_block = !in_code_block;
            continue;
        }

        if in_code_block {
            continue;
        }

        if *line == TOC_START_MARKER {
            in_toc_section = true;
            continue;
        }
        if *line == TOC_END_MARKER {
            in_toc_section = false;
            continue;
        }

        if in_toc_section {
            continue;
        }

        if *line == TOC_HEADING {
            continue;
        }

        if line.starts_with("## ") && *line != TOC_HEADING {
            let title = &line[3..];
            let anchor = create_anchor(title);
            writeln!(toc, "- [{title}](#{anchor})")?;
        } else if let Some(title) = line.strip_prefix("### ")
            && !in_config_section(content, &lines, i)
        {
            let anchor = create_anchor(title);
            writeln!(toc, "  - [{title}](#{anchor})")?;
        }
    }

    // Second pass: replace existing TOC
    let mut result = String::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        result.push_str(line);
        result.push('\n');

        if line == TOC_START_MARKER {
            // Skip until end marker
            result.push_str(&toc);
            while i + 1 < lines.len() && lines[i + 1] != TOC_END_MARKER {
                i += 1;
            }
        }
        i += 1;
    }

    Ok(result)
}

// Check if a heading is part of the Configuration Options section
fn in_config_section(content: &str, lines: &[&str], current_idx: usize) -> bool {
    let config_heading = "## Configuration options";
    if !content.contains(config_heading) {
        return false;
    }

    let mut i = current_idx;
    while i > 0 {
        i -= 1;
        let line = lines[i];

        if line.starts_with("## ") {
            return line == config_heading;
        }
    }

    false
}

fn create_anchor(title: &str) -> String {
    // Create GitHub-compatible anchor: lowercase, replace spaces with hyphens, remove non-alphanumeric
    let mut anchor = String::new();
    for c in title.chars() {
        if c.is_alphanumeric() {
            anchor.push(c.to_lowercase().next().unwrap());
        } else if c.is_whitespace() {
            anchor.push('-');
        }
    }
    anchor
}

fn generate_config_docs(content: &str, config_path: &Path) -> Result<String> {
    println!("Extracting config documentation...");

    let structs = parse_config_structs(config_path)?;

    let mut docs = String::new();

    if let Some(config_struct) = structs.get("Config") {
        process_struct(&mut docs, config_struct, &structs, "");
    } else {
        println!("Warning: Config struct not found in the source file.");
    }

    Ok(replace_section_content(
        content,
        CONFIG_START_MARKER,
        CONFIG_END_MARKER,
        &docs,
    ))
}

fn process_struct(
    docs: &mut String,
    struct_item: &ItemStruct,
    all_structs: &HashMap<String, ItemStruct>,
    toml_prefix: &str,
) {
    if let Fields::Named(ref fields) = struct_item.fields {
        for field in &fields.named {
            if let Some(ident) = &field.ident {
                let field_name = ident.to_string();

                // Skip the keys field as it has its own section (see `generate_keys_docs`)
                if toml_prefix.is_empty() && field_name == KEYS_FIELD_NAME {
                    continue;
                }

                let field_doc = extract_doc_comment(&field.attrs);

                if let Some(nested_struct) = all_structs.get(&get_type_name(field)) {
                    let toml_path = if toml_prefix.is_empty() {
                        field_name.clone()
                    } else {
                        format!("{toml_prefix}.{field_name}")
                    };

                    #[allow(clippy::format_push_string)]
                    docs.push_str(&format!("### `[{toml_path}]` section\n\n",));

                    if !field_doc.is_empty() {
                        docs.push_str(&field_doc);
                        docs.push_str("\n\n");
                    }

                    process_struct(docs, nested_struct, all_structs, &toml_path);
                } else {
                    #[allow(clippy::format_push_string)]
                    docs.push_str(&format!("#### `{field_name}`\n\n",));
                    docs.push_str(&field_doc);
                    docs.push_str("\n\n");
                }
            }
        }
    }
}

fn get_type_name(field: &Field) -> String {
    field
        .ty
        .to_token_stream()
        .to_string()
        .trim_start_matches("Option < ")
        .trim_end_matches(" >")
        .to_string()
}

fn extract_doc_comment(attrs: &[Attribute]) -> String {
    let mut doc_lines = Vec::new();

    for attr in attrs {
        if attr.path().is_ident("doc")
            && let Meta::NameValue(meta) = attr.meta.clone()
            && let syn::Expr::Lit(expr_lit) = meta.value
            && let syn::Lit::Str(lit_str) = expr_lit.lit
        {
            let comment = lit_str.value();
            doc_lines.push(comment.trim().to_string());
        }
    }

    doc_lines.join("\n")
}

fn generate_keys_docs(content: &str) -> Result<String> {
    println!("Generating keys documentation...");

    let keys_config = scooter_core::config::KeysConfig::default();
    let toml_str =
        toml::to_string(&keys_config).context("Failed to serialize keys config to TOML")?;

    let config_path = Path::new("scooter-core/src/config.rs");
    let structs = parse_config_structs(config_path)?;

    // Extract doc comments for all structs in the keys hierarchy
    let doc_comments = extract_keys_doc_comments(&structs)?;

    // Parse TOML into sections for alignment
    let mut sections: Vec<(String, Vec<String>)> = Vec::new();
    let mut current_section = String::new();
    let mut current_lines: Vec<String> = Vec::new();

    for line in toml_str.lines() {
        if line.starts_with('[') && !line.starts_with("[[") {
            // Save previous section if it exists
            if !current_section.is_empty() {
                sections.push((current_section.clone(), current_lines.clone()));
                current_lines.clear();
            }

            // Extract section name (e.g., "general" from "[general]")
            let section_name = &line[1..line.len() - 1];
            current_section = format!("keys.{section_name}");
        } else if line.contains('=') {
            current_lines.push(line.to_string());
        }
    }

    // Don't forget the last section
    if !current_section.is_empty() {
        sections.push((current_section, current_lines));
    }

    // Generate output with aligned comments
    let mut keys_str_with_comments = String::new();

    for (section, lines) in sections {
        // Add section comment above the section header
        if let Some(doc) = doc_comments.get(&section) {
            writeln!(keys_str_with_comments, "# {doc}")?;
        }
        writeln!(keys_str_with_comments, "[{section}]")?;

        if lines.is_empty() {
            writeln!(keys_str_with_comments)?;
            continue;
        }

        // Calculate max width for alignment
        let max_width = lines
            .iter()
            .map(std::string::String::len)
            .max()
            .unwrap_or(0);

        // Add fields with aligned comments
        for line in &lines {
            let field_name = line.split('=').next().unwrap().trim();
            let field_key = format!("{section}.{field_name}");

            if let Some(doc) = doc_comments.get(&field_key) {
                // Pad line to max_width, then add comment
                writeln!(keys_str_with_comments, "{line:<max_width$}  # {doc}")?;
            } else {
                writeln!(keys_str_with_comments, "{line}")?;
            }
        }

        writeln!(keys_str_with_comments)?;
    }

    let keys_docs = format!("```toml\n{keys_str_with_comments}```\n");

    Ok(replace_section_content(
        content,
        KEYS_START_MARKER,
        KEYS_END_MARKER,
        &keys_docs,
    ))
}

/// Parse config.rs and extract all struct definitions
fn parse_config_structs(config_path: &Path) -> Result<HashMap<String, ItemStruct>> {
    let config_content = fs::read_to_string(config_path).context(format!(
        "Failed to read config file: {}",
        config_path.display()
    ))?;

    let syntax = parse_file(&config_content).context(format!(
        "Failed to parse config file: {}",
        config_path.display()
    ))?;

    let mut structs = HashMap::new();
    for item in &syntax.items {
        if let Item::Struct(s) = item {
            structs.insert(s.ident.to_string(), s.clone());
        }
    }

    Ok(structs)
}

/// Replace content between start and end markers with new content
fn replace_section_content(
    content: &str,
    start_marker: &str,
    end_marker: &str,
    new_content: &str,
) -> String {
    let mut result = String::new();
    let mut in_section = false;

    for line in content.lines() {
        if line == start_marker {
            result.push_str(line);
            result.push('\n');
            result.push_str(new_content);
            in_section = true;
        } else if line == end_marker {
            in_section = false;
            result.push_str(line);
            result.push('\n');
        } else if !in_section {
            result.push_str(line);
            result.push('\n');
        }
    }

    result
}

fn extract_keys_doc_comments(
    structs: &HashMap<String, ItemStruct>,
) -> Result<HashMap<String, String>> {
    let mut comments = HashMap::new();

    // Start with KeysConfig struct
    if let Some(keys_config) = structs.get("KeysConfig") {
        extract_struct_comments(keys_config, structs, "keys", &mut comments)?;
    }

    Ok(comments)
}

fn extract_struct_comments(
    struct_item: &ItemStruct,
    all_structs: &HashMap<String, ItemStruct>,
    prefix: &str,
    comments: &mut HashMap<String, String>,
) -> Result<()> {
    if let Fields::Named(ref fields) = struct_item.fields {
        for field in &fields.named {
            if let Some(ident) = &field.ident {
                let field_name = ident.to_string();
                let field_path = format!("{prefix}.{field_name}");
                let doc = extract_doc_comment(&field.attrs);

                let type_name = get_type_name(field);

                // Check if this field is a nested struct
                if let Some(nested_struct) = all_structs.get(&type_name) {
                    // Store section-level comment
                    if !doc.is_empty() {
                        comments.insert(field_path.clone(), doc);
                    }

                    // Recursively extract comments from nested struct
                    extract_struct_comments(nested_struct, all_structs, &field_path, comments)?;
                } else {
                    // This is a regular field (Vec<KeyEvent>), store its comment
                    if !doc.is_empty() {
                        comments.insert(field_path, doc);
                    }
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq as pretty_assert_eq;

    #[test]
    fn test_full_readme_with_config() {
        let config_file = Path::new("src").join("fixtures").join("config.txt");
        let readme_file = Path::new("src").join("fixtures").join("readme.txt");

        let initial_content = fs::read_to_string(&readme_file).unwrap();

        let content = generate_contents_table(&initial_content).unwrap();
        let content = generate_config_docs(&content, &config_file)
            .unwrap()
            .replace("\r\n", "\n");

        let expected = include_str!("fixtures/expected_readme.txt").replace("\r\n", "\n");

        pretty_assert_eq!(&content, &expected);
    }
}
