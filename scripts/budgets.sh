#!/usr/bin/env bash
# Measure Hoarder's performance budgets (PLAN.md section 12) and compare two
# measurements. CI's `budgets` job measures the change and its base on the
# same runner, so a slow runner slows both alike, and fails on a regression
# of more than 20% (docs/testing.md, "The budgets job").
#
#   scripts/budgets.sh measure WORKSPACE OUT.tsv
#       Run the budget tests in WORKSPACE (release build, HOARDER_BUDGET_SCALE
#       as set) and write one "metric<TAB>value<TAB>unit" line per budget,
#       plus the stripped release binary's size. Exits non-zero if a budget
#       test fails, after writing what it measured.
#   scripts/budgets.sh compare BASE.tsv HEAD.tsv
#       Print a Markdown table of both and exit 1 if any metric in HEAD is
#       more than 20% (HOARDER_REGRESSION_PCT) above BASE. A metric BASE
#       lacks (a new budget) is reported, not compared.
set -euo pipefail

measure() {
	local workspace=$1 out=$2 log status=0
	log=$(mktemp)
	# The budget tests print "budget: WHAT: measured X ms, budget ..." and
	# fail on their own when over the (scaled) PLAN budget.
	(
		cd "$workspace"
		cargo test --release --locked -p hoarder \
			--test e2e_shot_clipboard --test e2e_overlay --test e2e_picker \
			-- 150_ms budget --nocapture --test-threads=1
	) 2>&1 | tee "$log" || status=$?
	sed -n 's/.*budget: \(.*\): measured \([0-9.]*\) ms.*/\1\t\2\tms/p' "$log" >"$out"
	rm -f "$log"

	(cd "$workspace" && cargo build --release --locked -p hoarder)
	local bin stripped
	bin=$(cd "$workspace" && cargo metadata --format-version 1 --no-deps |
		sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')/release/hoarder
	stripped=$(mktemp)
	strip -o "$stripped" "$bin"
	printf 'stripped release binary\t%s\tbytes\n' "$(stat -c %s "$stripped")" >>"$out"
	rm -f "$stripped"
	return "$status"
}

compare() {
	local base=$1 head=$2
	awk -F'\t' -v pct="${HOARDER_REGRESSION_PCT:-20}" '
		NR == FNR { base[$1] = $2; next }
		FNR == 1 {
			print "| Budget | Base | This change | Change |"
			print "|---|---|---|---|"
		}
		{
			if (!($1 in base) || base[$1] == 0) {
				printf "| %s | (new) | %s %s | |\n", $1, $2, $3
				next
			}
			change = ($2 / base[$1] - 1) * 100
			mark = change > pct ? " **regression**" : ""
			if (change > pct) failed = 1
			printf "| %s | %s %s | %s %s | %+.1f%%%s |\n", $1, base[$1], $3, $2, $3, change, mark
		}
		END {
			if (failed) {
				printf "\nA budget regressed by more than %s%%.\n", pct
				exit 1
			}
		}
	' "$base" "$head"
}

case "${1:-}" in
measure) measure "$2" "$3" ;;
compare) compare "$2" "$3" ;;
*)
	echo "usage: $0 measure WORKSPACE OUT.tsv | compare BASE.tsv HEAD.tsv" >&2
	exit 2
	;;
esac
