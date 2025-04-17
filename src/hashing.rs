use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::File,
    io::BufReader,
    path::{Path, PathBuf},
    time::SystemTime,
};

use crate::music_library::{library_relative_path, UpdateType};

/// Data about how a file is at a certain point in time. By comparing SyncRecords, you can see
/// if a file is out of date.
#[derive(Serialize, Deserialize, Debug, Clone)]
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

    pub fn set_update_type(self, update_type: UpdateType) -> SyncRecord {
        let mut proxy = self;
        proxy.update_type = Some(update_type);
        proxy
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
    U::Overwritten
}

pub type PreviousSyncDb = HashMap<PathBuf, SyncRecord>;

/// Tries to read the previous sync db into one of the possible locations
pub fn try_read_records(target_library: &Path) -> PreviousSyncDb {
    let file_candidates = generate_potential_locations_for_database_file(target_library);
    for file in file_candidates {
        match load_previous_sync(&file) {
            Some(x) => {
                println!("Read records from {}", file.display());
                return x;
            }
            None => {
                continue;
            }
        }
    }
    println!("Could not open any records. Assuming there is no previous sync data.");
    PreviousSyncDb::new()
}

fn load_previous_sync(path: &Path) -> Option<PreviousSyncDb> {
    // Deserialise it. If it fails, it's better to just handle it like a new sync; assume an empty PreviousSyncDb.
    let file = match File::open(path) {
        Ok(x) => x,
        Err(e) => {
            eprintln!(
                "Cannot open {} to read records from: {}. Assuming there is no previous sync data.",
                path.display(),
                e
            );
            return None;
        }
    };
    // Open the file in read-only mode with buffer, and parse into PreviousSyncDb
    let reader = BufReader::new(file);
    let previous_sync_db: PreviousSyncDb = match serde_json::from_reader(reader) {
        Ok(x) => x,
        Err(e) => {
            eprintln!(
                "Cannot load previous sync result from {}: {}. Ignoring contents of the file.",
                path.display(),
                e
            );
            return None;
        }
    };
    Some(previous_sync_db)
}

fn generate_potential_locations_for_database_file(target_library: &Path) -> Vec<PathBuf> {
    let mut potential_dirs = Vec::new();

    // File in target library itself
    const PREVIOUS_SYNC_DB_FILENAME: &str = "bopsync.dat";
    potential_dirs.push(target_library.join(PREVIOUS_SYNC_DB_FILENAME));

    // File in current working directory
    if let Ok(pwd) = std::env::current_dir() {
        potential_dirs.push(pwd)
    };

    // File in user's home directory
    if let Some(pwd) = dirs::home_dir() {
        potential_dirs.push(pwd)
    };
    potential_dirs
}

/// Tries to write the previous sync db into one of the possible locations
pub fn try_write_records(previous_sync_db: &PreviousSyncDb, target_library: &Path) {
    let file_candidates = generate_potential_locations_for_database_file(target_library);
    let mut success = false;
    for file in file_candidates {
        success = write_sync_db_to_file(previous_sync_db, &file);
        if success {
            println!("Written records to {}", file.display());
            break;
        }
    }
    if !success {
        println!(
                "Could not find any suitable file to write records to. No previous sync data will be saved. This probably means your next sync will unnecessarily redo a lot of things :(" 
            );
    }
}

fn write_sync_db_to_file(previous_sync_db: &PreviousSyncDb, path: &Path) -> bool {
    // Open file for writing
    let file = match File::create(path) {
        Ok(x) => x,
        Err(e) => {
            eprintln!(
                "Cannot open {} for writing records: {}. No previous sync data will be saved. This probably means your next sync will unnecessarily redo a lot of things :(", path.display(), e
            );
            // TODO: See if we can write to an alt name, to the user's home directory, or to
            // the current directory. and warn the user of it.
            // Just call this same function again, but now with a different target file.
            return false;
        }
    };
    let written = serde_json::to_writer(file, previous_sync_db);
    if written.is_err() {
        eprintln!("Could not write to this file :(");
        return false;
    }
    true
}

pub fn save_record_to_previous_sync_db(
    previous_sync_db: &mut PreviousSyncDb,
    sync_record: SyncRecord,
) {
    let update_type = sync_record
        .update_type
        .expect("update type should be set already.");

    // "file was not modified" is not very useful information
    // knowing when it was last added and when it was last modified is much
    // more useful information.
    // Therefore, only write information if it is actually useful.
    if update_type == UpdateType::Unchanged {
        return;
    }
    // Returned value is old value, don't need it anymore.
    let _ = previous_sync_db.insert(sync_record.library_relative_path.clone(), sync_record);
}

pub fn hash_file(path: &Path) -> Option<u64> {
    let mut file = std::fs::File::open(path).ok()?;
    let hash = rapidhash::rapidhash_file(&mut file).ok()?;
    Some(hash)
}
