use core::hash;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::SystemTime,
};

use crate::{
    music_library::{library_relative_path, UpdateType},
    song::Song,
};

/// Data about how a file is at a certain point in time. By comparing SyncRecords, you can see
/// if a file is out of date.
pub struct SyncRecord {
    pub library_relative_path: PathBuf,
    /// None for any SyncRecords in the source library.
    pub update_type: Option<UpdateType>,
    pub date: SystemTime,
    pub hash: Option<u64>,
}

impl SyncRecord {
    pub fn from_file_path(file: &Path, source_library: &Path) -> SyncRecord {
        SyncRecord {
            library_relative_path: library_relative_path(file, source_library),
            update_type: None,
            date: SystemTime::now(),
            hash: hash_file(file),
        }
    }
}
pub fn compare_records(source: &SyncRecord, previous: &SyncRecord) -> UpdateType {
    use UpdateType as U;
    if let Some(new_hash) = source.hash {
        if let Some(other_hash) = previous.hash {
            // Both have hashes, so we can compare hashes.
            let is_hash_different = new_hash != other_hash;
            if is_hash_different {
                return U::Overwritten;
            } else {
                return U::Unchanged;
            }
        }
        // New one can be hashed, but the existing one does not have a hash.
        // It does have an entry though, so it counts as overwriting, not as adding a new
        // one.
        return U::Overwritten;
    }
    // New one cannot be hashed. We don't know about the new file, so can only overwrite
    return U::Overwritten;
}

pub type PreviousSyncDb = HashMap<PathBuf, SyncRecord>;

pub fn load_previous_sync_db(target_library: &Path) -> PreviousSyncDb {
    todo!()
}

pub fn save_to_previous_sync_db(previous_sync_db: &mut PreviousSyncDb, sync_record: SyncRecord) {
    let add_new = previous_sync_db.insert(sync_record.library_relative_path.clone(), sync_record);
    if add_new.is_none() {
        eprintln!("Could not register song to previous sync db. Maybe it already exists?");
    };
}

pub fn hash_file(path: &Path) -> Option<u64> {
    let mut file = std::fs::File::open(path).ok()?;
    let hash = rapidhash::rapidhash_file(&mut file).ok()?;
    Some(hash)
}
