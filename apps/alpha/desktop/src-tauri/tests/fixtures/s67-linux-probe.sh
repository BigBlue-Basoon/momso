#!/usr/bin/env bash
set -eu

# The desktop's existing object-store tests refer to a Windows-only module.
# This script runs only in an isolated Linux archive, never in the source checkout.
python3 - <<'PY'
from pathlib import Path
p = Path('apps/alpha/desktop/src-tauri/src/object_store_state.rs')
text = p.read_text(encoding='utf-8')
old = '#[cfg(test)]\nmod tests {'
assert text.count(old) == 1
p.write_text(text.replace(old, '#[cfg(all(test, windows))]\nmod tests {'), encoding='utf-8')
PY

export CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_PROFILE_DEV_DEBUG=0
manifest=apps/alpha/desktop/src-tauri/Cargo.toml
for name in pdf_attachment_is_a_verified_acp_resource_link native_image_input_rejects_modified_originals image_attachment_becomes_anthropic_stream_content gemini_reads_cli_model_list fast_capabilities_do_not_assert_current_session_state; do
  cargo test --manifest-path "$manifest" -j 1 --lib "$name" -- --test-threads=2
done
