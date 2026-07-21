# SkillYard

> 本文件只统一 SkillYard 1.0 的领域术语，不新增产品行为。产品承诺以 [1.0 产品契约](docs/1.0-product-contract.md) 为准，详细执行规则以 [1.0 接管与管理权设计](docs/1.0-management-authority.md) 为准。

SkillYard 1.0 is a macOS desktop application that takes over user-selected AI agent Skills and manages their complete local lifecycle while preserving upstream provenance and read-only ownership boundaries.

## Language

**Personal Local Build**:
The SkillYard 1.0 delivery boundary: the product owner builds and runs `SkillYard.app` on one personal Apple silicon Mac. Version 1.0 has no public installer, GitHub Release, Developer ID, notarization, App Store submission, or Application Update flow.
_Avoid_: public distribution, paid Apple signing, release feed, multi-user installation support

**Application Architecture**:
The SkillYard 1.0 implementation boundary: a Tauri 2 desktop shell loads bundled TypeScript web assets in the system WKWebView and communicates through typed, task-specific commands with a small Rust Lifecycle Core. The production app has no localhost server, bundled Chromium runtime, Python runtime, or Python sidecar.
_Avoid_: SwiftUI application, Electron runtime, local web service, general-purpose IPC backend

**Single Application Surface**:
The SkillYard 1.0 delivery contract that `SkillYard.app` is the only user-facing executable and the only product entry point. The local 1.0 build includes no CLI, headless mode, daemon, localhost API, public Rust API, or separately installable recovery tool; every supported lifecycle action and recovery state is handled inside the app.
_Avoid_: `skillyard` command, automation server, public core library, secondary recovery interface

**Lifecycle Core**:
The narrow Rust authority that owns SQLite, Source acquisition, local scanning, filesystem validation, Bundle transaction planning and execution, symbolic links, and transaction recovery. The TypeScript UI cannot access the filesystem, database, or shell directly and cannot bypass a confirmed Plan.
_Avoid_: frontend filesystem access, generic SQL bridge, shell plugin, business logic duplicated in TypeScript

**Supported Platform**:
The complete operating-system and processor support promise for SkillYard 1.0: Apple silicon (`arm64`) running macOS 14 Sonoma or later. Intel (`x86_64`), Universal 2, and macOS 13 or earlier are outside the 1.0 acceptance matrix.
_Avoid_: every Mac, Rosetta support, build-host architecture

**Manual App Replacement**:
The only SkillYard 1.0 application upgrade path: the product owner rebuilds `SkillYard.app` on the same personal Mac and replaces the app manually. Central Store, SQLite, Current Content, Journals, and Mounts remain untouched.
_Avoid_: Application Update, updater feed, installer download, Developer ID verification

**Zero Telemetry**:
The SkillYard 1.0 privacy contract that analytics, identifiers, crash reports, managed inventory, Source URLs, Skill names, local paths, and SQLite state never leave the Mac. Explicitly user-triggered network requests for Source, catalog, search, Update Check, or Bundle Update operations are functional requests and carry no analytics payload. Local SQLite transaction state and Filesystem Transaction Journals are correctness mechanisms, not telemetry.
_Avoid_: telemetry opt-out, automatic crash upload, cloud sync

**Skill**:
A Host-consumable instruction unit defined by one file named exactly `SKILL.md`; that file's parent directory is the Skill root. Every discovered `SKILL.md` defines one Skill Member inside the supplied Bundle, without requiring a Bundle manifest.
_Avoid_: Bundle, Local Installation

**Skill Name**:
The canonical `name` declared in valid `SKILL.md` YAML frontmatter. In version 1.0 it is 1-64 lowercase ASCII letters, digits, or hyphens, has no leading, trailing, or consecutive hyphens, and exactly matches the Skill root directory name. Directory, Bundle, plugin, or Host UI context added around it does not rename the Skill.
_Avoid_: Host Presentation Label, Skill Identity

**Host Presentation Label**:
A Host-owned UI label that displays a Skill with contextual prefixes such as a plugin, Bundle, or directory group. It does not change Skill Name, Skill Identity, managed content, or Mount.
_Avoid_: Skill Name, rename, managed alias

**Bundle Display Name**:
The simple short name fixed when the Bundle is created, usually derived from its Source or installation input, such as a repository slug, package name, or archive or directory basename. It remains available if the Source is later deleted. SkillYard 1.0 does not support customizing it.
_Avoid_: user-defined label, AI-generated name, Host Presentation Label

