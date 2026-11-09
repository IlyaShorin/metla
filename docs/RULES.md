# Rule catalogue

Every candidate comes from one of these rules. `~` is the user's home
directory; `*` matches exactly one path component. A rule never matches
inside another candidate, inside an excluded path, or through a symlink.

The catalogue is data in `core/crates/metla-core/src/rules/catalogue.rs`;
a test fails when its rule ids and the ids in the tables below differ. A
rule id is its category's name (`app-cache`, `dev-cache`, `build`, `log`,
`temp`, `worktree`), a dot and the rule's own name; the app localises
titles by id.

Three matcher kinds exist.

* **Location** — an exact path pattern for a place a known tool owns.
* **Project output** — a directory name plus markers, valid only inside a Git
  repository, and re-verified as Git-ignored before removal. No marker, no
  match. A project-output directory that was modified in the last 14 days
  (configurable) is listed but not preselected.
* **Linked worktree** — a directory whose `.git` is a pointer file into a
  repository's `worktrees` directory (last section).

A directory is checked against location rules first, then the worktree
rule, then project output; within a kind the first rule in catalogue order
wins. Only the outermost of nested matches is a candidate: nothing inside a
candidate is classified, and that includes a linked worktree's own build
output.

### Location patterns

* `~` is the home directory the engine is given (never read from the
  environment by the rules); a pattern may also be absolute.
* `*` is exactly one component; `{a,b}` lists alternatives, which may span
  components (`{Google/Chrome,Chromium}`); `<user>` is the user's name.
* `name[a|b]`: the directory at that component must contain an entry `a`
  or `b`. Conditions can sit one or two levels above the matched
  directory (the walker reports the parent's and the grandparent's marker
  entries); anywhere else they never hold.
* At every level a literal component wins over `*`. A `*` that is the last
  component never claims a name another pattern spells out at that
  position: `~/Library/Caches/*` leaves `Homebrew` to `dev-cache.homebrew`
  and `Firefox` to `app-cache.firefox`, which reaches deeper into it.
* A rule may carry a **deny list**: names (`x`, or `x*` for every name
  starting with `x`) its `*` components never match.

### Markers

A marker name is exact or a `*.ext` / `prefix.*` pattern. A **sibling**
marker is an entry of the candidate's parent, an **inner** marker an entry
of the candidate itself. A `CACHEDIR.TAG` marker counts only when the file
is a regular file that starts with the standard signature
`Signature: 8a477f597d28d172789f06886806bc55`; the walker reads at most
2048 bytes of it.

## App caches (`AppCache`, permanent delete)

| Rule id | Matches | Why it is safe |
|---------|---------|----------------|
| `app-cache.library` | `~/Library/Caches/*` except names owned by another rule and Apple system caches listed in the rule's deny list | Apps rebuild their cache directory |
| `app-cache.container` | `~/Library/Containers/*/Data/Library/Caches` except Apple's own apps (`com.apple.*`) | Sandboxed app cache |
| `app-cache.chromium` | `Cache`, `Code Cache`, `GPUCache`, `Service Worker/CacheStorage` inside a Chromium profile under `~/Library/Application Support/{Google/Chrome,BraveSoftware/Brave-Browser,Microsoft Edge,Arc/User Data,Chromium}/*`, when the profile has a `Preferences` file | Browser HTTP/code caches |
| `app-cache.electron` | `Cache`, `Code Cache`, `GPUCache`, `CachedData` directly inside `~/Library/Application Support/*` when a sibling `Local Storage` or `Preferences` exists | Electron runtime caches |
| `app-cache.firefox` | `~/Library/Caches/Firefox/Profiles/*/cache2` | Browser cache |

The deny list of `app-cache.library` is conservative: every name starting
with `com.apple.`, and `CloudKit`, `FamilyCircle`, `familycircled`,
`GameKit`, `GeoServices`, `Metadata`, `PassKit`, `SiriTTS`,
`TelephonyUtilities`, `askpermissiond`, `storeassetd`. System services keep
sync, account and location state there; rebuilding it can mean a full
re-sync or a lost setting, so Metla never offers them. Names that another
rule spells out (`Homebrew`, `Yarn`, `pnpm`, `deno`, `pip`, `pypoetry`,
`go-build`, `CocoaPods`, `org.swift.swiftpm`, `JetBrains`, `ms-playwright`,
`ccache`, `Mozilla.sccache`, `Firefox`) belong to that rule.

## Developer caches (`DevCache`, permanent delete)

