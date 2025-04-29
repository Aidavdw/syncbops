use crate::ffmpeg_interface::transcode_song;
use crate::ffmpeg_interface::FfmpegError;
use crate::ffmpeg_interface::SongMetaData;
use crate::hashing::hash_file;
use crate::hashing::PreviousSyncDb;
use crate::hashing::SyncRecord;
use crate::song::Song;
use indicatif::ParallelProgressIterator;
use indicatif::ProgressBar;
use indicatif::ProgressIterator;
use indicatif::ProgressStyle;
use itertools::Itertools;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::fmt::Display;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, Serialize, Deserialize, Clone, Copy)]
pub enum UpdateType {
    /// The file did not need to be changed, as it is up-to-date
    Unchanged,
    /// The file is completely new, so everything had to be done from scratch
    New,
    /// Updated because it was modified more recently than the shadow copy
    Overwritten,
    /// Actually unchanged, but forced into being overwritten.
    ForcefullyOverwritten,
    /// The song is present in the SyncDB (It has been synced before),
    /// but the target file is no longer there
    MissingTarget,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ArtworkType {
    Embedded,
    External,
    None,
}

#[derive(Clone, Debug, clap::Subcommand)]
pub enum MusicFileType {
    /// Constant bitrate MP3. Very widely supported, not very good.
    Mp3CBR {
        /// The constant bitrate in kbps
        #[arg(short, long, value_name = "BITRATE", default_value_t = 180)]
        bitrate: u64,
    },
    /// Variable bitrate MP3. A decent bit smaller than MP3 CBR, usually at negligible qualtiy
    /// degredation.
    Mp3VBR {
        /// quality factor. From 0 to 9. Lower is higher quality, but larger filesize. See https://trac.ffmpeg.org/wiki/Encode/MP3
        #[arg(short, long, default_value_t = 3)]
        quality: usize,
    },
    /// Transcode to Opus. Nichely supported, but highest quality audio codec. This might not be supported by your ffmpeg build.
    /// You need to explicitly configure the ffmpeg build with --enable-libopus.
    Opus {
        /// Target bitrate in
        #[arg(short, long, value_name = "BITRATE", default_value_t = 180)]
        bitrate: u64,
        /// Compression algorithm complexity. 0-10. Trades quality for encoding time. higher is best quality. Does not affect filesize
        #[arg(short, long, default_value_t = 3)]
        compression_level: usize,
    },
    /// Transcode to Vorbis. Good support, high quality. Not always supported by ffmpeg
    /// You need to explicitly configure the build with --enable-libvorbis.
    Vorbis {
        /// Trades quality for filesize. -1.0 - 10.0 (float!). Higher is better quality.
        #[arg(short, long, default_value_t = 10.0)]
        quality: f64,
    },
    /// Lossless. If a source file is already compressed, it will not be re-encoded.
    Flac {
        /// Compression factor. Trades compilation time for filesize. Higher is smaller file. From 0 to 12.
        #[arg(short, long, default_value_t = 10)]
        quality: u64,
    },
}

// impl MusicFileType {
//     pub fn get_extension(path: &Path) -> Option<MusicFileType> {
//         use MusicFileType as M;
//         if !path.exists() {
//             return None;
//         }
//         if path.is_dir() {
//             return None;
//         };
//         let ext = path.extension()?.to_ascii_lowercase();
//
//         Some(match ext.as_os_str().to_str()? {
//             "mp3" => M::Mp3 {
//                 constant_bitrate: 0,
//                 vbr: false,
//                 quality: 0,
//             },
//             _ => return None,
//         })
//     }
// }

impl Display for MusicFileType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                MusicFileType::Mp3VBR { .. } => "mp3",
                MusicFileType::Mp3CBR { .. } => "mp3",
                MusicFileType::Opus { .. } => "opus",
                MusicFileType::Vorbis { .. } => "ogg",
                MusicFileType::Flac { .. } => "flac",
            }
        )
    }
}

pub enum ImageType {
    Png,
    Jpg,
}

