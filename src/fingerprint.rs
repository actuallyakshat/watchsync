//! Quick file identity check: size + SHA-256 of the first and last 4 MB.
//! Catches "different rip of the same movie" without hashing 4+ GB.

use anyhow::Result;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

const CHUNK: u64 = 4 * 1024 * 1024;

pub fn fingerprint(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    let mut hasher = Sha256::new();
    hasher.update(size.to_le_bytes());

    let mut buf = vec![0u8; CHUNK.min(size) as usize];
    file.read_exact(&mut buf)?;
    hasher.update(&buf);

    if size > CHUNK {
        let tail = CHUNK.min(size - CHUNK);
        file.seek(SeekFrom::Start(size - tail))?;
        let mut buf = vec![0u8; tail as usize];
        file.read_exact(&mut buf)?;
        hasher.update(&buf);
    }

    let digest = hasher.finalize();
    Ok(hex::encode(&digest.as_slice()[..8]))
}
