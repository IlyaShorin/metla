//! Classification is the safety core: every rule is shown a tree it must
//! match and the nearest trees it must not. Each case builds a synthetic
//! tree (see `Fixture::build`; `~` is the tree's home directory), scans it
//! with the rule set and compares every candidate found.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::sync::Arc;

use metla_core::rules::RuleSet;
use metla_core::scan::ScanControl;

use crate::scan::{options, scan_with};
use crate::support::Fixture;

/// A tree and every candidate it must yield, as (path, rule id).
type Case<'a> = (&'a [&'a str], &'a [(&'a str, &'a str)]);

pub const TAG: &str = "CACHEDIR.TAG=Signature: 8a477f597d28d172789f06886806bc55\n";

/// The candidates a scan of `lines` finds, as (path, rule id).
pub fn found(lines: &[&str]) -> BTreeSet<(String, String)> {
    let fx = Fixture::new();
    fx.build(lines);
    let rules = RuleSet::new(&fx.home(), OsStr::new("tester"));
    let options = options(fx.root()).rules(Arc::new(rules));
    let scanned = scan_with(&options, &ScanControl::new(64));
    let candidates = scanned.records.iter().filter_map(|(path, record)| {
        let rule = record.rule.as_ref()?;
        Some((fx.spelled(path), rule.to_string()))
    });
    candidates.collect()
}

