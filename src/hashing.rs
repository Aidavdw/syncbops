use crate::{log_failure, music_library::UpdateType, song::Song, PREVIOUS_SYNC_DB_FILENAME};
use indicatif::ProgressBar;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::File,
    io::BufReader,
    path::{Path, PathBuf},
    time::SystemTime,
};

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
    pub fn from_song(song: &Song) -> SyncRecord {
        SyncRecord {
            library_relative_path: song.library_relative_path.clone(),
            update_type: None,
            date: SystemTime::now(),
            hash: hash_file(&song.absolute_path),
        }
    }

    pub fn set_update_type(self, update_type: UpdateType) -> SyncRecord {
        let mut proxy = self;
        proxy.update_type = Some(update_type);
        proxy
    }
}

/// Knowledge on how the previous sync was done.
/// Map where the keys are source-library relative paths.
pub type PreviousSyncDb = HashMap<PathBuf, SyncRecord>;

/// Tries to read the previous sync db into one of the possible locations.
pub fn read_records_of_previous_sync(target_library: &Path) -> Option<PreviousSyncDb> {
    let file_candidates = potential_locations_for_records_of_previous_syncs(target_library);
    for file in file_candidates {
        match read_records_from_file(&file) {
            Some(x) => {
                println!("Read records from {}", file.display());
                return Some(x);
            }
            None => {
                continue;
            }
        }
    }
    println!("Could not find any records of previous syncs.");
    None
}

/// Attempts to read records of a previous sync fron the given path.
fn read_records_from_file(path: &Path) -> Option<PreviousSyncDb> {
    // Deserialise it. If it fails, it's better to just handle it like a new sync; assume an empty PreviousSyncDb.
    let file = match File::open(path) {
        Ok(x) => x,
        Err(e) => {
            eprintln!(
                "Cannot open {} to read records from: {}.",
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

/// Previous sync records should normally be saved in the target library, but they can be
/// missing or somewhere else. This generates potential locations it could be found at.
fn potential_locations_for_records_of_previous_syncs(target_library: &Path) -> Vec<PathBuf> {
    let mut potential_dirs = Vec::new();

    // File in target library itself
    potential_dirs.push(target_library.join(PREVIOUS_SYNC_DB_FILENAME));

    // File in current working directory
    if let Ok(pwd) = std::env::current_dir() {
        potential_dirs.push(pwd.join(PREVIOUS_SYNC_DB_FILENAME))
    };

    // File in user's home directory
    if let Some(pwd) = dirs::home_dir() {
        potential_dirs.push(pwd.join(PREVIOUS_SYNC_DB_FILENAME))
    };
    potential_dirs
}

/// Tries to write the previous sync db into one of the possible locations, so that they can be
/// checked against in the next sync.
pub fn write_records_of_current_sync(previous_sync_db: &PreviousSyncDb, target_library: &Path) {
    let file_candidates = potential_locations_for_records_of_previous_syncs(target_library);
    let mut success = false;
    for file in file_candidates {
        success = write_sync_records_to_file(previous_sync_db, &file);
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

/// Attempt to write to this specific file
fn write_sync_records_to_file(previous_sync_db: &PreviousSyncDb, path: &Path) -> bool {
    // Open file for writing
    let file = match File::create(path) {
        Ok(x) => x,
        Err(e) => {
            eprintln!(
                "Cannot open {} for writing records: {}. No previous sync data will be saved. This probably means your next sync will unnecessarily redo a lot of things :(", path.display(), e
            );
            return false;
        }
    };
    let written = serde_json::to_writer(file, previous_sync_db);
    match written {
        Ok(_) => true,
        Err(e) => {
            eprintln!("Could not write records to {}: {}", path.display(), e);
            false
        }
    }
}

/// Adds a new sync result to the currently opened database of sync results, so that it can be
/// written to disk later.
pub fn register_record_to_previous_sync_db(
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
    if update_type == UpdateType::NoChange {
        return;
    }
    // Returned value is old value, don't need it anymore.
    let _ = previous_sync_db.insert(sync_record.library_relative_path.clone(), sync_record);
}

/// Simple hash to see if a file has changed. Non-cryptographic!
pub fn hash_file(path: &Path) -> Option<u64> {
    let mut file = std::fs::File::open(path).ok()?;
    let hash = rapidhash::rapidhash_file(&mut file).ok()?;
    Some(hash)
}
