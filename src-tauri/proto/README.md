# BES wire projection

The small schemas retain the original package, RPC paths, field numbers and wire types.
Unneeded fields/events are skipped, never retained. Not an alternate protocol.

Sources (Apache-2.0):

- https://github.com/googleapis/googleapis/blob/master/google/devtools/build/v1/publish_build_event.proto
- https://github.com/googleapis/googleapis/blob/master/google/devtools/build/v1/build_events.proto
- https://github.com/bazelbuild/bazel/blob/8.7.0/src/main/java/com/google/devtools/build/lib/buildeventstream/proto/build_event_stream.proto

`optional` on legacy milliseconds preserves absent versus epoch zero. Modern Timestamp
is preferred when present. TOOL=3; FINISHED=1; Any type is build_event_stream.BuildEvent.
