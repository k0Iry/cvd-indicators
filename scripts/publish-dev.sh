#!/usr/bin/env bash
# Refresh the rolling `dev` pre-release from the CI gate's dist (CI publisher
# job only; tested locally with a mock `gh`/`git` by conformance/tests/publish_dev.rs).
#   scripts/publish-dev.sh <DIST_DIR>      (run from the repository checkout)
# Env: GITHUB_SHA, GITHUB_REPOSITORY, GH_TOKEN. Tools: git gh jq sha256sum.
#
# Publishes only if <DIST_DIR> is exactly the six allowlisted files, every hash
# matches, and manifest.json names release `dev`, this commit and this
# checkout's Cargo.lock SDK pin; and only while this commit is still the remote
# master head (checked first and again right before promotion; stale -> exit 0,
# public dev unchanged). Writes only `dev`, refs/tags/dev and this run's draft.
#
# 1. upload the complete set into a NEW hidden draft release (tag_name
#    dev-staging-<sha>), download it back by asset id, byte-compare;
# 2. re-check the remote master head;
# 3. promote: hide the previous dev release (draft, tag_name dev-previous-<id>),
#    move refs/tags/dev, publish the draft as `dev` (prerelease, make_latest=false);
# 4. read the public `dev` back, then delete the hidden previous release.
# A failure in 3/4 attempts rollback (hide new, restore ref + previous release).
# If remote cleanup/rollback fails, the run fails with affected release ids;
# publication state then requires manual verification. Between hide and publish
# (seconds), dev downloads are unavailable; assets are never overwritten in place.
set -euo pipefail
DIST="$(cd "${1:?usage: publish-dev.sh <DIST_DIR>}" && pwd)"
: "${GITHUB_SHA:?}" "${GITHUB_REPOSITORY:?}"
[[ "$GITHUB_SHA" =~ ^[0-9a-f]{40}$ ]] || { echo "bad GITHUB_SHA" >&2; exit 1; }
R="repos/$GITHUB_REPOSITORY"
TAG=dev
STAGE="dev-staging-$GITHUB_SHA"
WASM=(cvd_indicator_cvd.wasm cvd_indicator_cvd_anchored.wasm cvd_indicator_example_range.wasm cvd_indicator_flow.wasm cvd_indicator_orderbook.wasm)
SET="SHA256SUMS ${WASM[*]} manifest.json"
LOCK_SDK=$(awk '$0=="name = \"cvd-indicator-sdk\""{f=1;next} f&&/^source = /{gsub(/^source = "|"$/,"");print;exit}' Cargo.lock)
[[ "$LOCK_SDK" =~ ^git\+https://github\.com/k0Iry/cvd-core\.git\?rev=[0-9a-f]{40}#[0-9a-f]{40}$ ]] || { echo "unexpected SDK pin '$LOCK_SDK'" >&2; exit 1; }
NOTES="Rolling development build of master $GITHUB_SHA (replaced on every master push; not a formal release). Formal versions are the vX.Y.Z releases. Verify downloads against SHA256SUMS / manifest.json."

verify() { # $1: dir holding exactly the release set for this commit
  local got sums
  got=$(cd "$1" && find . -mindepth 1 -maxdepth 1 -printf '%f\n' | LC_ALL=C sort | xargs)
  [[ "$got" == "$SET" ]] || { echo "$1: file set '$got' != '$SET'" >&2; return 1; }
  sums=$(awk '{print $2}' "$1/SHA256SUMS" | LC_ALL=C sort | xargs)
  [[ "$sums" == "${WASM[*]} manifest.json" ]] || { echo "$1: SHA256SUMS lists '$sums'" >&2; return 1; }
  (cd "$1" && sha256sum --quiet --strict -c SHA256SUMS) || return 1
  jq -e --arg c "$GITHUB_SHA" --arg s "$LOCK_SDK" \
    '.release == "dev" and .source == {commit: $c} and .sdk.source == $s' "$1/manifest.json" >/dev/null \
    || { echo "$1: manifest release/source/sdk mismatch" >&2; return 1; }
}
same_as_dist() { # $1: downloaded dir -> verified and byte-identical to DIST
  verify "$1" || return 1
  local f; for f in $SET; do cmp "$DIST/$f" "$1/$f" || return 1; done
}
api_get() { # GET $1 -> stdout; exit 3 on HTTP 404, 1 on any other error
  local out err
  err=$(mktemp)
  if out=$(gh api "$1" 2>"$err"); then rm -f "$err"; printf '%s' "$out"; return 0; fi
  if grep -q 'HTTP 404' "$err"; then rm -f "$err"; return 3; fi
  cat "$err" >&2; rm -f "$err"; return 1
}
master_is_us() { # 0 = still the head, 10 = moved (stale), 1 = cannot tell
  local head
  head=$(git ls-remote --exit-code origin refs/heads/master | cut -f1) || return 1
  [[ "$head" =~ ^[0-9a-f]{40}$ ]] || return 1
  [[ "$head" == "$GITHUB_SHA" ]] && return 0
  echo "::notice::master is now $head; $GITHUB_SHA is stale, public dev not changed"; return 10
}
download() { # $1 release json, $2 dir: every asset by id (also works for drafts)
  local id name
  mkdir -p "$2"
  while read -r id name; do
    [[ "$name" =~ ^[A-Za-z0-9._-]+$ ]] || return 1
    gh api -H 'Accept: application/octet-stream' "$R/releases/assets/$id" >"$2/$name" || return 1
  done < <(jq -r '.assets[] | "\(.id) \(.name)"' <<<"$1")
}
ref_sha() { local j; j=$(api_get "$R/git/ref/tags/$TAG") || return $?; jq -r .object.sha <<<"$j"; }

verify "$DIST"
rc=0; master_is_us || rc=$?
[[ $rc == 10 ]] && exit 0
[[ $rc == 0 ]] || { echo "cannot resolve remote master" >&2; exit 1; }

WORK=$(mktemp -d)
OLD_ID=""; NEW_ID=""; PHASE=pre; HID_OLD=0; REF_MOVED=0; NEW_PUB=0
trap 'rm -rf "$WORK"' EXIT

rc=0; old=$(api_get "$R/releases/tags/$TAG") || rc=$?
[[ $rc == 0 || $rc == 3 ]] || exit 1
rc2=0; OLD_REF=$(ref_sha) || rc2=$?
[[ $rc2 == 0 || $rc2 == 3 ]] || exit 1
[[ $rc2 == 3 ]] && OLD_REF=""
if [[ $rc == 0 ]]; then
  OLD_ID=$(jq -er .id <<<"$old")
  # Already current (tag target, flags, exact bytes)? -> no writes.
  if [[ "$OLD_REF" == "$GITHUB_SHA" ]] && jq -e '.prerelease and (.draft | not)' <<<"$old" >/dev/null \
    && download "$old" "$WORK/cur" && same_as_dist "$WORK/cur" 2>/dev/null; then
    echo "dev already publishes $GITHUB_SHA"; exit 0
  fi
fi

drop_new() {
  gh api -X DELETE "$R/releases/$NEW_ID" >/dev/null || echo "::warning::draft $NEW_ID not deleted" >&2
}
rollback() { # restore the previous public state; 1 = could not
  local ok=0
  if ((NEW_PUB)); then
    if ! gh api -X PATCH "$R/releases/$NEW_ID" -F draft=true -f tag_name="$STAGE" >/dev/null; then
      gh api -X DELETE "$R/releases/$NEW_ID" >/dev/null && NEW_ID="" \
        || echo "::error::unverified dev release $NEW_ID could not be hidden or deleted" >&2
      ok=1
    fi
  fi
  if ((REF_MOVED)); then
    if [[ -n "$OLD_REF" ]]; then
      gh api -X PATCH "$R/git/refs/tags/$TAG" -f sha="$OLD_REF" -F force=true >/dev/null || ok=1
    else
      gh api -X DELETE "$R/git/refs/tags/$TAG" >/dev/null || ok=1
    fi
  fi
  if ((HID_OLD)); then
    if ((ok == 0)); then
      gh api -X PATCH "$R/releases/$OLD_ID" -F draft=false -f tag_name="$TAG" -F prerelease=true \
        -f make_latest=false >/dev/null || ok=1
    fi
    ((ok == 0)) || echo "::error::previous dev release $OLD_ID left hidden as draft dev-previous-$OLD_ID" >&2
  fi
  if ((ok == 0)) && [[ -n "$NEW_ID" ]]; then drop_new; fi
  return $ok
}
on_exit() {
  local st=$?
  set +e
  if ((st != 0)); then
    case $PHASE in
      staged) drop_new ;;
      promoting) if rollback; then echo "::error::dev update failed; previous dev restored" >&2
        else echo "::error::dev update AND rollback failed; no verified dev is public" >&2; fi ;;
    esac
  fi
  rm -rf "$WORK"
  exit "$st"
}
trap on_exit EXIT

