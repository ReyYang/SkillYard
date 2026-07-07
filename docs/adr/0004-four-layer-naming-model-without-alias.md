# Four-layer naming model without Alias

We will not expose Alias as a first-class product concept. The naming model has four visible layers: Library Identity for management, Skill Name from `SKILL.md`, Display Label for UI readability, and Host Entry Name for the actual directory exposed to a Host. Host Entry Name overrides exist only as internal conflict-resolution state, which keeps ordinary users focused on the skill identity while still allowing safe resolution when two skills would occupy the same host entry.
