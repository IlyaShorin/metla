#!/usr/bin/env bash
# Writes the architecture view's IR for uml-viewer
# (https://github.com/unclebob/uml-viewer) from
# docs/architecture/metla.policy.edn: target/architecture/metla.edn, the
# Rust core's modules with their dependencies, and the levels that mark
# a dependency breaking the rule of docs/ARCHITECTURE.md section 2. Show
# it with uml-viewer (`clj -M:run <repo>/target/architecture/metla.edn` in
# its checkout).
#
# Needs the Clojure CLI and a uml-viewer checkout (UML_VIEWER, default
# ~/.local/share/ub-tools/uml-viewer). The generator runs in the
# repository root, so the policy's source roots are relative to it; the
# classpath is the viewer's own (its `:ir` alias), so nothing is cached in
# the repository.

set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
viewer="${UML_VIEWER:-$HOME/.local/share/ub-tools/uml-viewer}"
[[ -f "$viewer/deps.edn" ]] || { echo "no uml-viewer at $viewer (set UML_VIEWER)" >&2; exit 2; }
command -v clojure >/dev/null || { echo "the Clojure CLI (clojure) is needed" >&2; exit 2; }

# The viewer's classpath, its relative entries made absolute.
classpath="$(cd "$viewer" && clojure -Spath -M:ir)"
absolute=""
IFS=: read -r -a entries <<<"$classpath"
for entry in "${entries[@]}"; do
    [[ "$entry" == /* ]] || entry="$viewer/$entry"
    absolute="${absolute:+$absolute:}$entry"
done

cd "$repo"
mkdir -p target/architecture
clojure -Scp "$absolute" -M -m uml-viewer.main.ir-generator \
    docs/architecture/metla.policy.edn target/architecture/metla.edn