# 1. stage a hidden draft holding the complete set; verify its remote bytes
new=$(gh api -X POST "$R/releases" -f tag_name="$STAGE" -f target_commitish="$GITHUB_SHA" \
  -f name="dev (rolling master build)" -f body="$NOTES" -F draft=true -F prerelease=true \
  -f make_latest=false)
NEW_ID=$(jq -er .id <<<"$new"); PHASE=staged
for f in $SET; do
  gh api -X POST -H 'Content-Type: application/octet-stream' --input "$DIST/$f" \
    "https://uploads.github.com/$R/releases/$NEW_ID/assets?name=$f" >/dev/null
done
staged=$(api_get "$R/releases/$NEW_ID")
jq -e '.draft' <<<"$staged" >/dev/null
download "$staged" "$WORK/staged"
same_as_dist "$WORK/staged"

# 2. late freshness check, under the serialized publisher job
rc=0; master_is_us || rc=$?
if [[ $rc == 10 ]]; then drop_new; PHASE=finished; exit 0; fi
[[ $rc == 0 ]] || { echo "cannot resolve remote master" >&2; exit 1; }

# 3. promote (publication commit point); any failure from here rolls back
PHASE=promoting
if [[ -n "$OLD_ID" ]]; then
  HID_OLD=1
  gh api -X PATCH "$R/releases/$OLD_ID" -F draft=true -f tag_name="dev-previous-$OLD_ID" >/dev/null
