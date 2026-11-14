# Separate application audio (#68)

Issue: https://github.com/rusttalkcrew/RustTalk/issues/68

The share dialog defaults to **Mix into one**. **Send separately** captures and
encodes up to eight applications independently at 64 kbps each. System audio
remains a single stream. Each application has a receiver volume, mute and meter.
Generic labels are the default; app names are opt-in and sent only to the voice
channel. No process IDs, paths, window titles or icons cross the network.

Protocol v17 adds bounded stream registrations over TLS. UDP audio version 4
adds a little-endian u32 stream ID after the counter (32-byte header). ID zero
retains microphone/control/mixed-audio semantics. Separate streams use nonzero
IDs, never reused on a connection. The entire header is authenticated. Encoders
have separate sequence/timestamp state but share the existing session cipher
and its atomic encryption counter. No new cryptographic construction is used.

The server validates registrations atomically, rejects stale/duplicate IDs and
invalid labels, and limits active streams to eight. UDP requires an active
sharing flag and matching registration; unknown/removed IDs are discarded.
The existing session replay window spans all streams. A per-user aggregate cap
of 600 audio packets/second bounds relay traffic to at most 900 KB/second per
recipient at the 1500-byte packet limit. Registration changes are rate-limited;
removals and stopping remain possible when the limit is reached.

New encoders wait for registration acknowledgement. Receivers drop UDP that
arrives before its metadata, without allocating decoder state. Metadata is
validated again on receipt. Receiver registrations and decoder counts are
bounded. Channel joins receive current registrations; leaving, stopping and
source exit remove registrations. Playback discards removed decoders.

Each encoder has an independent AEC. The worker reads the render reference once
per tick and fans the same reference out to every AEC. Mixed capture retains its
existing single-AEC path. Mixer keys use a u128 containing the full u64 speaker
ID, u32 stream ID and shared/voice bit. App volume/mute state is session-only;
existing microphone and mixed-audio volume preferences retain their meaning.

Applying a new selection restarts capture with fresh stream IDs, so application
volume settings reset on Apply. Live source exit removes just the ended app.
Hardware CPU cost and echo behavior at eight streams need Windows/Linux manual
validation; synthetic encoder tests cannot establish capture-device behavior.

## Abuse cases and verification

A peer could forge IDs, replay valid packets, register excessive streams, send
invalid names, retain removed IDs, or flood audio. Authentication binds IDs,
replay protection spans streams, server authorization precedes relay, and
registration/packet/count limits bound work. Labels are at most 64 UTF-8 bytes
and reject controls and bidi formatting. Unknown metadata cannot create a new
user. No app metadata is persisted or logged.

Tests cover authenticated IDs, truncated/oversized headers, invalid flags,
registration limits/duplicates/labels, stale IDs, aggregate rate limits,
independent encoder sequences with unique shared-cipher counters, channel-only
metadata and UDP relay/replay/removal through a real server. Existing protocol
mismatch tests verify old clients are refused.

Before readiness: independent protocol/security review, locked crate tests,
clippy, and manual Windows/Linux audio checks where hardware is available.
