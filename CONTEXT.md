# SkillYard

A Mac-first local manager for AI agent skills. It keeps a unified skill library separate from the agent-specific names and locations where skills are exposed.

## Language

**Library**:
The local source tree that stores skill sources from GitHub repositories, local personal skills, and bundled sources. It is the canonical local place a skill is managed from.
_Avoid_: Install folder, download cache, source cache

**Library Identity**:
The stable management identity for a skill inside the Library, written as `namespace/name`, such as `mattpocock/review`, `lark/lark-doc`, or `openai/hatch-pet`. It is used for management, deduplication, provenance, and update decisions.
_Avoid_: Host Entry Name, Display Label, internal key

**Namespace**:
The first segment of a Library Identity. Namespace is inferred from source metadata by default and can be explicitly overridden when the product identity should differ from the upstream owner or repository name.
_Avoid_: Host Entry Name, Display Label, folder name

**Skill Name**:
The short name declared by the skill itself in `SKILL.md`, such as `review`. It is the default Host Entry Name when exposing a skill.
_Avoid_: Library Identity, Display Label

**Display Label**:
A human-readable label shown in the UI, such as `Matt Pocock: Review`, `Lark: Doc`, or `OpenAI: Hatch Pet`. It can be derived from Library Identity and Skill Name, and can be customized without changing management identity or host exposure.
_Avoid_: Library Identity, Host Entry Name

**Library Entry**:
One manageable skill inside the Library. A Library Entry has a Library Identity and points to a `SKILL.md` path inside a Source Tree.
_Avoid_: Exposure, active skill, installed skill

**Exposure**:
A declaration that a Library skill is made available to a specific Host and scope. Exposure is the relationship between one Library Entry and one Host Entry Name.
_Avoid_: Install, copy, subscription

**Exposure Scope**:
The level at which an Exposure is written for a Host. First-version scopes are `user` and `project`; project scope records a `projectRoot`.
_Avoid_: Skill type, Library type

**Exposure Mode**:
The mechanism an Exposure uses to make a Library skill visible to a Host. The default mode is `symlink`; `snapshot` is a compatibility fallback.
_Avoid_: Install type, source type

**Host Entry Name**:
The actual directory name used when an Exposure is written into a Host's skill directory. It defaults to Skill Name; conflict resolution can choose not to expose, use a recommended name, use a custom name, or replace an existing entry.
_Avoid_: Library Identity, Namespace, Display Label

**Host Entry Name Override**:
An internal field used only when an Exposure cannot use the default Skill Name. It should not be presented as a first-class user concept.
_Avoid_: Alias

**Host Entry Conflict**:
A pre-write condition where a Host and scope already contain the Host Entry Name needed by a different Library Entry. The tool refuses to write until the user explicitly chooses a resolution.
_Avoid_: Blocked Exposure, background reconciliation

**Unmanaged Host Entry**:
A skill entry found in a Host directory that is not represented by a SkillYard Exposure. Doctor reports it and offers choices to keep it unmanaged, import it into the Library, or replace it with a managed Exposure.
_Avoid_: Broken entry, project skill

**Conflict Prompt**:
The Chinese user-facing prompt shown when a Host Entry Conflict occurs. It explains the existing entry, the new entry, and explicit choices such as using a recommended name, replacing the existing entry, or skipping the exposure.
_Avoid_: Silent rename, automatic overwrite

**Internal Key**:
The private fallback identifier used when Library Identity is not sufficient, such as rare duplicate names within the same namespace or special source layouts like `.system` and `.curated`. It can include Source Tree ID and skill path.
_Avoid_: Library Identity, Display Label

**Source Tree**:
The local checked-out or materialized source for one upstream repository, local personal collection, or managed bundle. One Source Tree can contain multiple Library Entries.
_Avoid_: Agent directory, active skill folder

**Dirty Source Tree**:
A Source Tree with local uncommitted changes. Dirty Source Trees block updates until the user chooses how to proceed.
_Avoid_: Update candidate, clean source