**SkillYard Presentation Label**:
A SkillYard-owned UI label composed as `Bundle Display Name: Skill Name` for lists and search results. It provides consistent context across Hosts without becoming Skill Name or Skill Identity.
_Avoid_: Host Presentation Label, canonical name, managed alias

**Source**:
An upstream entry from which SkillYard can discover, acquire, or update Skills, independent of any local installation. A Source can be a Git repository, registry artifact, deterministic URL, ZIP input, or explicitly registered editable local tree. Different discovery routes to the same upstream do not create separate Source identities. A Source may exist without an installed Bundle but can be linked to at most one Bundle on the Mac; deleting it never deletes local managed content. Version 1.0 does not directly replace the Source linked to a Bundle: the user deletes the old Source, then adds and associates the new one through the ordinary flow.
_Avoid_: Bundle, Installation Chain, Local Installation

**Editable Local Source**:
An explicitly user-owned working directory, with or without an upstream, where personal Skill content is edited. SkillYard adopts confirmed content into its managed copy and never mounts the editable directory directly.
_Avoid_: Managed Bundle Directory, Current Content, Project-managed Installation

**Unavailable Source**:
A registered Source that SkillYard cannot currently access or resolve. It blocks new discovery, capture, or update from that Source but does not change existing Current Content, Mounts, or Local Lifecycle Authority.
_Avoid_: Upstream-removed Skill, Unmounted Installation

**Stale Source Catalog**:
The last successfully fetched member catalog retained after a later Source reload fails. SkillYard shows the catalog and its successful fetch time for reference, marks the reload failure, and does not treat the failed response as an empty or newer catalog. The stale catalog cannot authorize installation or Update until a fresh Source fetch succeeds.
_Avoid_: current catalog, cached Skill content, empty Source

**Safe Skill Content**:
A candidate Skill root containing only ordinary directories and regular files. Symbolic links, hard-linked files, FIFOs, sockets, device nodes, and other special filesystem entries invalidate the Skill before it can enter Current Content. Archive paths must also remain inside SkillYard's staging root after normalization.
_Avoid_: malware scan, Mount, executable-file ban

**Source Resource Limit**:
A fixed, non-configurable ceiling enforced while SkillYard acquires or expands Source content. Version 1.0 allows at most 100 MiB received, 20,000 archive entries, 512 MiB of expanded regular-file content, and 100 MiB for one regular file. Exceeding any limit rejects the operation before Current Content changes and cannot be overridden by the user.
_Avoid_: storage quota, Bundle size target, user preference

**Publisher**:
The person or organization that publishes a Skill or Source. Publisher describes content provenance, not permission to mutate a Local Installation.
_Avoid_: local manager, Local Lifecycle Authority

**Installation Chain**:
The ordered, evidence-backed sequence of commands, tools, registries, and Sources that produced or can reproduce local content. It remains provenance after Takeover; it does not retain control of the active managed copy.
_Avoid_: Source, Upstream Adapter

**Bundle**:
A SkillYard-managed local installed group created by direct installation or Takeover. It contains the Skill Members currently installed in the Central Store and may be linked to at most one Source for discovery and updates. The association is optional and one-to-one: a Source may exist without a Bundle, and a Bundle may continue without a Source. If another Source-less Bundle is later associated with the same already-linked Source, its members and Mounts converge into the existing Bundle through a confirmed recoverable transaction instead of creating a second linked Bundle. Direct installation selects every available Source member by default; Takeover preserves the members already installed locally; an explicit Bundle Update installs every validated current member fetched from the linked Source. During Unknown Provenance Takeover, multiple Skills share a Bundle only when deterministic Management Evidence proves that they came from the same installation group; otherwise each Skill creates its own Bundle.
_Avoid_: Source, upstream catalog, read-only inventory group

**Skill Member**:
A locally installed Skill contained in a Bundle. A linked Source may list additional available Skills, but they do not become Bundle members until installed. A Skill Member can be mounted independently, but version 1.0 does not delete one installed member independently. A later Bundle Update installs every valid current Skill from the linked Source.
_Avoid_: Bundle, copied Skill

