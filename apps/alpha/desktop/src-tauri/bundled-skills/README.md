# Built-in skills

The skills in this directory are bundled at compile time by registering each name
and `SKILL.md` text in `src/skills.rs`'s `BUNDLED_SKILLS`. `marketing-preparation`
prepares promotional materials for the teacher to publish. The installer updates only files that still
match the previous bundled version; it preserves files changed by the user.

The installer covers `SKILL.md` only and maintains the provider copies. Saving
newly installed bundled files to Git still requires integration work before a
release can rely on their inclusion in GitHub backups.
