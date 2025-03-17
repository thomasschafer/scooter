use anyhow::{Context, Result};
use quote::ToTokens;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use syn::{parse_file, Attribute, Field, Fields, Item, ItemStruct, Meta};

const TOC_START_MARKER: &str = "<!-- TOC START -->";
const TOC_END_MARKER: &str = "<!-- TOC END -->";
const CONFIG_START_MARKER: &str = "<!-- CONFIG START -->";
const CONFIG_END_MARKER: &str = "<!-- CONFIG END -->";
const TOC_HEADING: &str = "## Contents";

pub fn generate_readme(readme_path: &Path, config_path: &Path, check_only: bool) -> Result<()> {
    println!("Processing README file: {}", readme_path.display());

    let readme_content = fs::read_to_string(readme_path).context(format!(
        "Failed to read README file: {}",
        readme_path.display()
    ))?;

    let mut updated_content = generate_contents_table(&readme_content);

    if config_path.exists() {
        updated_content = generate_config_docs(&updated_content, config_path)?;
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

fn generate_contents_table(content: &str) -> String {
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
            toc.push_str(&format!("- [{}](#{})\n", title, anchor));
        } else if let Some(title) = line.strip_prefix("### ") {
            if !in_config_section(content, &lines, i) {
                let anchor = create_anchor(title);
                toc.push_str(&format!("  - [{}](#{})\n", title, anchor));
            }
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

    result
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

    let config_content = fs::read_to_string(config_path).context(format!(
        "Failed to read config file: {}",
        config_path.display()
    ))?;

    let syntax = parse_file(&config_content).context(format!(
        "Failed to parse config file: {}",
        config_path.display()
    ))?;

    let mut structs: HashMap<String, ItemStruct> = HashMap::new();
    for item in &syntax.items {
        if let Item::Struct(s) = item {
            structs.insert(s.ident.to_string(), s.clone());
        }
    }

    let mut docs = String::new();

    if let Some(config_struct) = structs.get("Config") {
        process_struct(&mut docs, config_struct, &structs, "");
    } else {
        println!("Warning: Config struct not found in the source file.");
    }

    let mut result = String::new();
    let mut in_config_section = false;
    for line in content.lines() {
        if line == CONFIG_START_MARKER {
            result.push_str(line);
            result.push('\n');
            result.push_str(&docs);
            in_config_section = true;
        } else if line == CONFIG_END_MARKER {
            in_config_section = false;
            result.push_str(line);
            result.push('\n');
        } else if !in_config_section {
            result.push_str(line);
            result.push('\n');
        }
    }

    Ok(result)
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
                let field_doc = extract_doc_comment(&field.attrs);

                if let Some(nested_struct) = all_structs.get(&get_type_name(field)) {
                    let toml_path = if toml_prefix.is_empty() {
                        field_name.clone()
                    } else {
                        format!("{}.{}", toml_prefix, field_name)
                    };

                    docs.push_str(&format!("### `[{}]` section\n\n", toml_path));

                    if !field_doc.is_empty() {
                        docs.push_str(&field_doc);
                        docs.push_str("\n\n");
                    }

                    process_struct(docs, nested_struct, all_structs, &toml_path);
                } else {
                    docs.push_str(&format!("#### `{}`\n\n", field_name));
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
        if attr.path().is_ident("doc") {
            if let Meta::NameValue(meta) = attr.meta.clone() {
                if let syn::Expr::Lit(expr_lit) = meta.value {
                    if let syn::Lit::Str(lit_str) = expr_lit.lit {
                        let comment = lit_str.value();
                        doc_lines.push(comment.trim().to_string());
                    }
                }
            }
        }
    }

    doc_lines.join("\n")
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

        let content = generate_contents_table(&initial_content);
        let content = generate_config_docs(&content, &config_file)
            .unwrap()
            .replace("\r\n", "\n");

        let expected = include_str!("fixtures/expected_readme.txt").replace("\r\n", "\n");

        pretty_assert_eq!(&content, &expected);
    }
}
