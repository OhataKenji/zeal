# Bazel Build Event Protocol sources

These `.proto` files are unmodified copies from the official Bazel `9.3.1`
release, commit `fee2bf2d1c14bc2096068a4d46f1842472092044`.

Upstream root:
<https://github.com/bazelbuild/bazel/tree/9.3.1/src/main>

The directory name stays fixed across Bazel upgrades. The upstream version and
commit are recorded above; update them in this README when replacing the copied
files. To update, copy the BEP source and its transitive Bazel-owned imports from one
official stable Bazel tag. Keep their upstream paths and license headers, update
the hashes below, then run `cargo check` and the BEP/store tests. Google
well-known protos come from `protoc-bin-vendored` and are not copied here.

SHA-256:

```
f15a652c9804e09fabf3a77919ecaf314750e547daabc7572fd0fad058174316  src/main/java/com/google/devtools/build/lib/buildeventstream/proto/build_event_stream.proto
679135cd318b1e13b920ded15094dc80a3483bb0c34514daeaa098b5fe334375  src/main/java/com/google/devtools/build/lib/packages/metrics/package_load_metrics.proto
9eeff48f357ec2dc07617a0a326fdd89ef6108d6b93bc9eea5592ecb7ff849b3  src/main/protobuf/action_cache.proto
bad4d1fc43be92cd7a70ff3414d4e4c7c3f1843ed57eca75db84b4ae30c7fa33  src/main/protobuf/command_line.proto
ca96ed36af5ba3e8477397b8756fc269880fa0aa34e5e3f6d492332604120959  src/main/protobuf/failure_details.proto
bed0008e69fbc537bc912753bbf23895d9cffd4a1c97adbf41f29082274f523b  src/main/protobuf/invocation_policy.proto
c8e92a8a5be45484f705e4abe4f2a3d4b5c77c99477b5ce2286add87ed64f2cc  src/main/protobuf/option_filters.proto
0068e23223be68f55ddc36d63916efae9b8f64f1cbae5b02bac6059b5b032d4d  src/main/protobuf/strategy_policy.proto
```
