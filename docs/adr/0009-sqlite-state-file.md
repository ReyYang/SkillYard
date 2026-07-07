# SQLite State File

We will use SQLite as the first State File format. The product needs relationship-heavy queries across Source Trees, Library Entries, Exposures, Host entries, health findings, conflicts, and update impact, so SQLite is a better internal source of truth than JSON or YAML. The CLI can still export JSON for debugging and portability, but JSON is not the primary state format.