| Rule id | Matches |
|---------|---------|
| `dev-cache.homebrew` | `~/Library/Caches/Homebrew` |
| `dev-cache.npm` | `~/.npm/_cacache` |
| `dev-cache.yarn` | `~/Library/Caches/Yarn`, `~/.yarn/berry/cache` |
| `dev-cache.pnpm` | `~/Library/pnpm/store`, `~/Library/Caches/pnpm` |
| `dev-cache.bun` | `~/.bun/install/cache` |
| `dev-cache.deno` | `~/Library/Caches/deno` |
| `dev-cache.pip` | `~/Library/Caches/pip` |
| `dev-cache.uv` | `~/.cache/uv` |
| `dev-cache.poetry` | `~/Library/Caches/pypoetry` |
| `dev-cache.cargo-registry` | `~/.cargo/registry/cache`, `~/.cargo/registry/src`, `~/.cargo/git/checkouts` |
| `dev-cache.go-build` | `~/Library/Caches/go-build` |
| `dev-cache.go-mod` | `~/go/pkg/mod/cache/download` |
| `dev-cache.gradle` | `~/.gradle/caches`, `~/.gradle/daemon` |
| `dev-cache.cocoapods` | `~/Library/Caches/CocoaPods` |
| `dev-cache.swiftpm` | `~/Library/Caches/org.swift.swiftpm` |
| `dev-cache.xcode-derived-data` | `~/Library/Developer/Xcode/DerivedData/*` |
| `dev-cache.xcode-device-support` | `~/Library/Developer/Xcode/{iOS,watchOS,tvOS,visionOS} DeviceSupport/*` |
| `dev-cache.core-simulator` | `~/Library/Developer/CoreSimulator/Caches` |
| `dev-cache.jetbrains` | `~/Library/Caches/JetBrains/*` |
| `dev-cache.vscode` | `CachedData`, `CachedExtensionVSIXs`, `Cache`, `Code Cache`, `GPUCache` in `~/Library/Application Support/{Code,Cursor,VSCodium}` |
| `dev-cache.playwright` | `~/Library/Caches/ms-playwright` |
| `dev-cache.bazel` | `/private/var/tmp/_bazel_<user>` (`<user>` is the user's own name) |
| `dev-cache.ccache` | `~/Library/Caches/ccache`, `~/.ccache` |
| `dev-cache.sccache` | `~/Library/Caches/Mozilla.sccache` |
| `dev-cache.flutter-pub` | `~/.pub-cache/hosted` |
| `dev-cache.terraform` | `~/.terraform.d/plugin-cache` |

Deliberately **not** rules: `~/.m2/repository` (may hold locally installed
artifacts), Docker disk images, simulator devices, model stores
(`~/.ollama`, Hugging Face), virtual machines. They show up in the Folders
view by size; they are not junk by rule.

## Build artifacts (`BuildArtifact`, permanent delete, inside a Git repo, Git-ignored)

| Rule id | Directory | Marker |
|---------|-----------|--------|
| `build.cargo-target` | `target` | sibling `Cargo.toml`, or inner `CACHEDIR.TAG` with the standard signature plus inner `.rustc_info.json` |
| `build.node-modules` | `node_modules` | sibling `package.json` |
| `build.next` | `.next` | sibling `package.json` and sibling `next.config.*` |
| `build.nuxt` | `.nuxt` | sibling `package.json` and sibling `nuxt.config.*` |
| `build.turbo` | `.turbo` | sibling `package.json` or `turbo.json` |
| `build.parcel-cache` | `.parcel-cache` | sibling `package.json` |
| `build.swiftpm` | `.build` | sibling `Package.swift` |
| `build.xcode-derived-data` | `DerivedData` | sibling `*.xcodeproj` or `*.xcworkspace` |
| `build.cocoapods` | `Pods` | sibling `Podfile` |
| `build.gradle-build` | `build` | sibling `build.gradle` or `build.gradle.kts` |
| `build.gradle-cache` | `.gradle` | sibling `settings.gradle`, `settings.gradle.kts`, `build.gradle` or `build.gradle.kts` |
| `build.maven-target` | `target` | sibling `pom.xml` |
| `build.python-pycache` | `__pycache__` | contains at least one `*.pyc` |
| `build.pytest-cache` | `.pytest_cache` | inner `CACHEDIR.TAG` with the standard signature |
| `build.mypy-cache` | `.mypy_cache` | inner `CACHEDIR.TAG` with the standard signature, or inner `.gitignore` |
| `build.ruff-cache` | `.ruff_cache` | inner `CACHEDIR.TAG` with the standard signature |
| `build.tox` | `.tox` | sibling `tox.ini` |
| `build.cmake` | any name | inner `CMakeCache.txt` and inner `CMakeFiles` |
| `build.dart-tool` | `.dart_tool` | sibling `pubspec.yaml` |
| `build.flutter-build` | `build` | sibling `pubspec.yaml` |
| `build.zig-cache` | `.zig-cache`, `zig-cache`, `zig-out` | sibling `build.zig` |
| `build.elixir` | `_build` | sibling `mix.exs` |
| `build.haskell` | `dist-newstyle`, `.stack-work` | sibling `*.cabal` or `stack.yaml` |
| `build.terraform` | `.terraform` | sibling `*.tf` |

There is no rule for a bare `dist`, `build`, `out`, `cache` or `tmp`
directory, for a `target` without a Cargo or Maven marker, or for a
`node_modules` outside a Git repository.

Nothing inside an installed package tree — below a directory named
`node_modules`, `site-packages`, `dist-packages` or `Cellar` — is build
output. Version managers and package prefixes (nvm, asdf, pyenv, Homebrew)
are often Git clones that keep installed packages in an ignored
`lib/node_modules`, `site-packages` or `Cellar`, where for example
`npm/node_modules` sits next to a `package.json` exactly like a project's
dependencies; removing it would break the installed tool, and Git reports
it as ignored, so the removal-time check could not tell. (On the
development machine this also kept 1,216 of 1,300 `__pycache__` matches,
all in installed trees, out of "Build artifacts".)

## Logs (`Log`, move to Trash, always confirmed)

| Rule id | Matches |
|---------|---------|
| `log.user-library` | `~/Library/Logs/*` |
| `log.diagnostic-reports` | `~/Library/Logs/DiagnosticReports` (takes precedence over `log.user-library`) |
| `log.app-support` | `logs` or `Logs` directly inside `~/Library/Application Support/*` |

System logs under `/private/var/log` and `/Library/Logs` need root and are
shown by size only.

## Temporary data (`Temp`, always confirmed)

| Rule id | Matches | Disposal |
|---------|---------|----------|
| `temp.trash` | entries of `~/.Trash` (as one candidate) | permanent delete (this is "Empty Trash") |

Xcode archives, downloads and installers are not rules.

## Git worktrees (`GitWorktree`, `git worktree remove`, always confirmed)

| Rule id | Matches |
|---------|---------|
| `worktree.linked` | a directory whose `.git` is a regular file starting with `gitdir:` and pointing into a `worktrees/<name>` directory of a repository |

The walker reads at most 2048 bytes of a `.git` that is a regular file
(never a symlink, never a cloud placeholder). Its first line must be
`gitdir: <git dir>/worktrees/<name>` where the git directory's name ends in
`.git` (`.git` of a working tree, or a bare `repo.git`) and `<name>` is a
real name. A submodule's pointer (`…/.git/modules/…`), a worktree of a
submodule and anything else are not linked worktrees. The candidate is
recorded during the scan; its Git status is computed before it can be
offered (M3).

A worktree is removable only with a clean `WorktreeStatus`; see
ARCHITECTURE.md §9.

## Exclusions and preselection

* An exclusion (an absolute path the user keeps) hides a candidate that is
  the path, lies beneath it, or holds it — removing a candidate that holds
  the kept path would remove that path too. Paths compare by whole
  components: excluding `/a/b` hides neither `/a/bc` nor its contents.
  Exclusions are applied when the index is read, so changing them needs no
  rescan.
* A candidate is preselected unless it is build output active within the
  recent-activity age (default 14 days; activity is the newest modification
  time in its tree, and unknown activity counts as recent) or a worktree,
  whose status decides.

## File kinds

The Overview's "By data type" row totals the scan's bytes by file kind,
for the whole scan (not per directory). Where a file is decides first:

1. **Developer** — inside a Git repository, a developer cache or build
   output candidate, or `~/Library/Developer`;
2. **Applications** — inside a directory whose name ends in `.app`;
3. **System** — below `/System`, `/Library`, `/private`, `/usr`, `/bin`,
   `/sbin` or `/cores`;
4. otherwise by extension (case-insensitive): **Media** (images, audio,
   video, camera raw), **Documents** (PDF, office and iWork files, text,
   Markdown, CSV, EPUB), **Archives and disk images** (zip, tar and its
   compressions, 7z, rar, dmg, iso, img, pkg, xip, sparseimage), and
   **Other** for the rest.
