#!/usr/bin/env bash
set -euo pipefail

# Only successful main runs and PR runs explicitly approved by a successful
# main run can supply artifacts to another run.
list_runs() {
  gh api --paginate --method GET "repos/$GITHUB_REPOSITORY/actions/artifacts" \
    -f name="$1" -F per_page=100 --jq '.artifacts[] | select(.expired == false) | .workflow_run.id'
}

source_type() {
  gh api "repos/$GITHUB_REPOSITORY/actions/runs/$1" | jq -r --arg repo "$GITHUB_REPOSITORY" '
    select(.path == ".github/workflows/release.yml" and .conclusion == "success" and
      .head_repository.full_name == $repo) |
    if .event == "push" and .head_branch == "main" then "main"
    elif .event == "pull_request" and .run_attempt == 1 then "pr"
    else empty end
  '
}

approved_on_main() {
  local approvals approval type
  if ! approvals="$(list_runs "native-approved-run-$1")"; then
    echo "::warning::Could not list native artifact approvals for run $1." >&2
    return 1
  fi
  for approval in $approvals; do
    if ! type="$(source_type "$approval")"; then
      echo "::warning::Could not verify native artifact approval from run $approval." >&2
      continue
    fi
    [[ "$type" == main ]] && return 0
  done
  return 1
}

trusted_artifact_run() {
  local candidates run_id type
  if ! candidates="$(list_runs "$1")"; then
    echo "::warning::Could not list $1; using local native work." >&2
    return 1
  fi
  while IFS= read -r run_id; do
    [[ -n "$run_id" ]] || continue
    if ! type="$(source_type "$run_id")"; then
      echo "::warning::Could not verify native artifact source run $run_id." >&2
      continue
    fi
    if [[ "$type" == main ]] || { [[ "$type" == pr ]] && approved_on_main "$run_id"; }; then
      printf '%s\n' "$run_id"
      return 0
    fi
  done <<< "$candidates"
  return 1
}

restore_archive() {
  local run_id download archive
  run_id="$(trusted_artifact_run "$ARCHIVE_NAME")" || return 1
  download="$RUNNER_TEMP/native-artifact"
  archive="$download/mekugi-native_${PLATFORM}.tar.gz"
  if ! gh run download "$run_id" --repo "$GITHUB_REPOSITORY" --name "$ARCHIVE_NAME" --dir "$download" ||
     [[ ! -s "$archive" ]]; then
    echo "::warning::Could not restore native archive from run $run_id; building locally."
    return 1
  fi
  mkdir -p "$RUNNER_TEMP/artifacts" && mv "$archive" "$RUNNER_TEMP/artifacts/"
}

package=true
if [[ "$ARCHIVE_CACHE_HIT" == true ]]; then
  package=false
  echo "Native archive: exact cache hit."
elif restore_archive; then
  package=false
  echo "Native archive: restored a verified artifact."
fi

test=true
receipt="$RUNNER_TEMP/native-tests-passed"
if [[ "$NATIVE_PACKAGE_PROFILE" == dev-small ]]; then
  if [[ "$TEST_CACHE_HIT" == true && -f "$receipt" && "$(<"$receipt")" == "$TEST_RECEIPT" ]]; then
    test=false
    echo "Native tests: exact cached receipt."
  elif receipt_run="$(trusted_artifact_run "$TEST_RECEIPT")"; then
    printf '%s\n' "$TEST_RECEIPT" > "$receipt"
    test=false
    echo "Native tests: verified a passing receipt from run $receipt_run."
  fi
fi
printf 'test=%s\npackage=%s\n' "$test" "$package" >> "$GITHUB_OUTPUT"