#[derive(PartialEq, Eq)]
pub enum FileType {
    Folder,
    Music,
    Art,
}

/// Returns None if the file does not exist or is not identifiable.
fn identify_file_type(path: &Path) -> Option<FileType> {
    if !path.exists() {
        return None;
    }
    if path.is_dir() {
        return Some(FileType::Folder);
    };
    let ext = path.extension()?.to_ascii_lowercase();

    Some(match ext.as_os_str().to_str()? {
        "mp3" => FileType::Music,
        "m4a" => FileType::Music,
        "ogg" => FileType::Music,
        "flac" => FileType::Music,
        "png" => FileType::Art,
        "jpg" => FileType::Art,
        "jpeg" => FileType::Art,
        _ => return None,
    })
}

/// Checks if the file meets the criteria to be considered dedicated album art: is it named
/// cover.jpg or something?
fn is_image_file_album_art(path: &Path) -> bool {
    // if it's something like "cover" or "folder"
    const ALLOWED_STEMS: [&str; 6] = [
        "cover",
        "folder",
        "album",
        "cover_image",
        "cover_art",
        "front",
    ];
    let stem_is_allowed: bool = ALLOWED_STEMS.iter().any(|x| {
        path.file_stem()
            .is_some_and(|s| s.to_ascii_lowercase() == *x)
    });

    let has_right_extension =
        identify_file_type(path).is_some_and(|file_type| matches!(file_type, FileType::Art));

    stem_is_allowed && has_right_extension
}

//
fn identify_entries_in_folder(
    path: &Path,
) -> Result<impl Iterator<Item = (PathBuf, FileType)> + '_, MusicLibraryError> {
    if !path.is_dir() {
        return Err(MusicLibraryError::NotADirectory {
            path: path.to_path_buf(),
        });
    }
    let dir = fs::read_dir(path).map_err(|_| MusicLibraryError::CouldNotProcessDir {
        path: path.to_path_buf(),
    })?;
    let files_and_types = dir
        .into_iter()
        // Remove un-parseable items, and identify their types.
        .filter_map(|entry| {
            let Ok(valid_file_or_folder) = entry else {
                eprintln!("Unable to process an entry in folder {}", path.display(),);
                return None;
            };
            let path = valid_file_or_folder.path();
            let Some(filetype) = identify_file_type(&path) else {
                eprintln!("Could not identify file {}", path.display(),);
                return None;
            };
            Some((path, filetype))
        });
    Ok(files_and_types)
}

pub fn find_songs_in_source_library(
    source_library_path: &Path,
) -> Result<Vec<Song>, MusicLibraryError> {
    recursively_find_songs_in_directory_and_subdirectories(source_library_path, source_library_path)
}

fn recursively_find_songs_in_directory_and_subdirectories(
    relative_root: &Path,
    path: &Path,
) -> Result<Vec<Song>, MusicLibraryError> {
    // Iterate through the folders. If there is a music file here, then this should be an
    // album.
    // if there are no music files here, then go some level deeper, because there might be
    // music in a sub-folder.
    // If there are no music files, and there are also no sub-folders, then ignore this foledr
    // and continue with the next one.

    let files_and_folders_in_dir = identify_entries_in_folder(path)?.collect_vec();

    // See if this folder contains album art
    let folder_art = files_and_folders_in_dir
        .iter()
        .filter(|(_, filetype)| *filetype == FileType::Art)
        .map(|(path, _)| path)
        .filter(|image_path| is_image_file_album_art(image_path))
        .next()
        .cloned();

    // If there are sub-directories, recurse into them.
    let songs_in_sub_directories = files_and_folders_in_dir
        .iter()
        .filter(|(_, filetype)| *filetype == FileType::Folder)
        .filter_map(move |(path, _)| {
            recursively_find_songs_in_directory_and_subdirectories(relative_root, &path).ok()
        })
        .flatten();

    // Handle all song files in this dir
    let songs = files_and_folders_in_dir
        .iter()
        .filter(|(_, filetype)| *filetype == FileType::Music)
        .filter_map(|(path, _)| {
            // If there is an error with parsing this file to a song, then just ignore it, but do
            // print why it failed.
            Song::new(
                path.clone(),
                relative_root.to_path_buf(),
                folder_art.clone(),
            )
            .map_err(|e| eprintln!("{e}"))
            .ok()
        });
    //
    let songs_in_this_dir_and_subdirs = songs.chain(songs_in_sub_directories).collect_vec();

    Ok(songs_in_this_dir_and_subdirs)
}