**Member Selection**:
The current set of installed Skill Members in a Bundle. Direct installation selects every available Source member by default but allows changes before confirmation; when the user leaves a known Source catalog partially selected, SkillYard warns that cross-Skill dependencies are not checked and allows the user to continue at their own risk. Takeover preserves the locally installed subset. A successful Bundle Update adds every validated current Source member as an unmounted installation when it was not already installed; an installed member that has since been removed upstream remains selected and keeps its existing Mounts. Member Selection cannot be reduced through member-level deletion in version 1.0.
_Avoid_: Source catalog, latest upstream member catalog

**Cascading Delete**:
A twice-confirmed destructive operation entered from a Bundle. It removes every SkillYard-managed Skill Member in that local group, including Mounts, Member Selection, the Managed Bundle Directory, and Current Content, followed by the Bundle record. Its linked Source record remains available for a future installation. It never deletes upstream content or a user-owned editable Source directory.
_Avoid_: delete Source, disable Source, remove Mount, member-level Skill deletion

**Skill Identity**:
The stable continuity of one Skill Member across upstream name or path changes. It is established only by a stable upstream identifier, explicit metadata, reviewed Adapter mapping, or an explicit user-confirmed association.
_Avoid_: Skill Name, Host Presentation Label, directory path, content similarity

**Rename Candidate**:
A suspected connection between an Upstream-removed Skill and a newly discovered Skill Member based only on weak evidence such as name, path, description, or content similarity. It does not change either member until the user confirms the association.
_Avoid_: confirmed Skill Identity, automatic rename

**Upstream-removed Skill**:
An installed Skill Member that is absent from the latest successfully fetched catalog of its linked Source. SkillYard preserves that member inside Current Content together with its Member Selection and existing Mounts; it is not updated or silently deleted. The user may remove its Mounts, while its managed content remains until the Bundle is deleted.
_Avoid_: unmounted Skill, unavailable Bundle

**Upstream Update Marker**:
A release, tag, commit, digest, or other identity reported by a Source and used only to decide whether an Update is available for a linked Bundle. For a GitHub Source in version 1.0, the exact commit resolved from its Tracked Ref is authoritative. It is not a SkillYard local version or rollback state.
_Avoid_: Local Bundle Version, Current Content

**Tracked Ref**:
The single Git branch or other ref that a GitHub Source uses for member discovery, Update Check, and future updates. An explicit supported ref is used when supplied; otherwise SkillYard resolves and persists the repository's actual default branch name rather than storing a dynamic default marker. Another discovery route cannot change it without explicit user confirmation, and changing it does not itself replace Current Content.
_Avoid_: Bundle identity, Upstream Update Marker, Current Content

**Current Content**:
The one authoritative managed content tree of a complete local Bundle. It contains every currently installed Skill Member, including retained Upstream-removed or non-corresponding members. A candidate tree and the previous tree may coexist only while a transaction is being validated or recovered. After success, obsolete content is removed rather than retained as a selectable version.
_Avoid_: version history, rollback point, editable working tree

**Current Link**:
The single SkillYard-controlled symbolic link named `current` inside one Managed Bundle Directory. It points to the complete Current Content tree and is atomically replaced only after the complete candidate Bundle has been validated. Every Mount targets its member's stable managed path below this link, so one replacement activates the whole Bundle.
_Avoid_: Mount, Host path, version selector

**Filesystem Transaction Journal**:
A durable local record of a lifecycle operation's filesystem plan, persistent phase, recovery paths, and completed steps. It makes every step safe to retry after interruption and complements, rather than being replaced by, the SQLite transaction record.
_Avoid_: SQLite transaction, temporary log, user-visible rollback history

**Local Installation**:
A physical materialization of one selected Skill Member on the Mac. Discovery or Bundle membership alone does not make a Local Installation managed by SkillYard.
_Avoid_: Skill, Source

**Takeover Candidate**:
A discovered Local Installation that may be eligible for SkillYard management but has not yet been authorized. It remains unchanged until the user confirms a Takeover Plan.
_Avoid_: SkillYard-managed Installation

**Takeover Plan**:
An exact, reviewable proposal for registering or moving content, preserving temporary transaction-recovery content, replacing Host-visible paths with Mounts, and verifying the result.
_Avoid_: silent migration, scan result

**Takeover**:
The explicit transfer of complete local lifecycle responsibility to SkillYard. Takeover registers content in place when it already satisfies the Central Store rules, and otherwise uses a recoverable migration.
_Avoid_: import, scan

