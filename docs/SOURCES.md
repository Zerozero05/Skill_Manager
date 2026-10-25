# Sources and dependencies

Skill Manager is an independent implementation. No Skillshare or Skills Manager source code was copied into the application.

v0.1.4 independently adds collapsible native file groups, unified vertical scrolling, checked byte/version change notices with deferred focus checks, local declared-version display refresh, and a lower-luminance warm gray-green eye theme. It introduces no new external source, copied code or dependency. Ninety Rust tests, eight Node tests, TypeScript/Vite, formatting/Clippy, optimized Windows/NSIS builds, 14 native UI regressions and six focused behaviors passed. Focused tests use private-source reads and installations/edits only under owned work fixtures; no remote writes. The installer wizard and real model invocation remain unverified.

v0.1.3 independently adds distinct skill/repository counts, direct source navigation, bounded sidebar activity history, required/optional runtime classification, verified automatic association and observation-based change summaries, plus bounded in-memory reuse of immutable remote bytes within one check. It introduces no new external reference, copied code or dependency. Existing sources and license notices remain applicable. This unpublished local candidate passed 88 Rust and 8 list/link tests, static/frontend checks, 14 native UI behaviors with synthetic Skills, and 4 private-source read-only/association checks. Optimized Windows and NSIS artifacts were built; the optimized app's startup/data path/debug-port checks passed. The installer wizard, real repository writes and model invocation remain unverified.

v0.1.2 implements the compact library, search/filter/pagination logic, selected-item checks and detail drawer within the existing React/Tauri application. It introduces no new external design/code reference or dependency. Existing method references and dependency licenses below remain applicable.