/// Checks if the source music file has been changed since it has been transcoded.
pub fn has_music_file_changed(
    song: &Song,
    target: &Path,
    previous_sync_db: Option<&PreviousSyncDb>,
) -> UpdateType {
    use UpdateType as U;
    fn compare_metadata(source: &Song, target: &Path) -> UpdateType {
        match SongMetaData::parse_file(target) {
            Ok(shadow_metadata) => {
                if source.metadata == shadow_metadata {
                    U::Unchanged
                } else {
                    U::Overwritten
                }
            }
            Err(e) => {
                if matches!(e, FfmpegError::FileDoesNotExist { .. }) {
                    // False alarm. Just consider it as new.
                    U::New
                } else {
                    // If we also can't read the metadata of the existing song, then its pretty clear that we need to overwrite it.
                    eprintln!("Could not read metadata from shadow file, so overwriting it: {e}");
                    U::Overwritten
                }
            }
        }
    }

    let Some(source_hash) = hash_file(&song.absolute_path) else {
        // If you can't determine a hash,there is no way of knowing whether or not the file has
        // changed.
        return compare_metadata(song, target);
    };
    // If a previous_sync_db is given, then we can use that to check if the hash is the same.
    if let Some(db) = previous_sync_db {
        if let Some(previous_record) = db.get(&song.library_relative_path) {
            // If the file is in the previous_sync_db, but is not actually present,
            // consider it a missing file.
            if !target.exists() {
                return U::MissingTarget;
            }
            // Check if there is a saved hash, and if so, if they are the same.
            if let Some(hash_at_previous_sync) = previous_record.hash {
                if hash_at_previous_sync == source_hash {
                    return U::Unchanged;
                } else {
                    // The hashes are not the same. Hence, the file must have changed.
                    return U::Overwritten;
                }
            }
            // Didn't save a hash at previous sync.
            eprintln!("{song} does not have a hash for previous sync cached, but a record exists.")
        };
        // This file does not exist in the previous_sync db.
        // If it does exist, but somehow does not appear in the previous sync db, do not early
        // exit- apparently it is overwritten, but weirdly.
        if !target.exists() {
            return U::New;
        }
    };
    // No previous_sync_db is available, or checking for a previous sync didn't work.
    // TODO: Re-instate the small check here to see if the source file is newer than the
    // destination file.

    // We cannot just hash the target file, since it will be encoded differently.
    // So, instead we can check if the metadata is the same, and if the album art has
    // not changed.
    compare_metadata(song, target)
}

pub fn songs_without_album_art(songs: &[Song]) -> Vec<&Song> {
    let pb = ProgressBar::new(songs.len() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("[{elapsed}] [{bar:60.cyan/blue}] {pos}/{len} [ETA: {eta}] {msg}")
            .unwrap()
            .progress_chars("#>-"),
    );
    let yee = songs
        .par_iter()
        .progress_with(pb.clone())
        .with_finish(indicatif::ProgressFinish::AndLeave)
        .filter(|song| {
            pb.set_message(format!("{}", song.absolute_path.display()));
            song.has_artwork() == ArtworkType::None
        })
        .collect::<Vec<_>>();
    yee
}

/// Where to put the synchronised copy
pub fn get_shadow_filename(
    library_relative_path: &Path,
    target_library: &Path,
    // TODO: Change to FileType, so I can re-use the same code for images.
    filetype: &MusicFileType,
) -> PathBuf {
    target_library.join(library_relative_path.with_extension(filetype.to_string()))
}

