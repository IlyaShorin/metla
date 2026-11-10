//! The rule catalogue of docs/RULES.md, as data.
//!
//! Location patterns: `~` is the home directory, `*` exactly one path
//! component, `{a,b}` alternatives (also across `/`), `<user>` the user's
//! name, and `name[a|b]` a directory that must contain an entry `a` or `b`.

use crate::domain::Category::{self, AppCache, BuildArtifact, DevCache, GitWorktree, Log, Temp};

/// One rule: what it matches and why removing a match is safe.
#[derive(Debug)]
pub struct Rule {
    pub category: Category,
    /// The rule's own name; its id is `<category name>.<name>`.
    pub name: &'static str,
    pub title: &'static str,
    pub why: &'static str,
    pub matcher: Matcher,
    pub removes: Removes,
}

/// What a cleanup removes of a candidate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Removes {
    /// The directory and everything in it.
    Directory,
    /// What the directory holds: the directory is a place its owner
    /// expects to exist, so it stays.
    Contents,
}

#[derive(Debug)]
pub enum Matcher {
    /// Exact places a known tool owns. A `*` never matches a name on the
    /// deny list (`x*` denies every name starting with `x`).
    Location {
        patterns: &'static [&'static str],
        deny: &'static [&'static str],
    },
    /// Build output: a directory with one of `names` (any name when empty)
    /// inside a Git repository, proven by every marker of at least one
    /// group of `markers`.
    Project {
        names: &'static [&'static str],
        markers: &'static [&'static [Marker]],
    },
    /// A linked Git worktree: its `.git` is a file pointing into a
    /// repository's `worktrees/<name>` directory.
    LinkedWorktree,
}

/// Evidence that a directory is a tool's output. Names may be `*.ext` or
/// `prefix.*` patterns.
#[derive(Debug)]
pub enum Marker {
    /// An entry next to the directory.
    Sibling(&'static str),
    /// An entry inside the directory.
    Inner(&'static str),
    /// An inner `CACHEDIR.TAG` that starts with the standard signature.
    CacheDirTag,
}

use Marker::{CacheDirTag, Inner, Sibling};

const fn location(
    category: Category,
    name: &'static str,
    patterns: &'static [&'static str],
) -> Rule {
    denying(category, name, patterns, &[])
}

const fn denying(
    category: Category,
    name: &'static str,
    patterns: &'static [&'static str],
    deny: &'static [&'static str],
) -> Rule {
    Rule {
        category,
        name,
        title: "",
        why: "",
        matcher: Matcher::Location { patterns, deny },
        removes: Removes::Directory,
    }
}

const fn project(
    name: &'static str,
    names: &'static [&'static str],
    markers: &'static [&'static [Marker]],
) -> Rule {
    Rule {
        category: BuildArtifact,
        name,
        title: "",
        why: "",
        matcher: Matcher::Project { names, markers },
        removes: Removes::Directory,
    }
}

impl Rule {
    const fn says(self, title: &'static str, why: &'static str) -> Rule {
        Rule { title, why, ..self }
    }

    const fn keeps(self) -> Rule {
        Rule {
            removes: Removes::Contents,
            ..self
        }
    }
}

/// Apple's own caches in `~/Library/Caches`, which `app-cache.library`
/// never claims: system services keep sync and account state there.
const APPLE_CACHES: &[&str] = &[
    "com.apple.*",
    "CloudKit",
    "FamilyCircle",
    "familycircled",
    "GameKit",
    "GeoServices",
    "Metadata",
    "PassKit",
    "SiriTTS",
    "TelephonyUtilities",
    "askpermissiond",
    "storeassetd",
];

const NODE: &[Marker] = &[Sibling("package.json")];

