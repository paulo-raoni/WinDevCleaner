#!/usr/bin/env bash
# Guard check: shows that every correctness test fails when its guarding line is removed.
#
# Each guarding line in the product code sits between `GUARD-BEGIN(name)` and
# `GUARD-END(name)` comments. For every guard this script deletes those lines, rebuilds,
# runs the tests that depend on the guard, and requires each of them to fail at a guarded
# assertion (a panic starting with "GUARD-CHECK failed"), not at an unrelated one. It first
# runs every listed test on the unchanged code, where all must pass.
#
# Writes a Markdown table to $REPORT. Exits non-zero if a guard survives (a test still
# passes without it), a mutant does not compile, or a baseline test fails.
set -u
cd "$(dirname "$0")/.."

PROFILE="${PROFILE:-ci-test}"
REPORT="${REPORT:-ci-out/guard-check.md}"
LOGS="${LOGS:-ci-out/guard-logs}"
mkdir -p "$(dirname "$REPORT")" "$LOGS"

# guard | package | test target | test name
CASES=(
  "hardlink-dedup|wdc-core|fixtures|hard_link_with_both_names_inside_the_root_counts_once"
  "hardlink-dedup|wdc-core|fixtures|hard_link_with_a_name_outside_the_root_counts_once_and_reports_both_names"
  "large-file-recheck|wdc-core|fixtures|hard_link_with_a_name_outside_the_root_counts_once_and_reports_both_names"
  "large-file-recheck|wdc-core|fixtures|stale_size_in_another_names_directory_entry_is_corrected"
  "no-follow-links|wdc-core|fixtures|junctions_and_symbolic_links_are_recorded_and_never_followed"
  "root-link|wdc-core|fixtures|a_root_that_is_a_junction_is_recorded_and_not_followed"
  "size-on-disk|wdc-core|fixtures|a_plain_file_counts_its_allocation_not_its_length"
  "size-on-disk|wdc-core|fixtures|sparse_files_count_their_allocation"
  "size-on-disk|wdc-core|fixtures|compressed_files_count_their_compressed_allocation"
  "placeholder-zero|wdc-core|fixtures|offline_files_count_zero_and_are_never_opened"
  "background-mode|wdc-cli|million|a_million_files_stay_under_the_memory_and_time_limits_in_background_mode"
  "pause-gate|wdc-core|pipe|a_paused_agents_calls_wait_until_resumed"
  "pipe-dacl|wdc-core|pipe|another_user_cannot_open_the_pipe"
  "activity-mirror|wdc-app|mcp_mirror|an_mcp_client_drives_a_scan_and_the_app_mirrors_it_live"
  "diff-new|wdc-cli|growth|diff_reports_new_grown_and_removed_folders_between_two_scans"
  "diff-grown|wdc-cli|growth|diff_reports_new_grown_and_removed_folders_between_two_scans"
  "snapshot-retention|wdc-cli|growth|snapshot_retention_keeps_only_the_last_ten_and_reports_their_size"
  "recycle-bin|wdc-core|system|recycle_bin_size_comes_from_the_shell"
  "system-file-size|wdc-core|system|paging_swap_and_hibernation_files_match_the_volume_listing"
  "listed-in-file|wdc-core|rules|vscode_obsolete_extension_needs_the_obsolete_list_no_reference_and_vscode_closed"
  "not-referenced-by|wdc-core|rules|vscode_obsolete_extension_needs_the_obsolete_list_no_reference_and_vscode_closed"
  "process-not-running|wdc-core|rules|vscode_obsolete_extension_needs_the_obsolete_list_no_reference_and_vscode_closed"
  "name-matches-regex|wdc-core|rules|vscode_guid_staging_folder_needs_a_day_and_no_reference"
  "older-than|wdc-core|rules|vscode_guid_staging_folder_needs_a_day_and_no_reference"
  "driver-version|wdc-core|rules|nvidia_artifacts_older_equal_and_newer_than_the_installed_driver"
  "nvidia-steps|wdc-core|rules|nvidia_newer_unknown_and_unversioned_artifacts_get_no_command"
  "local-report-only|wdc-core|rules|a_local_rule_cannot_make_an_item_actionable"
  "local-origin|wdc-core|rules|a_local_rule_cannot_make_an_item_actionable"
  "unknown-flag|wdc-core|rules|large_folders_no_rule_knows_are_flagged"
  "expected-freed|wdc-core|rules|the_cowork_case_two_names_one_outside_expected_freed_zero"
  "plan-age|wdc-core|apply_flow|a_plan_older_than_a_day_or_edited_is_refused_whole"
  "apply-file-id|wdc-core|apply_flow|an_item_replaced_after_planning_is_skipped"
  "apply-file-id|wdc-core|apply_flow|an_item_replaced_after_planning_is_skipped_for_every_quarantine_rule"
  "apply-conditions|wdc-core|apply_flow|a_condition_that_became_false_skips_the_item"
  "apply-conditions|wdc-core|apply_flow|a_condition_that_became_false_skips_the_item_for_every_quarantine_rule"
  "apply-conditions|wdc-cli|apply_cli|a_program_started_after_planning_keeps_its_item"
  "apply-verdict|wdc-core|apply_flow|forbidden_places_and_non_items_are_refused_on_the_real_path"
  "apply-verdict|wdc-core|apply_windows|forbidden_places_are_refused_however_the_path_is_written"
  "apply-rematch|wdc-core|apply_flow|forbidden_places_and_non_items_are_refused_on_the_real_path"
  "apply-link|wdc-core|apply_windows|a_junction_planned_or_swapped_in_moves_nothing"
  "apply-link|wdc-core|apply_windows|a_junction_swapped_in_for_a_shader_or_steam_cache_moves_nothing"
  "quarantine-plain|wdc-core|apply_windows|a_quarantine_folder_that_is_a_link_is_refused"
  "restore-free-path|wdc-core|apply_flow|restore_refuses_when_the_folder_was_recreated"
  "restore-free-path|wdc-core|apply_flow|restore_refuses_when_the_folder_was_recreated_for_every_quarantine_rule"
  "purge-age|wdc-core|apply_flow|early_purge_is_only_for_caches_and_leftovers_that_were_explained"
  "early-purge-class|wdc-core|apply_flow|early_purge_is_only_for_caches_and_leftovers_that_were_explained"
  "early-purge-class|wdc-cli|apply_cli|purge_before_seven_days_is_only_for_caches_and_leftovers_on_both_paths"
  "presence-code|wdc-cli|apply_cli|apply_with_the_desktop_code_then_restore_gives_the_identical_tree"
)