/// How to handle album art
#[derive(Clone, Copy, PartialEq, clap::ValueEnum)]
pub enum ArtStrategy {
    /// Remove all embedded album art, and don't copy album art files.
    None,
    /// Embeds album art in all files. Carries over album art that was already in source files, and embeds external album art. Might take up more space!
    EmbedAll,
    /// If there is both embedded and external, prefer external. E.g. If there is a cover.jpg (or similar), use that. If there is no dedicated file, use embedded art.
    PreferFile,
    /// Do not embed any cover art: Discard all existing embedded art, only keep cover.jpg if it exists.
    FileOnly,
}

/// Synchronises the file. Returns true if the file is updated, false it was not.
pub fn sync_song(
    song: &Song,
    target_library: &Path,
    target_filetype: MusicFileType,
    art_strategy: ArtStrategy,
    previous_sync_db: Option<&PreviousSyncDb>,
    force: bool,
    dry_run: bool,
) -> Result<SyncRecord, MusicLibraryError> {
    use UpdateType as U;
    // TODO:If it exists with a different filetype, give a warning
    let shadow = get_shadow_filename(
        &song.library_relative_path,
        target_library,
        &target_filetype,
    );
    let status = has_music_file_changed(song, &shadow, previous_sync_db);
    let new_sync_record = SyncRecord::from_song(song);

    // Early exit if unchanged.
    // If force, don't early exit.
    // Instead, overwrite.
    let status = match status {
        U::Unchanged => {
            if force {
                U::ForcefullyOverwritten
            } else {
                return Ok(new_sync_record.set_update_type(status));
            }
        }
        // Don't touch the other statuses
        _ => status,
    };

    let whether_to_embed_art = match art_strategy {
        ArtStrategy::None => false,
        ArtStrategy::EmbedAll => true,
        ArtStrategy::PreferFile => song.external_album_art.is_none(),
        ArtStrategy::FileOnly => false,
    };

    // Can't change files in place with ffmpeg, so if we need to update then we need to
    // overwrite the file fully.
    // If the source directory does not yet exist, create it. ffmpeg will otherwise throw an error.
    if !dry_run {
        let _ = fs::create_dir_all(shadow.parent().expect("Cannot get parent dir of shadow"));
        // TODO: If the source file is already a lower bitrate, then don't do any transcoding.
        transcode_song(
            &song.absolute_path,
            &shadow,
            target_filetype,
            whether_to_embed_art,
            song.external_album_art.as_deref(),
        )?
    };

    // The sync record needs to have its new status written to it still!
    Ok(new_sync_record.set_update_type(status))
}

/// gets the path relative to the library.
pub fn library_relative_path(full_path: &Path, source_library: &Path) -> PathBuf {
    full_path
        .strip_prefix(source_library)
        .unwrap()
        .to_path_buf()
}

/// Returns the path to the new cover art if the file is copied over.
pub fn copy_dedicated_cover_art_for_song(
    song: &Song,
    source_library: &Path,
    target_library: &Path,
    dry_run: bool,
) -> Result<Option<PathBuf>, MusicLibraryError> {
    let Some(path) = &song.external_album_art else {
        return Ok(None);
    };

    let relative_path = path.strip_prefix(source_library).unwrap();
    let shadow = target_library.join(relative_path);
    // TODO: Return error on something that is not a "file already exists"
    if !fs::exists(&shadow).unwrap() {
        if !dry_run {
            let _ = std::fs::copy(path, &shadow);
        }
        Ok(Some(shadow))
    } else {
        Ok(None)
    }
}

#[derive(thiserror::Error, Debug, miette::Diagnostic)]
pub enum MusicLibraryError {
    #[error("Tried to discover albums in directory '{path}', but that is not a directory.")]
    NotADirectory { path: PathBuf },

    #[error("Could not process reading directory.")]
    CouldNotProcessDir { path: PathBuf },

