# Record terminal builds

1. Add these settings to your workspace's `.bazelrc`:

   ```bazelrc
   build --bes_backend=grpc://127.0.0.1:4718
   build --bes_upload_mode=wait_for_upload_complete
   build --experimental_build_event_upload_strategy=local
   build --bes_timeout=5s
   ```

2. Keep Zeal open and run `bazel build //...` in your terminal.

These settings also apply to Zeal runs. The local upload strategy keeps timing
available when using a remote cache. [Timing requirements](design/m2-invocation-detail.md#detailed-design).
