# Rule catalogue

Every candidate comes from one of these rules. `~` is the user's home
directory; `*` matches exactly one path component. A rule never matches
inside another candidate, inside an excluded path, or through a symlink.

Two matcher kinds exist.

* **Location** — an exact path pattern for a place a known tool owns.
* **Project output** — a directory name plus markers, valid only inside a Git
  repository, and re-verified as Git-ignored before removal. A project-output
  directory that was modified in the last 14 days is listed but not
  preselected.

## App caches (`AppCache`, permanent delete)

| Rule id | Matches | Why it is safe |
|---------|---------|----------------|
| `app-cache.library` | `~/Library/Caches/*` except names owned by a `DevCache` rule and Apple system caches listed in the rule's deny list | Apps rebuild their cache directory |
| `app-cache.container` | `~/Library/Containers/*/Data/Library/Caches` | Sandboxed app cache |
| `app-cache.chromium` | `Cache`, `Code Cache`, `GPUCache`, `Service Worker/CacheStorage` inside a Chromium profile under `~/Library/Application Support/{Google/Chrome,BraveSoftware/Brave-Browser,Microsoft Edge,Arc/User Data,Chromium}/*`, when the profile has a `Preferences` file | Browser HTTP/code caches |
| `app-cache.electron` | `Cache`, `Code Cache`, `GPUCache`, `CachedData` directly inside `~/Library/Application Support/*` when a sibling `Local Storage` or `Preferences` exists | Electron runtime caches |
| `app-cache.firefox` | `~/Library/Caches/Firefox/Profiles/*/cache2` | Browser cache |

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
| `dev-cache.bazel` | `/private/var/tmp/_bazel_<user>` |
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
| `build.pytest-cache` | `.pytest_cache` | inner `CACHEDIR.TAG` |
| `build.mypy-cache` | `.mypy_cache` | inner `CACHEDIR.TAG` or `.gitignore` |
| `build.ruff-cache` | `.ruff_cache` | inner `CACHEDIR.TAG` |
| `build.tox` | `.tox` | sibling `tox.ini` |
| `build.cmake` | any name | inner `CMakeCache.txt` and inner `CMakeFiles` |
| `build.dart-tool` | `.dart_tool` | sibling `pubspec.yaml` |
| `build.flutter-build` | `build` | sibling `pubspec.yaml` |
| `build.zig-cache` | `.zig-cache`, `zig-cache`, `zig-out` | sibling `build.zig` |
| `build.elixir` | `_build` | sibling `mix.exs` |
| `build.haskell` | `dist-newstyle`, `.stack-work` | sibling `*.cabal` or `stack.yaml` |
| `build.terraform` | `.terraform` | sibling `*.tf` |

There is no rule for a bare `dist`, `build`, `out` or `cache` directory.

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

A worktree is removable only with a clean `WorktreeStatus`; see
ARCHITECTURE.md §9.