    #[error("Error in calling ffmpeg")]
    Ffmpeg(#[from] FfmpegError),

    #[error("The given target directory '{target_library}' does not (yet) exist. Please make sure the folder exists, even if it is just an empty folder!")]
    TargetLibraryDoesNotExist { target_library: PathBuf },

    #[error("This output filetype/encoding is not yet supported :(. Feel free to implement it and send a PR <3")]
    OutputCodecNotYetImplemented,

    #[error("Could not hash the file {path}")]
    CantHash { path: PathBuf },
}

#[cfg(test)]
mod tests {
    use super::songs_without_album_art;
    use crate::{
        ffmpeg_interface::SongMetaData,
        hashing::PreviousSyncDb,
        music_library::{
            get_shadow_filename, library_relative_path, ArtStrategy, ArtworkType, MusicFileType,
            UpdateType,
        },
        song::Song,
    };
    use itertools::Itertools;
    use std::path::PathBuf;

    fn with_embedded_album_art() -> PathBuf {
        let mut d = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        d.push("test_data/with_art.mp3");
        assert!(
            d.exists(),
            "test song with embedded album art does not exist."
        );
        d
    }

    fn without_art() -> PathBuf {
        let mut d = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        d.push("test_data/no_art.mp3");
        assert!(d.exists(), "test song without album art does not exist.");
        d
    }

    fn external_art() -> Option<PathBuf> {
        let mut d = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        d.push("test_data/cover_art.jpg");
        assert!(d.exists(), "test song with embedded art does not exist.");
        Some(d)
    }

    /// Shared between all tests for has_music_file_changed
    fn construct_has_music_file_changed(orig_name: &str, modified_name: &str) -> UpdateType {
        use super::has_music_file_changed as f;
        let mut original = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        original.push("test_data/");
        let mut shadow = original.clone();
        original.push(format!("{}.mp3", orig_name));
        assert!(
            original.exists(),
            "{} does not exist, so cannot test",
            original.display()
        );
        let original_song = Song::new_debug(original, None).unwrap();
        shadow.push(format!("{}.mp3", modified_name));
        assert!(
            shadow.exists(),
            "{} does not exist, so cannot test.",
            shadow.display()
        );

        f(&original_song, &shadow, None)
    }

    #[test]
    /// Calling it on the same file.
    fn has_music_file_changed_identical_file() {
        assert_eq!(
            construct_has_music_file_changed("no_art", "no_art"),
            UpdateType::Unchanged,
            "identical file, should say it has not changed"
        )
    }

    /// For tests that should have changed
    fn construct_should_have_changed(mod_suffix: &str) {
        let is_changed =
            construct_has_music_file_changed("no_art", &format!("no_art_changed_{}", mod_suffix));
        assert_eq!(
            is_changed,
            UpdateType::Overwritten,
            "Says file did not change, while it did!"
        )
    }

    #[test]
    fn has_music_file_changed_title() {
        construct_should_have_changed("title")
    }

    // TODO: Unit tests for changed artist, album artist, lyrics, album art, etc.

    /// convenience function to simulate adding a new song.
    /// Used for checking if the resulting som actually has the data that is requested of it.
    fn sync_new_song_test(
        identifier: &str,
        song: Song,
        art_strategy: ArtStrategy,
    ) -> miette::Result<()> {
        use super::sync_song;

        let mut d = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        d.push("test_data/");
        let source_library: PathBuf = d;
        let target_library: PathBuf = format!("/tmp/target_library_{}", identifier).into();
        let _ = std::fs::create_dir(&target_library);
        // Delete anything that's already there, because we wanna test it if it's a new file.
        let target_filetype = MusicFileType::Mp3VBR { quality: 3 };
        let library_relative_path = library_relative_path(&song.absolute_path, &source_library);
        let target = get_shadow_filename(&library_relative_path, &target_library, &target_filetype);
        let _ = std::fs::remove_file(&target);
        assert!(!target.exists());

        let updated_record = sync_song(
            &song,
            &target_library,
            target_filetype,
            art_strategy,
            None,
            false,
            false,
        )?;

        assert!(updated_record.update_type.unwrap() == UpdateType::New);
        // Don't care about external album art; that's not the responsibililty of sync_song
        let output = Song::new_debug(target, None)?;
        match art_strategy {
            ArtStrategy::None => assert!(
                !output.metadata.has_embedded_album_art,
                "Art strategy is to have no artwork yet there is embedded artwork."
            ),
            ArtStrategy::EmbedAll => {
                // Can't have any artwork if there never was any.
                if song.has_artwork() != ArtworkType::None {
                    assert!(
                        output.metadata.has_embedded_album_art,
                        "ArtStrategy::EmbedAll, yet no embedded artwork.."
                    )
                }
            }
            ArtStrategy::PreferFile => {
                if song.external_album_art.is_some() {
                    assert!(
                !output.metadata.has_embedded_album_art,
                        "If song has dedicated artwork, it should copy it over with this ArtStrategy, and not embed it."
                    )
                } else if song.metadata.has_embedded_album_art {
                    assert!(output.metadata.has_embedded_album_art , "Even though not preferred option, should still retain artwork that was already embedded")
                }
            }
            ArtStrategy::FileOnly => {
                assert!(
                    !output.metadata.has_embedded_album_art,
                    "If File Only, should not have any embedded artwork."
                )
            }
        }

        // The it should not be overwritten if
        Ok(())
    }

