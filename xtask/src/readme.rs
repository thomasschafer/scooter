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
        // Handle code blocks
        if line.starts_with("```") || line.starts_with("~~~") {
            in_code_block = !in_code_block;
            continue;
        }

        // Skip if in code block
        if in_code_block {
            continue;
        }

        // Check if we're in the TOC section
        if *line == TOC_START_MARKER {
            in_toc_section = true;
            continue;
        }
        if *line == TOC_END_MARKER {
            in_toc_section = false;
            continue;
        }

        // Skip if in TOC section
        if in_toc_section {
            continue;
        }

        // Skip the TOC heading itself
        if *line == TOC_HEADING {
            continue;
        }

        // Process heading lines (## and ###)
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
    // Find the "Configuration options" heading
    let config_heading = "## Configuration options";

    // First check if "Configuration options" exists in the document
    if !content.contains(config_heading) {
        return false;
    }

    // Look backward to find the last h2 heading
    let mut i = current_idx;
    while i > 0 {
        i -= 1;
        let line = lines[i];

        if line.starts_with("## ") {
            // If the last h2 heading is "Configuration options", this is in the config section
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

    // Read the config file
    let config_content = fs::read_to_string(config_path).context(format!(
        "Failed to read config file: {}",
        config_path.display()
    ))?;

    // Parse the Rust source
    let syntax = parse_file(&config_content).context(format!(
        "Failed to parse config file: {}",
        config_path.display()
    ))?;

    // Collect all struct definitions and their documentation
    let mut structs: HashMap<String, ItemStruct> = HashMap::new();
    for item in &syntax.items {
        if let Item::Struct(s) = item {
            structs.insert(s.ident.to_string(), s.clone());
        }
    }

    // Generate documentation string
    let mut docs = String::new();
    docs.push_str(
        r#"Scooter looks for a TOML configuration file at:

- Linux or macOS: `~/.config/scooter/config.toml`
- Windows: `%AppData%\scooter\config.toml`

The following options can be set in your configuration file:

"#,
    );

    // Process Config struct and its nested structs
    if let Some(config_struct) = structs.get("Config") {
        process_struct(&mut docs, config_struct, &structs, "");
    } else {
        println!("Warning: Config struct not found in the source file.");
    }

    // Insert config docs into README
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
    prefix: &str,
) {
    if let Fields::Named(ref fields) = struct_item.fields {
        // First, process any struct documentation
        let struct_doc = extract_doc_comment(&struct_item.attrs);
        if !prefix.is_empty() && !struct_doc.is_empty() {
            docs.push_str(&format!("### {}\n\n", prefix));
            docs.push_str(&struct_doc);
            docs.push_str("\nConfiguration options for this section:\n\n");
        }

        for field in &fields.named {
            if let Some(ident) = &field.ident {
                let field_name = ident.to_string();
                let field_doc = extract_doc_comment(&field.attrs);

                // We'll remove this variable since it's not used
                // Just directly use field_name or build the path where needed

                if is_nested_struct(field) {
                    // This is a nested struct field
                    if let Some(type_name) = get_type_name(field) {
                        if let Some(nested_struct) = all_structs.get(&type_name) {
                            // For nested structs, add the section header with field documentation
                            docs.push_str(&format!("### {}\n\n", field_name));
                            docs.push_str(&field_doc);
                            docs.push('\n');

                            // Process the nested struct with the field name as prefix
                            process_struct(docs, nested_struct, all_structs, &field_name);
                        }
                    }
                } else {
                    // Regular field
                    if prefix.is_empty() {
                        docs.push_str(&format!("### {}\n\n", field_name));
                    } else {
                        docs.push_str(&format!("#### {}\n\n", field_name));
                    }
                    docs.push_str(&field_doc);
                    docs.push_str("\n\n");
                }
            }
        }
    }
}

fn is_nested_struct(field: &Field) -> bool {
    // Check if the field type is a custom type (not a primitive or standard library type)
    match get_type_name(field) {
        Some(type_name) => {
            ![
                "String", "bool", "i32", "i64", "u32", "u64", "f32", "f64", "char", "Vec",
            ]
            .iter()
            .any(|&t| type_name == t)
                && !type_name.starts_with("Option<")
        }
        None => false,
    }
}

fn get_type_name(field: &Field) -> Option<String> {
    // Extract the type name from the field using the public ToTokens trait
    let type_str = field.ty.to_token_stream().to_string();

    // Handle Option<TypeName>
    if type_str.starts_with("Option < ") {
        let inner_type = type_str
            .trim_start_matches("Option < ")
            .trim_end_matches(" >");
        Some(inner_type.to_string())
    } else {
        Some(type_str)
    }
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
