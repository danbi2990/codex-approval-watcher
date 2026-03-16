#!/bin/zsh

set -euo pipefail

output_path="${1:?missing output path}"
mkdir -p "$(dirname "$output_path")"
cat > "$output_path"
