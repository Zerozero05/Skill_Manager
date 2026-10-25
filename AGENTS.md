# Skill Manager maintenance

Keep one canonical runtime folder per skill. Register existing folders in place. Do not silently copy client deployments or overwrite local edits. Never modify system/plugin-owned skills by default.

GitHub README and management files are remote documentation, not deletions to propagate from a minimal local installation. Unknown versions and unknown baselines must remain unknown. Network writes and local replacements require a preview tied to the exact original content.

Keep tokens in the operating-system credential store, never ordinary settings/logs. Render untrusted Markdown without raw HTML execution. Limit file operations to the selected roots and reject traversal and internal reparse points.

Use synthetic test fixtures. Do not change the user's skills, client directories, or private repositories during testing. Do not publish a repository, tag, release, or asset without explicit approval. Do not clean existing user files.

The root agent owns `.planning/v1/`; contributors only edit their assigned files. Update README summaries and CHANGELOG for real repository changes. Preserve third-party licenses and record actual reuse in docs/SOURCES.md.
