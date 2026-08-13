# RTMP sink backend decision

## Decision

Production capability discovery prefers GStreamer's `rtmp2sink` and falls back
to `rtmpsink` when `rtmp2sink` is unavailable. A build exposes RTMP and RTMPS
only when an approved H.264 encoder, AAC encoder, `flvmux`, and one of those
sinks are present. The selected sink is retained in the negotiated pipeline
plan, so runtime construction cannot silently choose a different plugin.

## Evidence

The comparison was performed on GStreamer 1.28.5 with both plugins installed.
Both accept `video/x-flv`, implement `GstURIHandler`, and advertise `rtmp` and
`rtmps`. The newer `rtmp2sink` has the higher factory rank and provides:

- an explicit five-second connection timeout;
- TLS certificate validation controls with validation enabled by default;
- RTMP acknowledgement and byte counters (`in/out-bytes-total` and
  `in/out-bytes-acked`);
- connection pacing, authentication, chunk sizing, and explicit publish-stop
  commands.

The legacy `rtmpsink` is retained as a compatibility fallback. Its observable
statistics are limited to the generic base-sink rendered/dropped counters, so
it cannot provide the same connection-level acknowledgement evidence.

## Qualification matrix

| Scenario | Automated/local coverage | Release-ingest coverage |
| --- | --- | --- |
| Plain RTMP and RTMPS graph construction | Exact sink/mux graph and URI-property tests | Publish to the supported service matrix |
| Preferred and fallback selection | Deterministic selection unit test | Package test with each plugin removed in turn |
| Server rejection / invalid stream key | Typed bus failure and bounded reconnect lifecycle | Verify service-specific rejection text and latency |
| DNS failure / connection interruption | Bounded reconnect-limit test | Disconnect network and restore it during publication |
| High bitrate | Bounded application queues and configured encoder bitrate | Run at each supported service's maximum bitrate |
| Metrics quality | Plugin-property inspection and application submit/drop/reconnect counters | Compare RTMP acknowledgement counters with ingest statistics |
| 24-hour soak | Not suitable for the normal deterministic test suite | Mandatory release qualification with bounded memory and reconnect telemetry |

External-service rows are release qualification gates, not claims made by the
unit suite. Credentials, ingest policy, DNS, and deliberate network disruption
belong in a private test environment and must not be embedded in this
repository. A release candidate fails qualification if memory grows without a
bound, timestamps regress, reconnects exceed policy, RTMPS certificate
validation is disabled, or acknowledged bytes stop advancing while application
submission continues.

## Reconsideration triggers

Re-evaluate the preference if a supported service proves incompatible with
`rtmp2sink`, if its RTMPS validation regresses, or if GStreamer deprecates either
plugin. Compatibility exceptions should be represented as service capability,
not as an unvalidated global fallback.
