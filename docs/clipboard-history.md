# Clipboard History Compatibility

On macOS, Clipaste publishes a single pasteboard item containing the original
representations plus a PNG, legacy PNGf, file URL, and plain-text file path.
`x.nspasteboard.ModifiedType` identifies the change count that was enriched.
Maccy 2.6.1 understands this marker and replaces an already-recorded original
instead of inserting a second history item. No ignored-type setting or timing
delay is needed. Other history managers must support this modification protocol
to merge an original they already recorded.

Clipaste checks the pasteboard change count after conversion and disk I/O and
abandons an obsolete conversion when another copy has arrived. The next poll
still processes that copy; there is no time-based suppression of new copies.
Empty or declared-but-unavailable images remain pending: a producer can supply
data after clearing the pasteboard without incrementing its change count again.
Browser images are accepted regardless of the number of advertised formats.
Unavailable optional representations do not block a readable PNG.
Multi-item and concealed/transient/auto-generated image copies are not normalized.
Only one image-normalizing daemon should be enabled. A legacy
`clipboard-normalizer` LaunchAgent duplicates Clipaste's responsibility and
should be disabled when Clipaste is in use.

## Cache Lifetime

PNG files use `shot-sha256-<digest>.png` names under the existing Clipaste cache
directory. Identical PNG bytes reuse the same path, including after a daemon
restart. Files are published atomically and verified before reuse. On Unix the
directory is private (`0700`) and image files are owner-only (`0600`).
Unix cache filesystems must support hard links (such as APFS). Windows uses a
same-volume, non-overwriting move, including on FAT/exFAT. Publication failures
are reported instead of exposing partially written files.

Published images, including legacy timestamp filenames, no longer expire after
one hour. Clipboard managers and pasted terminal paths can retain references
indefinitely, and Clipaste cannot determine when every consumer has released
them. Disk usage grows with unique images. Explicitly deleting cache files
reclaims space but invalidates any saved file-path references; embedded image
bytes in a clipboard manager may remain available. Clipaste does not upload
the cache, change its loopback HTTP bind, or extend sharing to new hosts.

Existing duplicate history entries and missing legacy files need a separate,
backed-up repair. Stop the history manager before modifying its database,
restore missing owned paths from embedded PNG data, and merge only verified
equivalent entries while preserving copy times, counts, and pins. Do not treat
orphan content records as active history, and do not merge unrelated file
attachments merely because they contain identical image bytes.

## Verification

`cargo test` includes isolated named-pasteboard checks for modification markers,
original representation preservation, concurrent copies, rapid distinct copies,
and multi-item/private exclusions, plus isolated cache identity, permissions,
corruption, publication, and lifetime tests. Tests do not alter the general
clipboard or real home directory.

For a real Maccy check, let Maccy record a test PNG while Clipaste is stopped,
then start Clipaste and confirm only one active history item contains those PNG
bytes. Repeat copying the same PNG, restore it through Maccy, and confirm its
cached file and local HTTP image response still contain the original bytes.