v0.1.1 uses the [official GitHub fine-grained token URL parameters](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/managing-your-personal-access-tokens#pre-filling-fine-grained-personal-access-token-details-using-url-parameters) to prefill the creation form. It does not create a token through the GitHub connector or reuse its credentials. The URL permission correction follows the installed Tauri opener plugin's documented scoped command configuration; only explicit HTTP(S) links are allowed.

| Source | Actual use |
| --- | --- |
| [Agent Skills specification](https://agentskills.io/specification) | SKILL.md frontmatter and optional resource layout |
| [Skillshare](https://github.com/runkids/skillshare) | Method reference: one canonical runtime source and explicit client links |
| [Skills Manager](https://github.com/xingkongliang/skills-manager) | Method reference: skill list, details and source/status organization |
| [Tauri 2](https://v2.tauri.app/) | Desktop application framework and dialog/opener plugins |
| [GitHub REST API](https://docs.github.com/en/rest) | Authenticated repository content, trees, commits and references |

Runtime/build dependencies are declared in package.json, pnpm-lock.yaml, Cargo.toml and Cargo.lock. These lockfiles record the actual resolved versions. Their upstream license terms remain applicable; a distribution license inventory is prepared with the candidate artifacts.

`THIRD_PARTY_NOTICES.txt` accompanies both installer and portable distributions. The v0.1.9 inventory includes 689 locked packages (516 Rust and 173 npm), nested notices, Rust standard-library notices, and Microsoft WebView2 SDK 1.0.3800.47 LICENSE/NOTICE. The SDK loader binaries were checked against the matching crate's files. The inventory intentionally includes build/test/other-platform entries; entries without original notice text remain explicitly listed. Resolved cross-platform packages do not all link into the Windows executable. This does not choose the application's own open-source license.

The local build environment used a private adaptation of [mmozeiko's portable MSVC helper](https://gist.github.com/mmozeiko/7f3162ec2988e81e56d5c4e22cde9977), revision `49b58850aa5e5c1e65f090d4651ee76607883e11`, solely to obtain hash-checked Microsoft packages. [lessmsi 2.12.9](https://github.com/activescott/lessmsi/releases/tag/v2.12.9) (MIT) extracts SDK MSI files without installing them. Build helpers and Microsoft tools are not bundled with the application. Microsoft compiler/SDK terms apply separately. No system-installed Visual Studio is required by that local setup.

## v0.1.5 local refinement
Categories, shortcuts and sidebar interactions are implemented within the existing codebase. No new external code adaptation, package or downloaded dependency was introduced. Existing architectural references and license notices remain applicable.

## v0.1.6 local interaction refinement
Selected-row dragging, floating menus, scope explanations, extra-file status, category ordering, keyboard selection and favorites were implemented in the existing codebase. No new package, downloaded dependency or external code adaptation was introduced.

## v0.1.7 file and information refinement
Merged file availability, transient attachment selection, fixed-commit preview/export/download and source dependency/verification presentation reuse this application's existing GitHub, transaction, Markdown and ZIP facilities. No new package, downloaded dependency or external code adaptation was introduced. Source statements remain distinct from local invocation evidence.

## v0.1.8 transfer and verification refinement
First upload, independent fixed-commit verification, optional-file receipts/restoration, existing-client adoption and structured runtime checks are implemented within this codebase. No new dependency, download or external code adaptation was introduced. Codex directory-link behavior was checked against the [official Skill documentation](https://learn.chatgpt.com/docs/build-skills). HTTP tests use an isolated local fake GitHub service, not a real repository. Windows object-lock deletion, dependency classification and transport-race fixes are independent maintenance changes; existing dependency licenses continue to apply.

## v0.1.9 tasks, network and external resources

The task scheduler, persistent navigation history, client layout/batch controls, external resource classification/bindings and ZIP-only reference conversion are independent changes within this application. No new third-party application implementation was copied or adapted. The canonical Skill and external resources stay at their existing locations; test fixtures and development records are distinct from application runtime output.

Transport reuse and proxy support use the existing [reqwest 0.12.28 documentation](https://docs.rs/reqwest/0.12.28/reqwest/). Conditional reads, foreground request scheduling and rate-limit behavior refer to [GitHub's REST best practices](https://docs.github.com/en/rest/using-the-rest-api/best-practices-for-using-the-rest-api). Standard Git blob length/header/object identity follows [Git Internals: Git Objects](https://git-scm.com/book/en/v2/Git-Internals-Git-Objects); SHA-1 is used for Git object compatibility, alongside existing SHA-256 local/content evidence, rather than replacing update safety.

Cargo adds sha1 0.10.7 for Git blob identity and enables reqwest system-proxy/SOCKS features. The lockfile consequently adds windows-registry 0.6.1, system-configuration 0.7.0, system-configuration-sys 0.6.0 and core-foundation 0.9.4. These are normal upstream library dependencies, including other-platform entries; no downloaded source is executed as an external helper. Their original license notices are included in the refreshed distribution inventory. No new frontend package is introduced.

Synthetic HTTP and filesystem checks exercise cache identity/eviction/coalescing, conditional reads, GET-only retries, cancellation, independent fresh upload readback, external originals remaining unchanged and only the chosen ZIP being added. Such tests do not verify a user's actual VPN route, private GitHub writes, model invocation, installer interaction or another computer. Final candidate-wide verification is recorded separately after execution; no benchmark speed claim is inferred from cache design.

## v0.2.0 workspace and dual-source resources

Main-window tabs, normal native-window detach/return, scoped collapsible outlines, in-memory pane drafts and coordinated close behavior are independent changes in the existing React/Tauri application. They reuse its existing window APIs, single live Manager, task service, SQLite, credential store and network transport. No floating-overlay window framework or second application state store was introduced.

Dual-source external evidence, exact fixed-commit repository mappings, explicit ambiguous candidates, role/reference separation and batch management reuse the existing GitHub readers and semantic reference checks. Mixed necessary-resource export reuses the existing ZIP writer and provenance contract. Final package preflight applies the existing private-resource filename/path guard to logical and actual local/remote paths without adding a new content-scanning rule. Original Skill/external files remain unchanged; export resource output is only the chosen ZIP, while normal Manager state retains its existing independent database location.

No new dependency, downloaded package or third-party project code was introduced for v0.2.0. Existing lockfiles and distribution license notices remain applicable; the references above retain their original scope. Synthetic filesystem/HTTP regressions cover reference classification, fixed-commit lookup, optional choices, partial batch failure and protected paths. Final candidate-wide and native multiwindow verification is still in progress; these source records do not claim actual model invocation, real private repository writes, installer execution or another computer's behavior.


## v0.2.1 local changes

Scroll sizing, same-window split focus/visibility, left outline, automatic external mapping and storage lifecycle reuse the existing React, Tauri, Git tree/blob hashing, SQLite and Windows APIs. These application changes are independently implemented for the confirmed requirements, with no new dependency download. New local storage/developer path configuration and verification fixtures are private metadata, excluded from public source and portable archives.

`src-tauri/installer.nsi` adapts the installer template embedded in the already installed Tauri CLI `@tauri-apps/cli-win32-x64-msvc` 2.12.4 (MIT OR Apache-2.0; original SHA256 `a0eea9f8e16a33df772f74499beb886fece8e006b7c16943f4fa84ebbc6ef450`). It retains the standard installer and changes only the initial default directory and missing-D-drive handling before any output directory is created. Explicit `/D=` and the recorded upgrade location take precedence. Upstream: [Tauri NSIS installer template](https://github.com/tauri-apps/tauri/blob/tauri-cli-v2.12.4/crates/tauri-bundler/src/bundle/windows/templates/installer.nsi). The existing Tauri license texts in `THIRD_PARTY_NOTICES.txt` remain included; no network download was needed for this template.

## v0.2.2 local changes

Visual hierarchy, intrinsic header layout, zoom-aware scoped outline navigation, derived reference presentation and automatic-record proof refresh are independently implemented in the existing React/Tauri application. They reuse existing browser geometry, semantic reference identities, task/SQLite state and Git tree/blob verification. No new external code, download or dependency is introduced. Existing upstream references and licenses retain their prior scope. Native UI and protocol tests are reported separately; controlled IPC display scenarios do not establish real private repository writes.


## v0.2.3 export clarity and contrast

Authoritative export manifests, memory-only shared choices, one-time ordinary environment attachments and three-theme contrast are independent changes within the existing application. Existing Tauri events, React state, ZIP, canonical-path and immutable GitHub read facilities are reused. No new external code, package or downloaded dependency was introduced; existing notices remain applicable. Verification distinguishes actual native IPC/ZIP output, synthetic protocol cases, style-only samples and unverified installer/save-dialog visuals.


## v0.2.4 file groups, automatic scope and history
This update is original local work on the existing application: shared resource rows, flat groups, a read-only history view, stable fold identities and automatic local scope evidence. It reuses the existing React/Tauri/Rust dependencies and resource proof/export protocols. No new third-party dependency, downloaded source or adapted external code was introduced. The existing sources and license notices above remain applicable.


## v0.2.5 layout and read performance

The layout uses the user's supplied ChatGPT screenshots as a method reference for compact navigation, consistent hierarchy and aligned actions. No third-party interface source code or screenshot asset is incorporated. Status presentation, client task/usage/repair behavior, settings sections and bounded document reads are independent maintenance of the existing React/Tauri/Rust application. Existing dependencies and license notices remain applicable; no new package, download or adapted project code was introduced.

Performance evidence compares actual isolated native windows with synthetic local Skill folders, exact executable hashes, identical100-Skill/20-round inputs and request counts. File and documentation measurements are reported separately; no private GitHub, real model call or VPN benchmark is inferred.


<a id="v026-method-references"></a>
## v0.2.6 method references — 2026-10-25

Read-only comparison of public repositories informed the following independently implemented UI and compatibility changes. No third-party application source was copied, installed or executed; no new dependency was introduced. Star counts are a point-in-time research snapshot, not a quality guarantee. The existing application framework, runtime ownership, proof and recovery contracts remain authoritative.

| Repository / fixed commit | Upstream license | Method considered / adopted |
| --- | --- | --- |
| [xingkongliang/skills-manager @7a22192](https://github.com/xingkongliang/skills-manager/tree/7a2219219287f4ef4c543fc169ebcc5b0bff085c) | MIT | Compact search/filter toolbar, category chips and card information hierarchy. Independently implement within existing views. |
| [skillsgate/skillsgate @4f33578](https://github.com/skillsgate/skillsgate/tree/4f33578de42c1635d4305a606310e7f812dc6dbd) | MIT | Per-client action clarity and shared-source removal protection scenario. Keep existing link-only disconnect proof. |
| [runkids/skillshare @a5477eb](https://github.com/runkids/skillshare/tree/a5477ebe6b0975c3ecf479b417b4db9ae30e7609) | MIT | Windows directory-link and local-modification preservation methods; existing implementation remains in place. |
| [vercel-labs/skills @a80e4c9](https://github.com/vercel-labs/skills/tree/a80e4c9d12d607631249d4d7d059f835d4b2f13a) | MIT | Client adaptation and explicit source/hash identities; no silent copy fallback adopted. |
| [qufei1993/skills-hub @1647138](https://github.com/qufei1993/skills-hub/tree/1647138a35c7d4cb4808f11f724cbaa77609f0de) | MIT | Card/filter alignment and clear batch-completion feedback. |
| [jiweiyeah/Skills-Manager @bc3c8bd](https://github.com/jiweiyeah/Skills-Manager/tree/bc3c8bd68cfe9a58a80b7c3631a8ed531d950823) | MIT | On-demand heavy views and batch-selection presentation; no extra feature/dependency added. |
| [rohitg00/skillkit @d2e5c34](https://github.com/rohitg00/skillkit/tree/d2e5c346f73325c326820efd3188dcf2b6eb704f) | Apache-2.0 | Evaluated CLI/TUI approach; no direct adoption in this desktop refinement. |
| [lcdzzz/lcdzzz-skill-admin @e5d8103](https://github.com/lcdzzz/lcdzzz-skill-admin/tree/e5d81035a36419de3e50c6547310e8d094473f0b) | MIT | Explicit import paths/conflicts and separate card checkbox/detail actions. Copy synchronization and name-only merging were not adopted. |

The user's reference screenshots informed spacing and action hierarchy. Existing icon/dependency notices remain applicable. Tests use isolated synthetic files; third-party project performance claims are not evidence for this program.


<a id="v027-method-references"></a>
## v0.2.7 method references — 2026-10-25

Read-only source/docs review used the fixed public commits below, together with the user's GPT screenshots. No upstream application source, screenshot asset or dependency was copied, installed or executed. These are methods for independently maintaining this existing application; all prior ownership, source-proof, recovery and data contracts remain.

| Fixed source / upstream license | Method adopted / boundary |
| --- | --- |
| [xingkongliang/skills-manager MySkills.tsx](https://github.com/xingkongliang/skills-manager/blob/7a2219219287f4ef4c543fc169ebcc5b0bff085c/src/views/MySkills.tsx#L1532), MIT | Compact toolbar and colored category hierarchy. Preserve our stable category identities rather than index-dependent colors. |
| [skillsgate/skillsgate home.tsx](https://github.com/skillsgate/skillsgate/blob/4f33578de42c1635d4305a606310e7f812dc6dbd/apps/desktop/src/renderer/routes/home.tsx#L2523), MIT | Considered split-pane and anchored-action presentation. Our independent help also handles viewport collision, focus and zoom. |
| [lcdzzz/lcdzzz-skill-admin main.tsx](https://github.com/lcdzzz/lcdzzz-skill-admin/blob/e5d81035a36419de3e50c6547310e8d094473f0b/src/main.tsx#L788), MIT | Compact card identity and folded roots. Do not adopt copy synchronization or name-only merging. |
| [qufei1993/skills-hub design guidelines](https://github.com/qufei1993/skills-hub/blob/1647138a35c7d4cb4808f11f724cbaa77609f0de/docs/UI-DESIGN-GUIDELINES.zh.md), MIT | Semantic colors, spacing, progressive explanations and avoiding nested decoration/duplicate badges. SkillCard body-portal method reviewed. |
| [jiweiyeah/Skills-Manager index.css](https://github.com/jiweiyeah/Skills-Manager/blob/bc3c8bd68cfe9a58a80b7c3631a8ed531d950823/src/index.css), MIT | Separate page/sidebar/card/popover/foreground roles; do not reuse its palette as a claim of exact GPT colors. |

At the research snapshot: skills-manager 6812 stars/latest commit 10-21/v1.43.0; skillsgate 1405/10-13/desktop-v0.8.1; lcdzzz-skill-admin 2/10-12/v0.0.8; skills-hub 1823/main commit10-16/v1.0.1 (repository pushed10-23); jiweiyeah Skills-Manager1037/latest docs commit09-10/v2.2.0. All five were unarchived and MIT. Activity dates/stars describe only this snapshot, not a maintenance guarantee. Fixed source links above are the implementation evidence.

The supplied GPT image informed dark main #181818, sidebar #1c1a1e and input #363636. The application uses its own semantic light/eye palettes and independent component colors, with actual composed-state contrast verification.
