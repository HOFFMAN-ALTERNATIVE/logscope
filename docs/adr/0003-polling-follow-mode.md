# 0003. Poll for new data in follow mode

Status: accepted
Date: 2026-10-17

## Context
`-f` must keep reading appended lines and survive logrotate in both `create` and
`copytruncate` modes. The binary ships for Linux, macOS and Windows, and log
directories are sometimes on NFS or bind mounts where change notifications are
unreliable.

## Options
1. inotify/kqueue/ReadDirectoryChanges via the `notify` crate: instant wake-up, but a
   larger dependency tree and silent failure on network filesystems.
2. Poll every 200 ms: read to EOF, sleep, compare file length with the read offset and
   reopen from the start when the file got shorter.

## Decision
Option 2. A 200 ms delay is irrelevant for a human watching logs, and the behaviour is
the same on every platform and filesystem.

## Consequences
- No extra dependencies; rotation handling is about 30 lines in `input.rs`.
- An idle follower wakes 5 times per second per file.
- Rotation where the new file grows past the old offset within one poll interval is
  not detected. Revisit with inode tracking if that shows up in practice.
