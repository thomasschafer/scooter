#!/bin/bash

README_FILE="README.md"
TEMP_FILE="README.tmp"
CONFIG_FILE="src/config.rs"
TOC_HEADING="## Contents"
TOC_START_MARKER="<!-- TOC START -->"
TOC_END_MARKER="<!-- TOC END -->"
CONFIG_START_MARKER="<!-- CONFIG START -->"
CONFIG_END_MARKER="<!-- CONFIG END -->"
EXTRACTOR_DIR="tools"
EXTRACTOR_FILE="$EXTRACTOR_DIR/extract_config.rs"

# Function to check if a file exists
check_file_exists() {
    local file=$1
    if [ ! -f "$file" ]; then
        echo "Error: $file not found!"
        exit 1
    fi
}

# Function to generate table of contents
generate_table_of_contents() {
    echo "Generating table of contents..."

    local toc=""
    local in_code_block=false
    local in_toc_section=false

    while IFS= read -r line; do
        # Check if we're entering/exiting a code block, skip if so
        if [[ "$line" =~ ^(\`\`\`|\~\~\~) ]]; then
            if $in_code_block; then
                in_code_block=false
            else
                in_code_block=true
            fi
            continue
        fi
        if $in_code_block; then
            continue
        fi

        # Check if we're entering/exiting the TOC section
        if [[ "$line" == "$TOC_START_MARKER" ]]; then
            in_toc_section=true
            continue
        fi
        if [[ "$line" == "$TOC_END_MARKER" ]]; then
            in_toc_section=false
            continue
        fi

        # Skip if we're in the TOC section
        if $in_toc_section; then
            continue
        fi

        # Skip the TOC heading itself
        if [[ "$line" == "$TOC_HEADING" ]]; then
            continue
        fi

        # Process heading lines (## and ###)
        if [[ "$line" =~ ^##\ (.+)$ ]] && [[ "$line" != "$TOC_HEADING" ]]; then
            title="${BASH_REMATCH[1]}"
            # Create anchor link (lowercase, replace spaces with hyphens)
            anchor=$(echo "$title" | tr '[:upper:]' '[:lower:]' | sed 's/ /-/g' | sed 's/[^a-z0-9-]//g')
            toc="${toc}- [${title}](#${anchor})\n"
        elif [[ "$line" =~ ^###\ (.+)$ ]]; then
            title="${BASH_REMATCH[1]}"
            # Create anchor link (lowercase, replace spaces with hyphens)
            anchor=$(echo "$title" | tr '[:upper:]' '[:lower:]' | sed 's/ /-/g' | sed 's/[^a-z0-9-]//g')
            toc="${toc}  - [${title}](#${anchor})\n"
        fi
    done < "$README_FILE"

    # Insert the TOC between markers
    awk -v toc="$toc" -v start="$TOC_START_MARKER" -v end="$TOC_END_MARKER" '
    {
        if ($0 == start) {
            print $0
            printf "%s", toc
            in_toc = 1
        } else if ($0 == end) {
            in_toc = 0
            print $0
        } else if (!in_toc) {
            print $0
        }
    }' "$README_FILE" > "$TEMP_FILE"

    mv "$TEMP_FILE" "$README_FILE"
    echo "Table of contents generated successfully"
}

extract_config_docs() {
    echo "Extracting config documentation..."

    check_file_exists $CONFIG_FILE

    local config_docs=""
    local in_config_struct=false
    local current_field=""
    local current_doc=""
    local in_doc=false

    while IFS= read -r line; do
        # Check if we're entering the Config struct
        if [[ "$line" =~ pub[[:space:]]+struct[[:space:]]+Config ]]; then
            in_config_struct=true
            continue
        fi

        # Check if we're exiting the Config struct
        if $in_config_struct && [[ "$line" =~ ^}$ ]]; then
            in_config_struct=false
            continue
        fi

        # Skip if not in Config struct
        if ! $in_config_struct; then
            continue
        fi

        # Capture doc comments
        if [[ "$line" =~ ^[[:space:]]*///(.*)$ ]]; then
            in_doc=true
            # Remove leading spaces and ///
            doc_line="${BASH_REMATCH[1]}"
            # Remove one leading space if present (common in Rust doc comments)
            doc_line="${doc_line# }"
            current_doc+="$doc_line\n"
        # Capture field declaration
        elif [[ "$line" =~ pub[[:space:]]+([a-zA-Z0-9_]+)[[:space:]]*:.*$ ]]; then
            field_name="${BASH_REMATCH[1]}"

            if $in_doc; then
                # Format the documentation into Markdown
                config_docs+="__${field_name}__\n\n"
                config_docs+="${current_doc}\n"

                # Reset for next field
                current_doc=""
                in_doc=false
            fi
        fi
    done < "$CONFIG_FILE"

    # Insert the config docs between markers
    awk -v docs="$config_docs" -v start="$CONFIG_START_MARKER" -v end="$CONFIG_END_MARKER" '
    {
        if ($0 == start) {
            print $0
            printf "%s", docs
            in_config = 1
        } else if ($0 == end) {
            in_config = 0
            print $0
        } else if (!in_config) {
            print $0
        }
    }' "$README_FILE" > "$TEMP_FILE"

    mv "$TEMP_FILE" "$README_FILE"
    echo "Configuration documentation generated successfully"
}

main() {
    check_file_exists "$README_FILE"

    generate_table_of_contents

    extract_config_docs

    echo "All readme generation tasks completed successfully"
}

main
