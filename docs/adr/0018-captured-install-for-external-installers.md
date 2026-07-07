# Captured Install for external installers

When users already have an external installer workflow such as `npx`, SkillYard should not force them into a staging model. Instead, it can run the real command through a Captured Install flow: snapshot Host skill directories before the command, execute the command, snapshot again, and convert newly created or changed Host entries into managed Library state.

For npm-based commands, SkillYard records an Install Receipt by resolving package metadata such as package name, version, repository URL, tarball URL, and integrity. This receipt can create a Package Source Tree or, when repository metadata points to a matching skill, upgrade to a Git Source Tree. If no reliable receipt exists, the result remains a Source Candidate or uses AI Assist as Provenance Inference rather than inventing a stable Library Identity.
