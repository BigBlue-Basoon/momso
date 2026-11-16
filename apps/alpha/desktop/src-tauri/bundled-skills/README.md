# Built-in skills

This directory intentionally contains no skill content. When content is approved,
bundle each skill at compile time and register its name and `SKILL.md` text in
`src/skills.rs`'s `BUNDLED_SKILLS`. The installer updates only files that still
match the previous bundled version; it preserves files changed by the user.

The installer covers `SKILL.md` only and maintains the provider copies. Saving
newly installed bundled files to Git still requires integration work before a
release can rely on their inclusion in GitHub backups.
