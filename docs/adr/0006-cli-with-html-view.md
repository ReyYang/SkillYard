# CLI with HTML View

We will make the first product shape a CLI plus local HTML View, not a native Mac GUI. The CLI owns state changes and filesystem operations, while the HTML View provides a readable local interface for Library browsing, doctor findings, update impact, and conflict choices. This keeps the risky symlink and source-tree behavior testable from commands while still giving non-terminal users a visual way to understand the Library.
