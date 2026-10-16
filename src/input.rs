//! Line sources: files, stdin, and `tail -F`-style follow mode.

use anyhow::{Context, Result};
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

const POLL: Duration = Duration::from_millis(200);

/// Stream lines from `paths` (stdin when empty or "-") into a channel.
///
/// Each source runs on its own thread so slow or idle inputs never block
/// others. With `follow`, files are polled for appended data after EOF and
/// re-opened from the start when truncated or rotated.
pub fn spawn_sources(paths: &[PathBuf], follow: bool) -> Result<Receiver<Result<String>>> {
    let (tx, rx) = mpsc::sync_channel::<Result<String>>(8192);
    let stdin_only = paths.is_empty() || paths.iter().all(|p| p.as_os_str() == "-");
    if stdin_only {
        thread::spawn(move || {
            let stdin = io::stdin();
            read_lines(stdin.lock(), |l| tx.send(Ok(l)).is_ok(), &tx);
        });
        return Ok(rx);
    }
    // Open everything up front so a typo fails fast with a clear error.
    let mut files = Vec::new();
    for p in paths {
        let f = File::open(p).with_context(|| format!("cannot open {}", p.display()))?;
        files.push((p.clone(), f));
    }
    for (path, file) in files {
        let tx = tx.clone();
        thread::spawn(move || {
            if follow {
                if let Err(e) = follow_file(&path, file, &tx) {
                    let _ = tx.send(Err(e));
                }
            } else {
                read_lines(BufReader::new(file), |l| tx.send(Ok(l)).is_ok(), &tx);
            }
        });
    }
    Ok(rx)
}

fn read_lines(
    mut r: impl BufRead,
    mut emit: impl FnMut(String) -> bool,
    tx: &mpsc::SyncSender<Result<String>>,
) {
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match r.read_until(b'\n', &mut buf) {
            Ok(0) => return,
            Ok(_) => {
                // Lossy decoding: one bad byte must not abort a multi-GB scan.
                if !emit(String::from_utf8_lossy(&buf).into_owned()) {
                    return;
                }
            }
            Err(e) => {
                let _ = tx.send(Err(e.into()));
                return;
            }
        }
    }
}

fn follow_file(path: &Path, file: File, tx: &mpsc::SyncSender<Result<String>>) -> Result<()> {
    let mut reader = BufReader::new(file);
    let mut pos: u64 = 0;
    let mut partial = Vec::new();
    loop {
        let mut chunk = Vec::new();
        let n = reader.by_ref().take(1 << 20).read_to_end(&mut chunk)?;
        if n > 0 {
            pos += n as u64;
            partial.extend_from_slice(&chunk);
            // Emit complete lines; keep a trailing partial line for later.
            while let Some(i) = partial.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = partial.drain(..=i).collect();
                if tx
                    .send(Ok(String::from_utf8_lossy(&line).into_owned()))
                    .is_err()
                {
                    return Ok(());
                }
            }
            continue;
        }
        thread::sleep(POLL);
        // Detect truncation (copytruncate) or rotation (new inode / shorter file).
        let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(pos);
        if len < pos {
            let f = File::open(path)?;
            reader = BufReader::new(f);
            reader.seek(SeekFrom::Start(0))?;
            pos = 0;
            partial.clear();
        }
    }
}