**Central Store**:
The SkillYard-controlled persistent user-content root fixed at `~/Library/Application Support/SkillYard/` in version 1.0. Managed Skill content stored here is the user's actual master copy, not a cache or disposable application data. The root also contains SQLite state, activation data, transaction journals, and isolated temporary areas. Users can reveal it in Finder but cannot relocate it in version 1.0.
_Avoid_: Host directory, Source, cache, disposable app data

**Central Store Notice**:
The generated `SKILLYARD-INFO.md` kept at the Central Store root. It warns that the directory contains the user's actual managed Skill content and lists known Sources and Host Mount locations so the storage cannot be mistaken for a disposable cache. It is human-readable context, not a replacement for SQLite or transaction journals.
_Avoid_: cache marker, database, recovery journal

**App Reset**:
A non-destructive reset that clears only preferences, window state, and cache. It never removes SQLite, the Central Store, Current Content, `current`, Mounts, or lifecycle transaction records.
_Avoid_: Bundle deletion, Central Store deletion, complete removal

**Managed Bundle Directory**:
The SkillYard-controlled home of one installed Bundle inside the Central Store. It contains one `current` link and complete candidate content trees; each installed Skill Member has a stable managed path inside the tree selected by `current`. The directory exists independently of every Host path where its members are used.
_Avoid_: Host directory, Mount, staging area

**Local Lifecycle Authority**:
The party responsible for the local master copy, Mounts, and removal of one Local Installation. Every Skill Member inside a Bundle has exactly one such authority: SkillYard. Source availability controls whether upstream update is possible, not who manages the local installation.
_Avoid_: Publisher, Installation Chain

**Managed Lifecycle**:
A local Skill Member contract covering installation or Takeover into its Bundle's Current Content, Mount management, and managed-content removal through Bundle deletion. When a Source is linked, the contract also provides whole-Bundle Source-backed Update. Deleting or losing the Source disables every update path for that Bundle but does not change Local Lifecycle Authority. Member-level uninstall, Source enablement state, rollback, and retained-version switching are not part of the version 1.0 contract.
_Avoid_: Source availability, read-only inventory

**SkillYard-managed Installation**:
A Skill Member installation inside a Bundle whose content within Current Content, Mounts, and Bundle-scoped removal are controlled by SkillYard. It remains managed when no Source is linked, although every update action is unavailable.
_Avoid_: Takeover Candidate, Host-managed Installation

**Unmounted Installation**:
A SkillYard-managed Installation with Current Content but no Mount. It remains installed and manageable but is not exposed to any Host.
_Avoid_: deleted Skill, unavailable Skill

**Project**:
A local project root explicitly added by the user or confirmed while taking over an existing project-level Skill. Only a registered Project can receive a new project Mount; SkillYard does not build this list through whole-disk scanning. Version 1.0 assumes registered Projects remain locally available on the current Mac.
_Avoid_: auto-discovered repository, Project-managed Installation, Mount Scope

**Mount**:
A Host- or project-visible symbolic link whose target is one installed Skill Member's stable managed path below its Bundle's Current Link. It exposes that member without creating another master copy and does not need to be rewritten when the Bundle's Current Link switches.
_Avoid_: Bundle activation, Local Installation

**Mount Scope**:
The visibility boundary of a Mount within one Host. A Mount Scope is either global for the user or project-specific for one project root.
_Avoid_: management authority, Project-managed Installation

**Scope Conflict**:
A condition where the same Skill would have both a global Mount and one or more project Mounts in the same Host. Project Mounts for distinct project roots can coexist without a Scope Conflict.
_Avoid_: Mount Conflict, cross-Host Mounts

**Mount Conflict**:
A condition where the Host target path for a planned Mount is occupied by a different or unverified entry. An existing symlink verified to expose the same managed Skill is not a Mount Conflict.
_Avoid_: Host Presentation Label conflict, automatic rename

**Batch Mount**:
A Bundle-level convenience action that prepares multiple independent Skill Mounts in one recoverable transaction. SkillYard preflights every selected target and requires the user to exclude or resolve Mount Conflicts before confirmation; after the transaction starts, every confirmed Mount succeeds or the operation restores its starting state. It does not create one Bundle-level Mount.
_Avoid_: partial success, Bundle-level Mount, automatic conflict replacement

**Mount Drift**:
A recorded SkillYard-managed Mount whose filesystem path is missing, is no longer a symlink, or points somewhere other than its expected managed target. The record remains until the user explicitly repairs or removes it.
_Avoid_: Unmounted Installation, Mount Conflict, automatic repair