    // ART STRATEGY = NONE

    #[test]
    /// Song with embedded album art, no external, art strategy = none.
    fn sync_song_artstrat_none_embedded_art() -> miette::Result<()> {
        sync_new_song_test(
            "artstrat_none/embedded",
            Song::new_debug(with_embedded_album_art(), None)?,
            ArtStrategy::None,
        )
    }

    #[test]
    /// Song with external art only, art strategy = none
    fn sync_song_artstrat_none_external_art() -> miette::Result<()> {
        sync_new_song_test(
            "artstrat_none/external",
            Song::new_debug(without_art(), external_art())?,
            ArtStrategy::None,
        )
    }

    #[test]
    /// Song with no art at all, art strategy = none
    fn sync_song_artstrat_none_no_art() -> miette::Result<()> {
        sync_new_song_test(
            "artstrat_none/no-art",
            Song::new_debug(without_art(), None)?,
            ArtStrategy::None,
        )
    }

    #[test]
    /// Song with both embedded and external art, art strategy = none.
    fn sync_song_artstrat_none_both() -> miette::Result<()> {
        sync_new_song_test(
            "artstrat_none/both",
            Song::new_debug(with_embedded_album_art(), external_art())?,
            ArtStrategy::None,
        )
    }

    // END ART STRATEGY = NONE
    // ART STRATEGY = EMBED ALL

    #[test]
    /// Song with embedded album art, no external, art strategy = none.
    fn sync_song_artstrat_embed_embedded_art() -> miette::Result<()> {
        sync_new_song_test(
            "artstrat_embed/embedded",
            Song::new_debug(with_embedded_album_art(), None)?,
            ArtStrategy::EmbedAll,
        )
    }

    #[test]
    /// Song with external art only, art strategy = none
    fn sync_song_artstrat_embed_external_art() -> miette::Result<()> {
        sync_new_song_test(
            "artstrat_embed/external",
            Song::new_debug(without_art(), external_art())?,
            ArtStrategy::EmbedAll,
        )
    }

    #[test]
    /// Song with no art at all, art strategy = none
    fn sync_song_artstrat_embed_no_art() -> miette::Result<()> {
        sync_new_song_test(
            "artstrat_embed/no-art",
            Song::new_debug(without_art(), None)?,
            ArtStrategy::EmbedAll,
        )
    }

    #[test]
    /// Song with both embedded and external art, art strategy = none.
    fn sync_song_artstrat_embed_both() -> miette::Result<()> {
        sync_new_song_test(
            "artstrat_embed/both",
            Song::new_debug(with_embedded_album_art(), external_art())?,
            ArtStrategy::EmbedAll,
        )
    }

    // END ART STRATEGY = EMBED ALL
    // ART STRATEGY = PREFER_FILE

    #[test]
    /// Song with embedded album art, no external, art strategy = prefer file.
    fn sync_song_artstrat_prefer_file_embedded_art() -> miette::Result<()> {
        sync_new_song_test(
            "artstrat_prefer_file/embedded",
            Song::new_debug(with_embedded_album_art(), None)?,
            ArtStrategy::PreferFile,
        )
    }

