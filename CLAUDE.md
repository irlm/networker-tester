# CLAUDE.md

Project-specific instructions for Claude Code.

## Quality Checks

After implementing any fix, test the exact end-to-end workflow (e.g., curl|bash install, remote SSH deploy, CI pipeline) before marking complete. Do not assume partial unit tests cover integration paths.

## Rust / Cargo

Always commit Cargo.lock when making release tags or merging deployment PRs. Run `cargo generate-lockfile` if needed.

## Documentation

When writing documentation for CLI flags or environment variables (e.g., RUST_LOG), verify the documented values by actually running the binary with those settings before committing.