run_test() { # package target name log
  cargo test --profile "$PROFILE" --locked -p "$1" --test "$2" -- --include-ignored --exact "$3" --nocapture >"$4" 2>&1
}

status=0
echo "## Guard check: every correctness test fails without its guarding line" >"$REPORT"
echo >>"$REPORT"

echo "::group::build (unchanged code)"
cargo build --profile "$PROFILE" --locked --workspace --bins || exit 1
echo "::endgroup::"

echo "### Baseline (unchanged code): every test passes" >>"$REPORT"
echo >>"$REPORT"
for c in $(printf '%s\n' "${CASES[@]}" | cut -d'|' -f2-4 | sort -u); do
  IFS='|' read -r pkg target name <<<"$c"
  log="$LOGS/baseline-$name.log"
  if run_test "$pkg" "$target" "$name" "$log"; then
    echo "- \`$name\`: passes" >>"$REPORT"
  else
    echo "- \`$name\`: **FAILS on unchanged code**" >>"$REPORT"
    status=1
    tail -n 40 "$log"
  fi
done
echo >>"$REPORT"

echo "| Guard | Removed lines | Test | Without the guard |" >>"$REPORT"
echo "|---|---|---|---|" >>"$REPORT"
for guard in $(printf '%s\n' "${CASES[@]}" | cut -d'|' -f1 | awk '!seen[$0]++'); do
  files=$(grep -rl --include='*.rs' "GUARD-BEGIN($guard)" crates)
  if [ "$(echo "$files" | wc -w)" != 1 ]; then
    echo "| $guard | not found exactly once: $files | | **error** |" >>"$REPORT"
    status=1
    continue
  fi
  file=$files
  begin=$(grep -n "GUARD-BEGIN($guard)" "$file" | cut -d: -f1)
  end=$(grep -n "GUARD-END($guard)" "$file" | cut -d: -f1)
  code=$(sed -n "$((begin + 1)),$((end - 1))p" "$file" | sed 's/^ *//' | tr '\n' ' ' | sed 's/|/\\|/g; s/ *$//')
  where="\`$file:$((begin + 1))-$((end - 1))\`: \`$code\`"
  echo "::group::guard $guard ($file:$begin-$end)"
  sed -i "${begin},${end}d" "$file"
  if ! cargo build --profile "$PROFILE" --locked --workspace --bins >"$LOGS/build-$guard.log" 2>&1; then
    echo "| $guard | $where | (build) | **does not compile** |" >>"$REPORT"
    tail -n 30 "$LOGS/build-$guard.log"
    status=1
    git checkout -- "$file"
    echo "::endgroup::"
    continue
  fi
  for c in "${CASES[@]}"; do
    IFS='|' read -r g pkg target name <<<"$c"
    [ "$g" = "$guard" ] || continue
    log="$LOGS/$guard-$name.log"
    if run_test "$pkg" "$target" "$name" "$log"; then
      echo "| $guard | $where | \`$name\` | **still passes (guard survived)** |" >>"$REPORT"
      status=1
    elif grep -q "GUARD-CHECK failed" "$log"; then
      msg=$(grep -m1 -o "GUARD-CHECK failed.*" "$log" | cut -c1-160 | sed 's/|/\\|/g')
      echo "| $guard | $where | \`$name\` | fails: $msg |" >>"$REPORT"
    else
      echo "| $guard | $where | \`$name\` | **fails for another reason** (see logs) |" >>"$REPORT"
      tail -n 30 "$log"
      status=1
    fi
  done
  git checkout -- "$file"
  echo "::endgroup::"
done
cat "$REPORT"
exit $status