    #[test]
    /// Song with external art only, art strategy = prefer_file
    fn sync_song_artstrat_prefer_file_external_art() -> miette::Result<()> {
        sync_new_song_test(
            "artstrat_prefer_file/external",
            Song::new_debug(without_art(), external_art())?,
            ArtStrategy::PreferFile,
        )
    }

    #[test]
    /// Song with no art at all, art strategy = prefer_file
    fn sync_song_artstrat_prefer_file_no_art() -> miette::Result<()> {
        sync_new_song_test(
            "artstrat_prefer_file/no-art",
            Song::new_debug(without_art(), None)?,
            ArtStrategy::PreferFile,
        )
    }

    #[test]
    /// Song with both embedded and external art, art strategy = prefer_file.
    fn sync_song_artstrat_prefer_file_both() -> miette::Result<()> {
        sync_new_song_test(
            "artstrat_prefer_file/both",
            Song::new_debug(with_embedded_album_art(), external_art())?,
            ArtStrategy::PreferFile,
        )
    }

    // END ART STRATEGY = PREFER_FILE
    // ART STRATEGY = FILE_ONLY

    #[test]
    /// Song with embedded album art, no external, art strategy = file_only.
    fn sync_song_artstrat_file_only_embedded_art() -> miette::Result<()> {
        sync_new_song_test(
            "artstrat_file_only/embedded",
            Song::new_debug(with_embedded_album_art(), None)?,
            ArtStrategy::FileOnly,
        )
    }

    #[test]
    /// Song with external art only, art strategy = file_only
    fn sync_song_artstrat_file_only_external_art() -> miette::Result<()> {
        sync_new_song_test(
            "artstrat_file_only/external",
            Song::new_debug(without_art(), external_art())?,
            ArtStrategy::FileOnly,
        )
    }

    #[test]
    /// Song with no art at all, art strategy = file_only
    fn sync_song_artstrat_file_only_no_art() -> miette::Result<()> {
        sync_new_song_test(
            "artstrat_file_only/no-art",
            Song::new_debug(without_art(), None)?,
            ArtStrategy::FileOnly,
        )
    }

    #[test]
    /// Song with both embedded and external art, art strategy = file_only.
    fn sync_song_artstrat_file_only_both() -> miette::Result<()> {
        sync_new_song_test(
            "artstrat_file_only/both",
            Song::new_debug(with_embedded_album_art(), external_art())?,
            ArtStrategy::FileOnly,
        )
    }

    // END ART STRATEGY = FILE_ONLY

    fn mock_song(embedded_art: bool, external_album_art: bool) -> Song {
        let path = if embedded_art {
            with_embedded_album_art()
        } else {
            without_art()
        };
        let external_album_art = if external_album_art {
            external_art()
        } else {
            None
        };
        Song::new_debug(path, external_album_art).unwrap()
    }

    #[test]
    fn songs_without_album_art_test() -> miette::Result<()> {
        assert!(songs_without_album_art(&[mock_song(false, true)]).is_empty());
        assert!(songs_without_album_art(&[mock_song(true, false)]).is_empty());
        assert!(!songs_without_album_art(&[mock_song(false, false)]).is_empty());

        // One album only, only embedded
        assert_eq!(
            songs_without_album_art(&[mock_song(true, false), mock_song(false, false),])
                .iter()
                .exactly_one()
                .unwrap(),
            &&mock_song(false, false)
        );

        // More only embbeded
        assert_eq!(
            songs_without_album_art(&[
                mock_song(true, false),
                mock_song(true, false),
                mock_song(false, false),
                mock_song(false, false),
            ])
            .len(),
            2
        );

        // one embedded, the other dedicated.
        assert_eq!(
            songs_without_album_art(&[
                mock_song(true, false),
                mock_song(false, false),
                mock_song(false, true),
            ])
            .iter()
            .exactly_one()
            .unwrap(),
            &&mock_song(false, false),
        );

        Ok(())
    }
}
