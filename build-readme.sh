#!/bin/bash

README_FILE="README.md"
TEMP_FILE="README.tmp"
TOC_START_MARKER="<!-- TOC START -->"
TOC_END_MARKER="<!-- TOC END -->"
TOC_HEADING="## Contents"

if [ ! -f "$README_FILE" ]; then
    echo "Error: $README_FILE not found!"
    exit 1
fi

cp "$README_FILE" "$TEMP_FILE"

# Extract headings (## and ###) from the README
# Skip the TOC section itself and any code blocks
toc=""
in_code_block=false
in_toc_section=false

while IFS= read -r line; do
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

    if [[ "$line" == "$TOC_START_MARKER" ]]; then
        in_toc_section=true
        continue
    fi
    if [[ "$line" == "$TOC_END_MARKER" ]]; then
        in_toc_section=false
        continue
    fi

    if $in_toc_section; then
        continue
    fi

    if [[ "$line" == "$TOC_HEADING" ]]; then
        continue
    fi

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

echo "Table of contents generated successfully!"