fi
if [[ -n "$OLD_REF" ]]; then
  REF_MOVED=1 # restoring OLD_REF is idempotent even if this PATCH did not land
  gh api -X PATCH "$R/git/refs/tags/$TAG" -f sha="$GITHUB_SHA" -F force=true >/dev/null
else
  gh api -X POST "$R/git/refs" -f ref="refs/tags/$TAG" -f sha="$GITHUB_SHA" >/dev/null
  REF_MOVED=1
fi
NEW_PUB=1
gh api -X PATCH "$R/releases/$NEW_ID" -f tag_name="$TAG" -F draft=false -F prerelease=true \
  -f make_latest=false >/dev/null

# 4. public readback: `dev` must now serve exactly the verified set
pub=$(api_get "$R/releases/tags/$TAG")
[[ "$(jq -r .id <<<"$pub")" == "$NEW_ID" ]]
jq -e '.prerelease and (.draft | not)' <<<"$pub" >/dev/null
download "$pub" "$WORK/back"
same_as_dist "$WORK/back"
[[ "$(ref_sha)" == "$GITHUB_SHA" ]]
latest=$(api_get "$R/releases/latest" | jq -r .tag_name)
[[ -n "$latest" && "$latest" != "$TAG" ]]
PHASE=finished
if [[ -n "$OLD_ID" ]]; then
  gh api -X DELETE "$R/releases/$OLD_ID" >/dev/null || echo "::warning::previous dev draft $OLD_ID not deleted" >&2
fi
echo "dev now publishes $GITHUB_SHA"