pub const CATALOGUE: &[Rule] = &[
    denying(AppCache, "library", &["~/Library/Caches/*"], APPLE_CACHES)
        .says("Application cache", "Apps rebuild their cache directory.")
    .keeps(),
    denying(
        AppCache,
        "container",
        &["~/Library/Containers/*/Data/Library/Caches"],
        &["com.apple.*"],
    )
    .says("Sandboxed app cache", "A sandboxed app rebuilds its cache.")
    .keeps(),
    location(
        AppCache,
        "chromium",
        &["~/Library/Application Support/{Google/Chrome,BraveSoftware/Brave-Browser,Microsoft Edge,Arc/User Data,Chromium}/*[Preferences]/{Cache,Code Cache,GPUCache,Service Worker/CacheStorage}"],
    )
    .says("Browser cache", "The browser downloads it again.")
    .keeps(),
    location(
        AppCache,
        "electron",
        &["~/Library/Application Support/*[Local Storage|Preferences]/{Cache,Code Cache,GPUCache,CachedData}"],
    )
    .says("Electron app cache", "The app's runtime rebuilds it.")
    .keeps(),
    location(AppCache, "firefox", &["~/Library/Caches/Firefox/Profiles/*/cache2"])
        .says("Firefox cache", "The browser downloads it again.")
    .keeps(),
    location(DevCache, "homebrew", &["~/Library/Caches/Homebrew"])
        .says("Homebrew downloads", "Homebrew downloads them again.")
    .keeps(),
    location(DevCache, "npm", &["~/.npm/_cacache"]).says("npm cache", "npm downloads it again.")
    .keeps(),
    location(DevCache, "yarn", &["~/Library/Caches/Yarn", "~/.yarn/berry/cache"])
        .says("Yarn cache", "Yarn downloads it again.")
    .keeps(),
    location(DevCache, "pnpm", &["~/Library/pnpm/store", "~/Library/Caches/pnpm"])
        .says("pnpm store", "pnpm downloads it again.")
    .keeps(),
    location(DevCache, "bun", &["~/.bun/install/cache"]).says("Bun cache", "Bun downloads it again.")
    .keeps(),
    location(DevCache, "deno", &["~/Library/Caches/deno"]).says("Deno cache", "Deno downloads it again.")
    .keeps(),
    location(DevCache, "pip", &["~/Library/Caches/pip"]).says("pip cache", "pip downloads it again.")
    .keeps(),
    location(DevCache, "uv", &["~/.cache/uv"]).says("uv cache", "uv downloads it again.")
    .keeps(),
    location(DevCache, "poetry", &["~/Library/Caches/pypoetry"])
        .says("Poetry cache", "Poetry downloads it again.")
    .keeps(),
    location(
        DevCache,
        "cargo-registry",
        &["~/.cargo/registry/{cache,src}", "~/.cargo/git/checkouts"],
    )
    .says("Cargo downloads", "Cargo downloads and unpacks them again.")
    .keeps(),
    location(DevCache, "go-build", &["~/Library/Caches/go-build"])
        .says("Go build cache", "Go rebuilds it.")
    .keeps(),
    location(DevCache, "go-mod", &["~/go/pkg/mod/cache/download"])
        .says("Go module downloads", "Go downloads them again.")
    .keeps(),
    location(DevCache, "gradle", &["~/.gradle/{caches,daemon}"])
        .says("Gradle caches", "Gradle downloads and rebuilds them.")
    .keeps(),
    location(DevCache, "cocoapods", &["~/Library/Caches/CocoaPods"])
        .says("CocoaPods cache", "CocoaPods downloads it again.")
    .keeps(),
    location(DevCache, "swiftpm", &["~/Library/Caches/org.swift.swiftpm"])
        .says("Swift package cache", "SwiftPM downloads it again.")
    .keeps(),
    location(DevCache, "xcode-derived-data", &["~/Library/Developer/Xcode/DerivedData/*"])
        .says("Xcode derived data", "Xcode rebuilds it."),
    location(
        DevCache,
        "xcode-device-support",
        &["~/Library/Developer/Xcode/{iOS,watchOS,tvOS,visionOS} DeviceSupport/*"],
    )
    .says("Device support files", "Xcode copies them again from a connected device."),
    location(DevCache, "core-simulator", &["~/Library/Developer/CoreSimulator/Caches"])
        .says("Simulator caches", "The simulator rebuilds them.")
    .keeps(),
    location(DevCache, "jetbrains", &["~/Library/Caches/JetBrains/*"])
        .says("JetBrains IDE cache", "The IDE rebuilds its indexes."),
    location(
        DevCache,
        "vscode",
        &["~/Library/Application Support/{Code,Cursor,VSCodium}/{CachedData,CachedExtensionVSIXs,Cache,Code Cache,GPUCache}"],
    )
    .says("Editor cache", "The editor rebuilds it.")
    .keeps(),
    location(DevCache, "playwright", &["~/Library/Caches/ms-playwright"])
        .says("Playwright browsers", "Playwright downloads them again.")
    .keeps(),
    location(DevCache, "bazel", &["/private/var/tmp/_bazel_<user>"])
        .says("Bazel output base", "Bazel rebuilds it.")
    .keeps(),
    location(DevCache, "ccache", &["~/Library/Caches/ccache", "~/.ccache"])
        .says("ccache", "The compiler cache refills itself.")
    .keeps(),
    location(DevCache, "sccache", &["~/Library/Caches/Mozilla.sccache"])
        .says("sccache", "The compiler cache refills itself.")
    .keeps(),
    location(DevCache, "flutter-pub", &["~/.pub-cache/hosted"])
        .says("Dart package cache", "pub downloads it again.")
    .keeps(),
    location(DevCache, "terraform", &["~/.terraform.d/plugin-cache"])
        .says("Terraform plugin cache", "Terraform downloads it again.")
    .keeps(),
    project(
        "cargo-target",
        &["target"],
        &[
            &[Sibling("Cargo.toml")],
            &[CacheDirTag, Inner(".rustc_info.json")],
        ],
    )
    .says("Cargo build output", "cargo build recreates it."),
    project("node-modules", &["node_modules"], &[NODE])
        .says("Node packages", "The package manager installs them again."),
    project(
        "next",
        &[".next"],
        &[&[Sibling("package.json"), Sibling("next.config.*")]],
    )
    .says("Next.js build output", "next build recreates it."),
    project(
        "nuxt",
        &[".nuxt"],
        &[&[Sibling("package.json"), Sibling("nuxt.config.*")]],
    )
    .says("Nuxt build output", "nuxt build recreates it."),
    project("turbo", &[".turbo"], &[NODE, &[Sibling("turbo.json")]])
        .says("Turborepo cache", "Turborepo rebuilds it."),
    project("parcel-cache", &[".parcel-cache"], &[NODE])
        .says("Parcel cache", "Parcel rebuilds it."),
    project("swiftpm", &[".build"], &[&[Sibling("Package.swift")]])
        .says("SwiftPM build output", "swift build recreates it."),
    project(
        "xcode-derived-data",
        &["DerivedData"],
        &[&[Sibling("*.xcodeproj")], &[Sibling("*.xcworkspace")]],
    )
    .says("Xcode project build output", "Xcode rebuilds it."),
    project("cocoapods", &["Pods"], &[&[Sibling("Podfile")]])
        .says("CocoaPods dependencies", "pod install recreates them."),
    project(
        "gradle-build",
        &["build"],
        &[&[Sibling("build.gradle")], &[Sibling("build.gradle.kts")]],
    )
    .says("Gradle build output", "Gradle recreates it."),
    project(
        "gradle-cache",
        &[".gradle"],
        &[
            &[Sibling("settings.gradle")],
            &[Sibling("settings.gradle.kts")],
            &[Sibling("build.gradle")],
            &[Sibling("build.gradle.kts")],
        ],
    )
    .says("Gradle project cache", "Gradle recreates it."),
    project("maven-target", &["target"], &[&[Sibling("pom.xml")]])
        .says("Maven build output", "mvn package recreates it."),
    project("python-pycache", &["__pycache__"], &[&[Inner("*.pyc")]])
        .says("Python bytecode", "Python compiles it again."),
    project("pytest-cache", &[".pytest_cache"], &[&[CacheDirTag]])
        .says("pytest cache", "pytest recreates it."),
    project(
        "mypy-cache",
        &[".mypy_cache"],
        &[&[CacheDirTag], &[Inner(".gitignore")]],
    )
    .says("mypy cache", "mypy recreates it."),
    project("ruff-cache", &[".ruff_cache"], &[&[CacheDirTag]])
        .says("Ruff cache", "Ruff recreates it."),
    project("tox", &[".tox"], &[&[Sibling("tox.ini")]])
        .says("tox environments", "tox recreates them."),
    project(
        "cmake",
        &[],
        &[&[Inner("CMakeCache.txt"), Inner("CMakeFiles")]],
    )
    .says("CMake build directory", "CMake recreates it."),
    project("dart-tool", &[".dart_tool"], &[&[Sibling("pubspec.yaml")]])
        .says("Dart tool cache", "dart pub get recreates it."),
    project("flutter-build", &["build"], &[&[Sibling("pubspec.yaml")]])
        .says("Flutter build output", "flutter build recreates it."),
    project(
        "zig-cache",
        &[".zig-cache", "zig-cache", "zig-out"],
        &[&[Sibling("build.zig")]],
    )
    .says("Zig build output", "zig build recreates it."),
    project("elixir", &["_build"], &[&[Sibling("mix.exs")]])
        .says("Mix build output", "mix compile recreates it."),
    project(
        "haskell",
        &["dist-newstyle", ".stack-work"],
        &[&[Sibling("*.cabal")], &[Sibling("stack.yaml")]],
    )
    .says("Haskell build output", "cabal or stack recreates it."),
    project("terraform", &[".terraform"], &[&[Sibling("*.tf")]])
        .says("Terraform working directory", "terraform init recreates it."),
    location(Log, "user-library", &["~/Library/Logs/*"])
        .says("Application logs", "Logs are history, not state; they go to the Trash."),
    location(Log, "diagnostic-reports", &["~/Library/Logs/DiagnosticReports"])
        .says("Crash and diagnostic reports", "Reports are history, not state; they go to the Trash.")
    .keeps(),
    location(Log, "app-support", &["~/Library/Application Support/*/{logs,Logs}"])
        .says("Application logs", "Logs are history, not state; they go to the Trash."),
    location(Temp, "trash", &["~/.Trash"])
        .says("Trash", "Everything in it was already thrown away.")
    .keeps(),
    Rule {
        category: GitWorktree,
        name: "linked",
        title: "Linked Git worktree",
        why: "Removed only when clean, pushed, unlocked and not managed by another tool.",
        matcher: Matcher::LinkedWorktree,
        removes: Removes::Directory,
    },
];
