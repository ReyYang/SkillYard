# `gh skill` as Discovery Provider

We will integrate `gh skill` as an external Discovery Provider for searching and importing public skills, but it will not own the local Library state. Source Trees, Library Entries, Exposures, doctor checks, and update impact remain managed by the SQLite State File and our own application layer. This lets the product benefit from GitHub's skill ecosystem without giving up the Source Tree and symlink model.
