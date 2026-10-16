#!/bin/sh
# Regenerates the M2 run fixtures: <case>/{bep.json,execlog,profile.gz}.
# Needs bazelisk on PATH as `bazel`, jq, and strings. Not run in CI; the
# outputs are committed. Run from anywhere: sh src-tauri/tests/fixtures/run/gen.sh
#
# Everything is built under /tmp so that no personal path ends up in the
# outputs. The script fails if one does.
set -eu

out=$(cd "$(dirname "$0")" && pwd)
ws=/tmp/zeal-fixture

# 1. Write the workspace. Nothing Bazel-related lives in this repo.
rm -rf "$ws"
mkdir -p "$ws/pkg" "$ws/fail" "$ws/dep" "$ws/syntax" "$ws/raw"
cd "$ws"

echo 8.7.0 > .bazelversion
: > MODULE.bazel
cat > .bazelrc <<'RC'
startup --output_user_root=/tmp/zeal-fixture-root
build --incompatible_strict_action_env
RC

cat > pkg/BUILD <<'BUILD'
genrule(
    name = "a",
    outs = ["a.txt"],
    cmd = "sleep 1; echo a > $@",
)

genrule(
    name = "b",
    srcs = [":a"],
    outs = ["b.txt"],
    cmd = "sleep 1; cat $< > $@; echo b >> $@",
)

genrule(
    name = "c",
    srcs = [":a"],
    outs = ["c.txt"],
    cmd = "echo c > $@",
)

sh_binary(
    name = "tool",
    srcs = ["tool.sh"],
)

genrule(
    name = "d",
    srcs = [":b", ":c"],
    outs = ["d.txt"],
    tools = [":tool"],
    cmd = "$(location :tool) $(SRCS) > $@",
)

sh_test(
    name = "t",
    srcs = ["t.sh"],
    data = [":d"],
)
BUILD
printf '#!/bin/sh\ncat "$@"\n' > pkg/tool.sh
printf '#!/bin/sh\nexit 0\n' > pkg/t.sh
chmod +x pkg/tool.sh pkg/t.sh

cat > fail/BUILD <<'BUILD'
genrule(
    name = "x",
    outs = ["x.txt"],
    cmd = "exit 3",
)
BUILD

cat > dep/BUILD <<'BUILD'
genrule(
    name = "x",
    srcs = ["//missing:y"],
    outs = ["x.txt"],
    cmd = "touch $@",
)
BUILD

echo 'genrule(name = "x"' > syntax/BUILD

# 2. Run each case with the observation flags M2 adds.
obs="--build_event_json_file=$ws/raw/bep.json --execution_log_binary_file=$ws/raw/execlog --profile=$ws/raw/profile.gz --experimental_profile_include_primary_output --noslim_profile --tool_tag=zeal"

for c in ok disk_cache_hit action_failed analysis_failed loading_failed; do
  rm -f raw/*
  case $c in
    ok) bazel clean && bazel test $obs //pkg/... ;;
    disk_cache_hit) bazel clean && bazel build --disk_cache=/tmp/zeal-fixture-root/dc //pkg:d && bazel clean && bazel build $obs --disk_cache=/tmp/zeal-fixture-root/dc //pkg:d ;;
    action_failed) bazel build $obs //fail:x ;;
    analysis_failed) bazel build $obs //dep:x ;;
    loading_failed) bazel build $obs //syntax:x ;;
  esac || true

  rm -rf "${out:?}/$c"
  mkdir "$out/$c"
  # Keep only the BEP events M2 reads; the rest carry HOME, USER, hostname and env.
  jq -c 'select(.id | has("pattern") or has("targetConfigured") or has("targetCompleted") or has("testSummary") or has("buildMetrics"))' raw/bep.json > "$out/$c/bep.json"
  mv raw/profile.gz "$out/$c/"
  # loading_failed writes no exec log at all.
  if [ -f raw/execlog ]; then mv raw/execlog "$out/$c/"; fi
done
bazel shutdown

# 3. A BEP cut in the middle of a line, as if bazel died while writing.
rm -rf "$out/bep_truncated"
mkdir "$out/bep_truncated"
head -c $(($(wc -c < "$out/ok/bep.json") / 2)) "$out/ok/bep.json" > "$out/bep_truncated/bep.json"

# 4. Fail if any output mentions this machine or its user.
for f in "$out"/*/*; do
  case $f in
    *.gz) gzip -dc "$f" ;;
    *) cat "$f" ;;
  esac
done | strings -a | grep -E "$HOME|$(id -un)|$(hostname)|/home/|/Users/" && { echo "personal data found" >&2; exit 1; }
echo "privacy check: 0 hits"
wc -c "$out"/*/*
