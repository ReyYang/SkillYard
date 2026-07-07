# Symlink-first exposures with snapshot fallback

We will expose Library skills to agent hosts through symlinks by default, because the product's core value is one local Source Tree updating multiple Host entries while keeping provenance clear. We will also support snapshot exposure as a compatibility fallback for hosts, permissions, or filesystem environments where symlinks are unsafe or unsupported, but snapshot mode must be visible in doctor/status because it can drift from the Library.
