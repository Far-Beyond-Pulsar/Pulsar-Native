#!/bin/bash

# Script to replace println! with tracing::info! and eprintln! with tracing::error!
# across the Pulsar workspace

# Colors for output
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo "🔍 Finding all println! and eprintln! statements..."

# Find all Rust files with println! or eprintln! (excluding target, .git, and test files)
files=$(find . -name "*.rs" \
    -not -path "./target/*" \
    -not -path "./.git/*" \
    -not -path "*/tests/*" \
    -type f \
    -exec grep -l "println!\|eprintln!" {} \;)

count=$(echo "$files" | wc -l)
echo -e "${YELLOW}Found $count files with println! or eprintln!${NC}"

# Process each file
for file in $files; do
    # Skip if file doesn't exist or is binary
    if [ ! -f "$file" ]; then
        continue
    fi

    # Check if file has println! or eprintln!
    if grep -q "println!\|eprintln!" "$file"; then
        echo -e "${GREEN}Processing: $file${NC}"

        # Create backup
        cp "$file" "$file.bak"

        # Replace patterns
        # Replace eprintln! with tracing::error!
        sed -i 's/eprintln!/tracing::error!/g' "$file"

        # Replace println! with debug messages (when obvious debug context)
        sed -i 's/println!\(\s*(\s*"DEBUG:\)/tracing::debug!(/g' "$file"
        sed -i 's/println!\(\s*(\s*"Debug:\)/tracing::debug!(/g' "$file"

        # Replace println! with tracing::info! for general messages
        sed -i 's/println!/tracing::info!/g' "$file"

        # Check if file needs tracing import
        if ! grep -q "use tracing::" "$file" && ! grep -q "tracing::" "$file" | head -50; then
            # Try to add tracing to imports if not already there
            # This is a simple approach - might need manual adjustment
            echo "  ⚠️  May need to add 'use tracing;' or ensure tracing dependency"
        fi
    fi
done

echo ""
echo -e "${GREEN}✅ Replacement complete!${NC}"
echo ""
echo "📝 Next steps:"
echo "1. Review the changes with: git diff"
echo "2. Ensure all crates have tracing as a dependency"
echo "3. Run: cargo check to find any missing imports"
echo "4. Restore backups if needed: find . -name '*.rs.bak' -exec bash -c 'mv \"\$0\" \"\${0%.bak}\"' {} \\;"
echo ""
echo "Backup files created with .bak extension"