**Update Impact**:
The set of Library Entries and Exposures that would be affected by updating a Source Tree from its current revision to a newer revision. It must be shown before applying the update.
_Avoid_: Changelog, release notes

**Host**:
An AI agent application or CLI that can consume skills, such as Codex, Claude Code, Cursor, or GitHub Copilot. A Host has one or more supported skill directory layouts.
_Avoid_: Agent when referring to the human-facing product integration

**Host Adapter**:
The host-specific mapping that knows where a Host stores skills for each scope and whether the Host supports symlink or snapshot Exposure Mode. First-version built-in Host Adapters are Codex, Claude Code, Cursor, and GitHub Copilot.
_Avoid_: Plugin when referring to built-in host support

**Discovery Provider**:
An external source used to find candidate skills before importing them into the Library. `gh skill` is a Discovery Provider, not the Library's state manager.
_Avoid_: State File, Source Tree manager

**AI Assist**:
An optional, explicit feature that explains skills, summarizes findings, or helps infer unknown provenance. AI Assist can suggest and annotate, but it must not mutate the State File, Source Trees, Exposures, or Host directories.
_Avoid_: Core dependency, automatic decision-maker

**Provenance Inference**:
An AI-assisted investigation used when a skill's source is unknown. It compares local skill content, metadata, filenames, and public sources to suggest likely origins with evidence and confidence, without recording the guess as confirmed provenance.
_Avoid_: Provenance, source of truth

**Captured Install**:
A flow where SkillYard runs an external installer command, snapshots Host skill directories before and after, and converts newly created Host entries into managed Library state. The external installer performs the download; SkillYard captures the result and records provenance.
_Avoid_: Delegated install, unmanaged install

**Install Receipt**:
The recorded evidence from a Captured Install, including the command, package or provider identity, resolved version, source metadata, and Host entries created or changed. It is used to infer Library Identity and Source Tree provenance.
_Avoid_: Event, provenance guess

**Package Source Tree**:
A Source Tree materialized from a package registry artifact such as an npm tarball. It records package name, version, registry metadata, tarball URL, integrity, and repository metadata when available.
_Avoid_: Git Source Tree, adopted source

**Command Surface**:
The first-version CLI command set: `init`, `import`, `expose`, `doctor`, `update`, and `serve`. It defines the product's initial operational boundary.
_Avoid_: Full command catalog, plugin API

**Snapshot**:
A copied materialization of a Library skill used when a Host or filesystem environment cannot safely use symlinks. Snapshot is not the default because it requires explicit update checks to stay current.
_Avoid_: Primary install, cache

**HTML View**:
A local browser-readable interface served by the Local Server to inspect Library state, update impact, health findings, and conflict choices. It is a UI surface, not the source of truth.
_Avoid_: Mac app, hosted service, registry database

**Local Server**:
A localhost-only process started by the CLI that serves the HTML View and calls the same state and filesystem operations as the CLI. It must not become a hosted service or a separate source of truth.
_Avoid_: Cloud service, separate backend, hosted registry

**Plan**:
A pre-execution description of filesystem, State File, and Host changes that a command or Local Server action intends to make. Every write action must produce a Plan before it can be applied.
_Avoid_: Log, dry-run output only

**Apply**:
The execution step that performs a previously reviewed Plan. Apply is the only layer allowed to mutate Source Trees, Exposures, Host directories, or the State File.
_Avoid_: Direct UI write, ad hoc script mutation

**State File**:
The durable local SQLite database that records Source Trees, Library Entries, Exposures, and user choices. CLI commands and the Local Server both write through the same application layer.
_Avoid_: HTML View, generated report, server cache

**Event**:
A minimal historical record of a user-visible state change, such as adding a Source Tree, creating or removing an Exposure, updating a Source Tree, or resolving a Host Entry Conflict. Events are for traceability, not a full audit system.
_Avoid_: Audit log, telemetry, analytics event
