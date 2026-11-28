import { test } from "node:test";
import assert from "node:assert/strict";
import {
  formatDuration,
  stepTargetLabel,
  targetForArtifactPath,
} from "../src/lib/critical_path.ts";
import type { Target } from "../src/lib/types.ts";

const TARGETS: Target[] = [
  {
    label: "//rates:rate_server",
    pkg: "rates",
    name: "rate_server",
    ruleClass: "cc_binary",
    location: { file: "BUILD", line: 1 },
  },
  {
    label: "//rates:tariff",
    pkg: "rates",
    name: "tariff",
    ruleClass: "cc_library",
    location: { file: "BUILD", line: 1 },
  },
  {
    label: "//rates/sub:helper",
    pkg: "rates/sub",
    name: "helper",
    ruleClass: "cc_library",
    location: { file: "BUILD", line: 1 },
  },
  {
    label: "//dispatch:quote_contract_test",
    pkg: "dispatch",
    name: "quote_contract_test",
    ruleClass: "cc_test",
    location: { file: "BUILD", line: 1 },
  },
];

test("formatDuration formats microseconds, milliseconds, and seconds compactly", () => {
  assert.equal(formatDuration(35), "35µs");
  assert.equal(formatDuration(1500), "1.5ms");
  assert.equal(formatDuration(26162), "26ms");
  assert.equal(formatDuration(1_468_086), "1.47s");
  assert.equal(formatDuration(12_340_000), "12.3s");
});

test("targetForArtifactPath resolves deepest package, testlogs, and lib<name> while ignoring external", () => {
  assert.equal(
    targetForArtifactPath("bazel-out/k8-fastbuild/bin/rates/rate_server", TARGETS),
    "//rates:rate_server",
  );
  assert.equal(
    targetForArtifactPath("bazel-out/k8-fastbuild/bin/rates/libtariff.a", TARGETS),
    "//rates:tariff",
  );
  assert.equal(
    targetForArtifactPath("bazel-out/k8-fastbuild/bin/rates/sub/helper.o", TARGETS),
    "//rates/sub:helper",
  );
  assert.equal(
    targetForArtifactPath(
      "bazel-out/k8-fastbuild/testlogs/dispatch/quote_contract_test/test.log",
      TARGETS,
    ),
    "//dispatch:quote_contract_test",
  );
  assert.equal(
    targetForArtifactPath("bazel-out/k8-fastbuild/bin/external/protobuf~/liblite.a", TARGETS),
    null,
  );
});

test("stepTargetLabel checks target, out, canonical label, and workspace path in name", () => {
  assert.equal(
    stepTargetLabel(
      { name: "Compiling", durationUs: 10, target: "@@//rates:rate_server" },
      TARGETS,
    ),
    "//rates:rate_server",
  );
  assert.equal(
    stepTargetLabel(
      { name: "Linking", durationUs: 10, out: "bazel-out/k8-fastbuild/bin/rates/libtariff.a" },
      TARGETS,
    ),
    "//rates:tariff",
  );
  assert.equal(
    stepTargetLabel(
      { name: "runfiles for //dispatch:quote_contract_test", durationUs: 10 },
      TARGETS,
    ),
    "//dispatch:quote_contract_test",
  );
  assert.equal(
    stepTargetLabel({ name: "Compiling rates/rate_server.cc", durationUs: 10 }, TARGETS),
    "//rates:rate_server",
  );
  assert.equal(
    stepTargetLabel(
      { name: "Linking external/protobuf~/liblite.a [for tool]", durationUs: 10 },
      TARGETS,
    ),
    null,
  );
});