**Local Refresh**:
A user-triggered, local-only rescan of the configured directories for Supported Apps and registered Projects, including their read-only evidence locations. It updates Inventory and Mount health so externally installed, removed, or changed Skills become visible, but it never queries an upstream Source or authorizes Takeover.
_Avoid_: Update Check, Source catalog reload, automatic Takeover

**Update Check**:
A user-triggered, read-only query to a registered Source linked to a Bundle that determines whether a newer Upstream Update Marker is available. A Bundle without a linked Source is not checked and shows no update Source. Update Check does not rescan local installation directories, fetch update content, replace Current Content, or perform an Update.
_Avoid_: Local Refresh, Update, background polling

**Batch Update**:
A user-confirmed action that coordinates updates for multiple updateable Bundles after one combined impact preview. Every participating Bundle installs the complete validated current catalog fetched from its linked Source. Each Bundle still executes as an independent recoverable transaction, so one failure does not block or undo successful updates for other Bundles.
_Avoid_: automatic update, one cross-Bundle transaction

**Manual Replacement Update**:
A user-triggered update for a Bundle linked to a registered ZIP or directly downloaded file Source that has no checkable Git marker. The user supplies a replacement artifact to the existing Bundle; SkillYard stages and validates the complete artifact before installing all of its valid Skill Members. It is Source-backed update capability, not an automatic Update Check or a replacement path for a Source-less Bundle.
_Avoid_: reinstall, background download, GitHub Update Check

**Upstream Adapter**:
A built-in, reviewed integration that discovers, checks, or fetches upstream content into a staging area from a directly supported Source such as GitHub, `skills.sh`, or a deterministic URL. Version 1.0 does not execute external CLI installers through an Upstream Adapter. After Takeover, an Upstream Adapter never controls or mutates the active managed copy.
_Avoid_: local manager, external command runner, arbitrary shell executor

**Supported App**:
A user-visible Agent application included in SkillYard's fixed built-in support table. Its internal configuration contains a display name, one fixed app-specific global Mount root, one fixed app-specific project Mount root, read-only compatibility directories, path-overlap facts, and an optional installation detector. Version 1.0 writes Codex Mounts to `~/.codex/skills` or `<project>/.codex/skills`, Claude Code Mounts to `~/.claude/skills` or `<project>/.claude/skills`, and GitHub Copilot Mounts to `~/.copilot/skills` or `<project>/.github/skills`. Selecting an app authorizes SkillYard to create a Mount in that fixed root; it does not guarantee that another app cannot scan the path. Shared compatibility roots such as `~/.agents/skills` are scanned for Inventory and Takeover but never receive a new SkillYard Mount. Multiple readable directories remain one Supported App. Most Supported Apps require path configuration only; custom logic is exceptional.
_Avoid_: auto-discovered app, Host Family, Runtime Surface, Upstream Adapter

**Management Evidence**:
A local fact used to reconstruct Source, Installation Chain, Bundle membership, Skill Identity, upstream update identity, or filesystem topology and to produce a Takeover Plan. Evidence informs provenance and safety; it does not grant an external tool authority over an active managed copy. A shared Host Skill root, parent directory, similar name, or nearby placement is not by itself evidence that distinct Skills belong to one Bundle.
_Avoid_: Source, confidence guess

**Unknown Provenance**:
A displayed state for a discovered Skill whose Source or Installation Chain cannot be confirmed from deterministic Management Evidence. It does not block Takeover: the current local content can become Current Content in a SkillYard-managed Bundle, while update remains unavailable until a Source is associated. SkillYard preserves known facts and asks for explicit user input instead of generating an AI guess.
_Avoid_: Unavailable Source, inferred Source, confirmed provenance

**Host-managed Installation**:
A Local Installation supplied and controlled by a Host or application, such as a built-in Skill, plugin Skill, or application bundle. SkillYard inventories it read-only and delegates lifecycle actions to the Host.
_Avoid_: SkillYard-managed Installation, Takeover Candidate

**Project-managed Installation**:
A Local Installation intentionally maintained in a project repository. Its lifecycle follows that repository; SkillYard inventories it read-only and directs the user to the project.
_Avoid_: project-scoped Mount, SkillYard-managed Installation

**Unmanaged Installation**:
A discovered Local Installation that is neither controlled by SkillYard nor classified as Host-managed or Project-managed. It may become a Takeover Candidate once its local content boundary is understood; its provenance may remain unknown.
_Avoid_: unknown Source, SkillYard-managed Installation