fn check(cases: &[Case<'_>]) {
    for (lines, expected) in cases {
        let expected: BTreeSet<(String, String)> = expected
            .iter()
            .map(|(path, rule)| (path.to_string(), rule.to_string()))
            .collect();
        assert_eq!(found(lines), expected, "tree {lines:?}");
    }
}

#[test]
fn app_cache_rules_match_their_places_and_nothing_next_to_them() {
    const PREFS: &str = "~/Library/Application Support/Google/Chrome/Default/Preferences";
    check(&[
        (
            &["~/Library/Caches/com.example.app/sub/"],
            &[("~/Library/Caches/com.example.app", "app-cache.library")],
        ),
        (&["~/Library/Caches/"], &[]),
        (&["~/Library/com.example.app/"], &[]),
        (&["~/x/Library/Caches/com.example.app/"], &[]),
        (
            &[
                "~/Library/Caches/com.apple.Safari/",
                "~/Library/Caches/CloudKit/",
                "~/Library/Caches/GeoServices/",
                "~/Library/Caches/familycircled/",
            ],
            &[],
        ),
        (
            &["~/Library/Containers/com.example.app/Data/Library/Caches/x/"],
            &[(
                "~/Library/Containers/com.example.app/Data/Library/Caches",
                "app-cache.container",
            )],
        ),
        (
            &[
                "~/Library/Containers/com.apple.mail/Data/Library/Caches/",
                "~/Library/Containers/com.example.app/Data/Library/Logs/",
                "~/Library/Containers/com.example.app/Data/Caches/",
                "~/Library/Containers/a/b/Data/Library/Caches/",
            ],
            &[],
        ),
        (
            &[
                PREFS,
                "~/Library/Application Support/Google/Chrome/Default/Cache/",
                "~/Library/Application Support/Google/Chrome/Default/Code Cache/",
                "~/Library/Application Support/Google/Chrome/Default/GPUCache/",
                "~/Library/Application Support/Google/Chrome/Default/Service Worker/CacheStorage/",
                "~/Library/Application Support/Google/Chrome/Default/Service Worker/Database/",
                "~/Library/Application Support/Google/Chrome/Default/History/",
            ],
            &[
                (
                    "~/Library/Application Support/Google/Chrome/Default/Cache",
                    "app-cache.chromium",
                ),
                (
                    "~/Library/Application Support/Google/Chrome/Default/Code Cache",
                    "app-cache.chromium",
                ),
                (
                    "~/Library/Application Support/Google/Chrome/Default/GPUCache",
                    "app-cache.chromium",
                ),
                (
                    "~/Library/Application Support/Google/Chrome/Default/Service Worker/CacheStorage",
                    "app-cache.chromium",
                ),
            ],
        ),
        (
            &[
                "~/Library/Application Support/BraveSoftware/Brave-Browser/Profile 1/Preferences",
                "~/Library/Application Support/BraveSoftware/Brave-Browser/Profile 1/Cache/",
                "~/Library/Application Support/Microsoft Edge/Default/Preferences",
                "~/Library/Application Support/Microsoft Edge/Default/GPUCache/",
                "~/Library/Application Support/Arc/User Data/Default/Preferences",
                "~/Library/Application Support/Arc/User Data/Default/Code Cache/",
                "~/Library/Application Support/Chromium/Default/Preferences",
                "~/Library/Application Support/Chromium/Default/Cache/",
            ],
            &[
                (
                    "~/Library/Application Support/BraveSoftware/Brave-Browser/Profile 1/Cache",
                    "app-cache.chromium",
                ),
                (
                    "~/Library/Application Support/Microsoft Edge/Default/GPUCache",
                    "app-cache.chromium",
                ),
                (
                    "~/Library/Application Support/Arc/User Data/Default/Code Cache",
                    "app-cache.chromium",
                ),
                (
                    "~/Library/Application Support/Chromium/Default/Cache",
                    "app-cache.chromium",
                ),
            ],
        ),
        (
            &[
                "~/Library/Application Support/Google/Chrome/Default/Cache/",
                "~/Library/Application Support/Google/Chrome/Default/Service Worker/CacheStorage/",
                "~/Library/Application Support/Google/Chrome/Default/Service Worker/Preferences",
                "~/Library/Application Support/Google/Chrome/Preferences",
            ],
            &[],
        ),
        (
            &[
                "~/Library/Application Support/Slack/Local Storage/",
                "~/Library/Application Support/Slack/Cache/",
                "~/Library/Application Support/Slack/Code Cache/",
                "~/Library/Application Support/Slack/GPUCache/",
                "~/Library/Application Support/Slack/CachedData/",
                "~/Library/Application Support/Other/Preferences",
                "~/Library/Application Support/Other/Cache/",
            ],
            &[
                (
                    "~/Library/Application Support/Slack/Cache",
                    "app-cache.electron",
                ),
                (
                    "~/Library/Application Support/Slack/Code Cache",
                    "app-cache.electron",
                ),
                (
                    "~/Library/Application Support/Slack/GPUCache",
                    "app-cache.electron",
                ),
                (
                    "~/Library/Application Support/Slack/CachedData",
                    "app-cache.electron",
                ),
                (
                    "~/Library/Application Support/Other/Cache",
                    "app-cache.electron",
                ),
            ],
        ),
        (
            &[
                "~/Library/Application Support/NoMarker/Cache/",
                "~/Library/Application Support/Slack/Local Storage/",
                "~/Library/Application Support/Slack/Data/Cache/",
                "~/Library/Application Support/Cache/",
            ],
            &[],
        ),
        (
            &[
                "~/Library/Caches/Firefox/Profiles/abc.default/cache2/x/",
                "~/Library/Caches/Firefox/Profiles/abc.default/startupCache/",
                "~/Library/Caches/Firefox/other/",
            ],
            &[(
                "~/Library/Caches/Firefox/Profiles/abc.default/cache2",
                "app-cache.firefox",
            )],
        ),
        (&["~/Library/Caches/Firefox/Profiles/cache2/"], &[]),
    ]);
}

#[test]
fn developer_cache_rules_match_their_places() {
    check(&[
        (
            &["~/Library/Caches/Homebrew/downloads/"],
            &[("~/Library/Caches/Homebrew", "dev-cache.homebrew")],
        ),
        (&["~/Library/Homebrew/", "~/Homebrew/"], &[]),
        (
            &[
                "~/Library/Yarn/",
                "~/Library/pnpm/",
                "~/Library/deno/",
                "~/Library/pip/",
                "~/Library/pypoetry/",
                "~/Library/go-build/",
                "~/Library/CocoaPods/",
                "~/Library/org.swift.swiftpm/",
                "~/Library/ms-playwright/",
                "~/Library/ccache/",
                "~/Library/Mozilla.sccache/",
                "~/Library/JetBrains/x/",
            ],
            &[],
        ),
        (
            &["~/.npm/_cacache/"],
            &[("~/.npm/_cacache", "dev-cache.npm")],
        ),
        (&["~/.npm/_logs/", "~/.npm/x/_cacache/", "~/_cacache/"], &[]),
        (
            &["~/Library/Caches/Yarn/", "~/.yarn/berry/cache/"],
            &[
                ("~/Library/Caches/Yarn", "dev-cache.yarn"),
                ("~/.yarn/berry/cache", "dev-cache.yarn"),
            ],
        ),
        (&["~/.yarn/cache/", "~/.yarn/berry/"], &[]),
        (
            &["~/Library/pnpm/store/", "~/Library/Caches/pnpm/"],
            &[
                ("~/Library/pnpm/store", "dev-cache.pnpm"),
                ("~/Library/Caches/pnpm", "dev-cache.pnpm"),
            ],
        ),
        (
            &["~/.bun/install/cache/"],
            &[("~/.bun/install/cache", "dev-cache.bun")],
        ),
        (&["~/.bun/install/global/", "~/.bun/bin/"], &[]),
        (
            &["~/Library/Caches/deno/"],
            &[("~/Library/Caches/deno", "dev-cache.deno")],
        ),
        (
            &["~/Library/Caches/pip/"],
            &[("~/Library/Caches/pip", "dev-cache.pip")],
        ),
        (&["~/.cache/uv/"], &[("~/.cache/uv", "dev-cache.uv")]),
        (&["~/.cache/other/", "~/.cache/"], &[]),
        (
            &["~/Library/Caches/pypoetry/"],
            &[("~/Library/Caches/pypoetry", "dev-cache.poetry")],
        ),
        (
            &[
                "~/.cargo/registry/cache/",
                "~/.cargo/registry/src/",
                "~/.cargo/git/checkouts/",
            ],
            &[
                ("~/.cargo/registry/cache", "dev-cache.cargo-registry"),
                ("~/.cargo/registry/src", "dev-cache.cargo-registry"),
                ("~/.cargo/git/checkouts", "dev-cache.cargo-registry"),
            ],
        ),
        (
            &[
                "~/.cargo/registry/index/",
                "~/.cargo/git/db/",
                "~/.cargo/bin/",
            ],
            &[],
        ),
        (
            &["~/Library/Caches/go-build/"],
            &[("~/Library/Caches/go-build", "dev-cache.go-build")],
        ),
        (
            &["~/go/pkg/mod/cache/download/"],
            &[("~/go/pkg/mod/cache/download", "dev-cache.go-mod")],
        ),
        (
            &["~/go/pkg/mod/github.com/x/", "~/go/pkg/mod/cache/vcs/"],
            &[],
        ),
        (
            &["~/.gradle/caches/", "~/.gradle/daemon/"],
            &[
                ("~/.gradle/caches", "dev-cache.gradle"),
                ("~/.gradle/daemon", "dev-cache.gradle"),
            ],
        ),
        (&["~/.gradle/wrapper/"], &[]),
        (
            &["~/Library/Caches/CocoaPods/"],
            &[("~/Library/Caches/CocoaPods", "dev-cache.cocoapods")],
        ),
        (
            &["~/Library/Caches/org.swift.swiftpm/"],
            &[("~/Library/Caches/org.swift.swiftpm", "dev-cache.swiftpm")],
        ),
        (
            &["~/Library/Developer/Xcode/DerivedData/App-abc/Build/"],
            &[(
                "~/Library/Developer/Xcode/DerivedData/App-abc",
                "dev-cache.xcode-derived-data",
            )],
        ),
        (&["~/Library/Developer/Xcode/DerivedData/"], &[]),
        (
            &[
                "~/Library/Developer/Xcode/iOS DeviceSupport/17.0/",
                "~/Library/Developer/Xcode/watchOS DeviceSupport/10.0/",
                "~/Library/Developer/Xcode/tvOS DeviceSupport/17.0/",
                "~/Library/Developer/Xcode/visionOS DeviceSupport/1.0/",
            ],
            &[
                (
                    "~/Library/Developer/Xcode/iOS DeviceSupport/17.0",
                    "dev-cache.xcode-device-support",
                ),
                (
                    "~/Library/Developer/Xcode/watchOS DeviceSupport/10.0",
                    "dev-cache.xcode-device-support",
                ),
                (
                    "~/Library/Developer/Xcode/tvOS DeviceSupport/17.0",
                    "dev-cache.xcode-device-support",
                ),
                (
                    "~/Library/Developer/Xcode/visionOS DeviceSupport/1.0",
                    "dev-cache.xcode-device-support",
                ),
            ],
        ),
        (
            &[
                "~/Library/Developer/Xcode/macOS DeviceSupport/14.0/",
                "~/Library/Developer/Xcode/Archives/2024-01-01/",
            ],
            &[],
        ),
        (
            &["~/Library/Developer/CoreSimulator/Caches/"],
            &[(
                "~/Library/Developer/CoreSimulator/Caches",
                "dev-cache.core-simulator",
            )],
        ),
        (&["~/Library/Developer/CoreSimulator/Devices/x/"], &[]),
        (
            &["~/Library/Caches/JetBrains/IntelliJIdea2024.1/"],
            &[(
                "~/Library/Caches/JetBrains/IntelliJIdea2024.1",
                "dev-cache.jetbrains",
            )],
        ),
        (&["~/Library/Caches/JetBrains/"], &[]),
        (
            &[
                "~/Library/Application Support/Code/Preferences",
                "~/Library/Application Support/Code/CachedData/",
                "~/Library/Application Support/Code/Cache/",
                "~/Library/Application Support/Code/Code Cache/",
                "~/Library/Application Support/Cursor/CachedExtensionVSIXs/",
                "~/Library/Application Support/VSCodium/GPUCache/",
                "~/Library/Application Support/Code/User/",
            ],
            &[
                (
                    "~/Library/Application Support/Code/CachedData",
                    "dev-cache.vscode",
                ),
                (
                    "~/Library/Application Support/Code/Cache",
                    "dev-cache.vscode",
                ),
                (
                    "~/Library/Application Support/Code/Code Cache",
                    "dev-cache.vscode",
                ),
                (
                    "~/Library/Application Support/Cursor/CachedExtensionVSIXs",
                    "dev-cache.vscode",
                ),
                (
                    "~/Library/Application Support/VSCodium/GPUCache",
                    "dev-cache.vscode",
                ),
            ],
        ),
        (
            &["~/Library/Caches/ms-playwright/"],
            &[("~/Library/Caches/ms-playwright", "dev-cache.playwright")],
        ),
        (
            &["~/Library/Caches/ccache/", "~/.ccache/"],
            &[
                ("~/Library/Caches/ccache", "dev-cache.ccache"),
                ("~/.ccache", "dev-cache.ccache"),
            ],
        ),
        (
            &["~/Library/Caches/Mozilla.sccache/"],
            &[("~/Library/Caches/Mozilla.sccache", "dev-cache.sccache")],
        ),
        (
            &["~/.pub-cache/hosted/"],
            &[("~/.pub-cache/hosted", "dev-cache.flutter-pub")],
        ),
        (&["~/.pub-cache/git/"], &[]),
        (
            &["~/.terraform.d/plugin-cache/"],
            &[("~/.terraform.d/plugin-cache", "dev-cache.terraform")],
        ),
        (&["~/.terraform.d/plugins/"], &[]),
    ]);
}

#[test]
fn logs_and_the_trash_match_their_places() {
    check(&[
        (
            &["~/Library/Logs/SomeApp/x/"],
            &[("~/Library/Logs/SomeApp", "log.user-library")],
        ),
        (
            &["~/Library/Logs/DiagnosticReports/"],
            &[("~/Library/Logs/DiagnosticReports", "log.diagnostic-reports")],
        ),
        (&["~/Library/Logs/", "~/Logs/SomeApp/"], &[]),
        (
            &[
                "~/Library/Application Support/SomeApp/logs/",
                "~/Library/Application Support/Other/Logs/",
                "~/Library/Application Support/Code/logs/",
            ],
            &[
                (
                    "~/Library/Application Support/SomeApp/logs",
                    "log.app-support",
                ),
                (
                    "~/Library/Application Support/Other/Logs",
                    "log.app-support",
                ),
                ("~/Library/Application Support/Code/logs", "log.app-support"),
            ],
        ),
        (
            &[
                "~/Library/Application Support/logs/",
                "~/Library/Application Support/SomeApp/x/logs/",
                "~/Library/Application Support/SomeApp/log/",
            ],
            &[],
        ),
        (&["~/.Trash/old/"], &[("~/.Trash", "temp.trash")]),
        (&["~/Trash/", "~/x/.Trash/"], &[]),
    ]);
}

#[test]
fn a_linked_worktree_is_found_by_its_git_pointer_file() {
    check(&[
        (
            &["wt/.git=gitdir: /elsewhere/repo/.git/worktrees/wt\n"],
            &[("wt", "worktree.linked")],
        ),
        (
            &["wt/.git=gitdir: ../repo.git/worktrees/feature-1"],
            &[("wt", "worktree.linked")],
        ),
        (
            &[
                "wt/.git=gitdir: /r/.git/worktrees/wt\n",
                "wt/Cargo.toml",
                "wt/target/",
            ],
            &[("wt", "worktree.linked")],
        ),
        (
            &[
                "sub/.git=gitdir: ../.git/modules/sub\n",
                "main/.git/worktrees/x/",
                "nested/.git=gitdir: /r/.git/modules/s/worktrees/w\n",
                "short/.git=gitdir: /r/.git/worktrees\n",
                "dots/.git=gitdir: /r/.git/worktrees/..\n",
                "bare/.git=/r/.git/worktrees/w\n",
                "other/.git=gitdir: /r/notgit/worktrees/w\n",
                "named/git=gitdir: /r/.git/worktrees/w\n",
                "linked/.git->../wt-pointer",
                "wt-pointer=gitdir: /r/.git/worktrees/w\n",
            ],
            &[],
        ),
    ]);
}

const IN_REPO: &str = "r/.git/";

#[test]
fn build_output_needs_its_marker_inside_a_git_repository() {
    check(&[
        (
            &[IN_REPO, "r/Cargo.toml", "r/target/debug/"],
            &[("r/target", "build.cargo-target")],
        ),
        (
            &[
                IN_REPO,
                "r/out/target/.rustc_info.json",
                &tagged("r/out/target"),
            ],
            &[("r/out/target", "build.cargo-target")],
        ),
        (
            &[
                IN_REPO,
                "r/a/target/CACHEDIR.TAG=Signature: wrong",
                "r/a/target/.rustc_info.json",
                &tagged("r/b/target"),
                "r/c/target/",
                "r/d/Cargo.toml",
                "r/d/sub/target/",
            ],
            &[],
        ),
        (&["p/Cargo.toml", "p/target/"], &[]),
        (
            &[IN_REPO, "r/package.json", "r/node_modules/a/"],
            &[("r/node_modules", "build.node-modules")],
        ),
        (
            &[
                "p/package.json",
                "p/node_modules/",
                IN_REPO,
                "r/node_modules/",
            ],
            &[],
        ),
        (
            &[IN_REPO, "r/package.json", "r/next.config.mjs", "r/.next/"],
            &[("r/.next", "build.next")],
        ),
        (
            &[
                IN_REPO,
                "r/package.json",
                "r/.next/",
                "r/a/next.config.js",
                "r/a/.next/",
            ],
            &[],
        ),
        (
            &[IN_REPO, "r/package.json", "r/nuxt.config.ts", "r/.nuxt/"],
            &[("r/.nuxt", "build.nuxt")],
        ),
        (&[IN_REPO, "r/nuxt.config.ts", "r/.nuxt/"], &[]),
        (
            &[
                IN_REPO,
                "r/turbo.json",
                "r/.turbo/",
                "r/a/package.json",
                "r/a/.turbo/",
            ],
            &[("r/.turbo", "build.turbo"), ("r/a/.turbo", "build.turbo")],
        ),
        (&[IN_REPO, "r/.turbo/"], &[]),
        (
            &[IN_REPO, "r/package.json", "r/.parcel-cache/"],
            &[("r/.parcel-cache", "build.parcel-cache")],
        ),
        (&[IN_REPO, "r/.parcel-cache/"], &[]),
        (
            &[IN_REPO, "r/Package.swift", "r/.build/"],
            &[("r/.build", "build.swiftpm")],
        ),
        (&[IN_REPO, "r/.build/"], &[]),
        (
            &[
                IN_REPO,
                "r/a/App.xcodeproj/",
                "r/a/DerivedData/",
                "r/b/App.xcworkspace/",
                "r/b/DerivedData/",
            ],
            &[
                ("r/a/DerivedData", "build.xcode-derived-data"),
                ("r/b/DerivedData", "build.xcode-derived-data"),
            ],
        ),
        (&[IN_REPO, "r/DerivedData/", "r/xcodeproj/"], &[]),
        (
            &[IN_REPO, "r/Podfile", "r/Pods/"],
            &[("r/Pods", "build.cocoapods")],
        ),
        (&[IN_REPO, "r/Pods/"], &[]),
        (
            &[
                IN_REPO,
                "r/a/build.gradle",
                "r/a/build/",
                "r/b/build.gradle.kts",
                "r/b/build/",
            ],
            &[
                ("r/a/build", "build.gradle-build"),
                ("r/b/build", "build.gradle-build"),
            ],
        ),
        (
            &[
                IN_REPO,
                "r/a/settings.gradle",
                "r/a/.gradle/",
                "r/b/settings.gradle.kts",
                "r/b/.gradle/",
                "r/c/build.gradle",
                "r/c/.gradle/",
                "r/d/build.gradle.kts",
                "r/d/.gradle/",
                "r/e/.gradle/",
            ],
            &[
                ("r/a/.gradle", "build.gradle-cache"),
                ("r/b/.gradle", "build.gradle-cache"),
                ("r/c/.gradle", "build.gradle-cache"),
                ("r/d/.gradle", "build.gradle-cache"),
            ],
        ),
        (
            &[IN_REPO, "r/pom.xml", "r/target/"],
            &[("r/target", "build.maven-target")],
        ),
        (
            &[IN_REPO, "r/__pycache__/m.cpython-312.pyc"],
            &[("r/__pycache__", "build.python-pycache")],
        ),
        (
            &[IN_REPO, "r/__pycache__/notes.txt", "r/a/__pycache__/"],
            &[],
        ),
        (
            &[IN_REPO, &tagged("r/.pytest_cache")],
            &[("r/.pytest_cache", "build.pytest-cache")],
        ),
        (&[IN_REPO, "r/.pytest_cache/CACHEDIR.TAG=nothing"], &[]),
        (
            &[
                IN_REPO,
                &tagged("r/a/.mypy_cache"),
                "r/b/.mypy_cache/.gitignore",
            ],
            &[
                ("r/a/.mypy_cache", "build.mypy-cache"),
                ("r/b/.mypy_cache", "build.mypy-cache"),
            ],
        ),
        (&[IN_REPO, "r/.mypy_cache/"], &[]),
        (
            &[IN_REPO, &tagged("r/.ruff_cache")],
            &[("r/.ruff_cache", "build.ruff-cache")],
        ),
        (&[IN_REPO, "r/.ruff_cache/x"], &[]),
        (
            &[IN_REPO, "r/tox.ini", "r/.tox/"],
            &[("r/.tox", "build.tox")],
        ),
        (&[IN_REPO, "r/.tox/"], &[]),
        (
            &[
                IN_REPO,
                "r/cmake-build-debug/CMakeCache.txt",
                "r/cmake-build-debug/CMakeFiles/",
            ],
            &[("r/cmake-build-debug", "build.cmake")],
        ),
        (
            &[
                IN_REPO,
                "r/a/CMakeCache.txt",
                "r/b/CMakeFiles/",
                "r/CMakeFiles/",
            ],
            &[],
        ),
        (
            &[IN_REPO, "r/pubspec.yaml", "r/.dart_tool/", "r/build/"],
            &[
                ("r/.dart_tool", "build.dart-tool"),
                ("r/build", "build.flutter-build"),
            ],
        ),
        (&[IN_REPO, "r/.dart_tool/"], &[]),
        (
            &[
                IN_REPO,
                "r/build.zig",
                "r/.zig-cache/",
                "r/zig-cache/",
                "r/zig-out/",
            ],
            &[
                ("r/.zig-cache", "build.zig-cache"),
                ("r/zig-cache", "build.zig-cache"),
                ("r/zig-out", "build.zig-cache"),
            ],
        ),
        (&[IN_REPO, "r/zig-out/"], &[]),
        (
            &[IN_REPO, "r/mix.exs", "r/_build/"],
            &[("r/_build", "build.elixir")],
        ),
        (&[IN_REPO, "r/_build/"], &[]),
        (
            &[
                IN_REPO,
                "r/a/x.cabal",
                "r/a/dist-newstyle/",
                "r/b/stack.yaml",
                "r/b/.stack-work/",
            ],
            &[
                ("r/a/dist-newstyle", "build.haskell"),
                ("r/b/.stack-work", "build.haskell"),
            ],
        ),
        (&[IN_REPO, "r/dist-newstyle/", "r/.stack-work/"], &[]),
        (
            &[IN_REPO, "r/main.tf", "r/.terraform/"],
            &[("r/.terraform", "build.terraform")],
        ),
        (&[IN_REPO, "r/.terraform/"], &[]),
    ]);
}

/// A directory holding a valid `CACHEDIR.TAG`, as a `build` line.
fn tagged(dir: &str) -> String {
    format!("{dir}/{TAG}")
}

#[test]
fn bare_build_dist_out_cache_and_tmp_are_never_candidates() {
    let bare = [
        "build/",
        "dist/",
        "out/",
        "cache/",
        "tmp/",
        "target/",
        "node_modules/",
    ];
    let outside: Vec<String> = bare.iter().map(|d| format!("p/{d}")).collect();
    let inside: Vec<String> = bare.iter().map(|d| format!("r/{d}")).collect();
    let mut lines: Vec<&str> = vec![IN_REPO, "r/README.md", "p/package.json", "p/Cargo.toml"];
    lines.extend(outside.iter().map(String::as_str));
    lines.extend(inside.iter().map(String::as_str));
    lines.extend(["~/build/", "~/dist/", "~/cache/", "~/Library/build/"]);
    assert_eq!(found(&lines), BTreeSet::new());
}

#[test]
fn what_the_catalogue_leaves_out_is_never_a_candidate() {
    let lines = [
        "~/.m2/repository/org/x/",
        "~/.ollama/models/",
        "~/.cache/huggingface/hub/",
        "~/Library/Containers/com.docker.docker/Data/vms/0/",
        "~/Library/Developer/CoreSimulator/Devices/ABC/data/",
        "~/Library/Developer/Xcode/Archives/2024/",
        "~/Downloads/installer.dmg",
        "~/Parallels/Windows.pvm/",
        "~/Library/Application Support/SomeApp/data/",
        "~/Documents/build/",
    ];
    assert_eq!(found(&lines), BTreeSet::new());
}

#[test]
fn a_symlink_named_like_a_candidate_is_never_a_candidate_nor_followed() {
    let lines = [
        IN_REPO,
        "r/Cargo.toml",
        "r/target->../real-target",
        "real-target/debug/",
        "r/package.json",
        "r/node_modules->/",
        "~/Library/Caches/Homebrew->/",
        "~/Library/Logs->/",
        "~/.Trash->../somewhere",
    ];
    assert_eq!(found(&lines), BTreeSet::new());
}

#[test]
fn installed_packages_inside_a_git_managed_tool_are_never_build_output() {
    // Version managers and package prefixes are often Git clones that keep
    // installed packages in an ignored `lib/node_modules`, `site-packages`
    // or Homebrew `Cellar`.
    let lines = [
        "tool/.git/",
        "tool/lib/node_modules/npm/package.json",
        "tool/lib/node_modules/npm/node_modules/abbrev/",
        "tool/lib/node_modules/cli/package.json",
        "tool/lib/node_modules/cli/next.config.js",
        "tool/lib/node_modules/cli/.next/",
        "tool/lib/node_modules/cli/x/Cargo.toml",
        "tool/lib/node_modules/cli/x/target/",
        "tool/lib/python3.12/site-packages/pkg/__pycache__/m.pyc",
        "tool/lib/python3/dist-packages/pkg/__pycache__/m.pyc",
        "tool/Cellar/python/3.12/lib/python3.12/__pycache__/m.pyc",
        "tool/Cellar/x/1.0/package.json",
        "tool/Cellar/x/1.0/node_modules/",
        "tool/src/pkg/__pycache__/m.pyc",
    ];
    assert_eq!(
        found(&lines),
        BTreeSet::from([(
            "tool/src/pkg/__pycache__".to_owned(),
            "build.python-pycache".to_owned()
        )])
    );
    let project = [
        IN_REPO,
        "r/packages/a/package.json",
        "r/packages/a/node_modules/",
    ];
    assert_eq!(
        found(&project),
        BTreeSet::from([(
            "r/packages/a/node_modules".to_owned(),
            "build.node-modules".to_owned()
        )])
    );
}

#[test]
fn only_the_outermost_of_nested_candidates_is_recorded() {
    check(&[
        (
            &[
                IN_REPO,
                "r/package.json",
                "r/node_modules/a/package.json",
                "r/node_modules/a/node_modules/b/",
                "r/node_modules/a/__pycache__/x.pyc",
            ],
            &[("r/node_modules", "build.node-modules")],
        ),
        (
            &[
                "~/Library/Caches/com.example/r/.git/",
                "~/Library/Caches/com.example/r/package.json",
                "~/Library/Caches/com.example/r/node_modules/",
            ],
            &[("~/Library/Caches/com.example", "app-cache.library")],
        ),
        (
            &[
                IN_REPO,
                "r/Cargo.toml",
                "r/target/sub/.git=gitdir: /r/.git/worktrees/w\n",
            ],
            &[("r/target", "build.cargo-target")],
        ),
    ]);
}
