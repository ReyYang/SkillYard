# Local Server writes use Plan, Chinese confirmation, and Apply

The Local Server may execute write operations such as exposing a skill, removing an Exposure, updating a Source Tree, or resolving a Host Entry Conflict. Every write operation must first produce a Plan, show the user a Simplified Chinese confirmation screen, and then execute through the same Apply layer used by the CLI. The UI must not mutate Host directories, Source Trees, or the State File directly.
