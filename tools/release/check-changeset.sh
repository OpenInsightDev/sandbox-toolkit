# Fails when a branch changes what we ship without a changeset. The changeset is
# what moves the packages, and with them the crate, to the next version: without
# one the change never reaches a release.
#
# Usage: check-changeset.sh [base-ref]
set -eu

base=${1:-origin/main}
changed=$(git diff --name-only --diff-filter=ACMR "$base...HEAD")

if printf '%s\n' "$changed" | grep -E '^\.changeset/[^/]+\.md$' |
    grep -qv '^\.changeset/README\.md$'; then
    exit 0
fi

# Sources that ship. Documentation, tests and tooling do not, wherever they sit.
relevant=$(printf '%s\n' "$changed" |
    grep -E '^(crates|packages)/' |
    grep -vE '(^|/)(tests?|docs?)/|\.md$' || true)

if [ -z "$relevant" ]; then
    exit 0
fi

cat >&2 <<EOF
This change ships without a changeset:

$relevant

Add one with \`pnpm exec changeset\`, or an empty one with
\`pnpm exec changeset --empty\` when nothing should be released.
EOF
exit 1
