use crate::{
    ffmpeg_interface::{transcode_song, FfmpegError, SongMetaData},
    hashing::{hash_file, PreviousSyncDb, SyncRecord},
    log_failure,
    music_library::{
        get_shadow_filename, ArtStrategy, MusicFileType, MusicLibraryError, UpdateType,
    },
    song::Song,
};
use indicatif::ProgressBar;
use std::{fs, io, path::Path};
use UpdateType as U;

/// Synchronises the file. Returns true if the file is updated, false it was not.
pub fn sync_song(
    song: &Song,
    target_library: &Path,
    target_filetype: MusicFileType,
    art_strategy: ArtStrategy,
    previous_sync_db: Option<&PreviousSyncDb>,
    force: bool,
    dry_run: bool,
    pb: Option<&ProgressBar>,
    verbose: bool,
) -> Result<SyncRecord, MusicLibraryError> {
    // TODO:If it exists with a different filetype, give a warning
    let shadow = get_shadow_filename(
        &song.library_relative_path,
        target_library,
        &target_filetype,
    );
    let want_embedded_album_art = match art_strategy {
        ArtStrategy::None => false,
        ArtStrategy::EmbedAll => true,
        ArtStrategy::PreferFile => {
            if song.external_album_art.is_some() {
                false
            } else {
                true
            }
        }
        ArtStrategy::FileOnly => false,
    };
    let desired_bitrate = target_filetype.equivalent_bitrate();
    let status = has_music_file_changed(
        song,
        &shadow,
        previous_sync_db,
        want_embedded_album_art,
        desired_bitrate,
        pb,
        verbose,
    );
    let new_sync_record = SyncRecord::from_song(song);

    // Early exit if unchanged.
    // If force, don't early exit.
    // Instead, overwrite.
    let status = match status {
        U::NoChange => {
            if force {
                U::ForceOverwrite
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
        if matches!(status, U::Copied) {
            std::fs::copy(&song.absolute_path, shadow).expect("could not copy!");
        } else {
            transcode_song(
                &song.absolute_path,
                &shadow,
                target_filetype,
                whether_to_embed_art,
                song.external_album_art.as_deref(),
            )?;
        }
    };

    // The sync record needs to have its new status written to it still!
    Ok(new_sync_record.set_update_type(status))
}

/// Checks if the source music file has been changed since it has been transcoded.
/// Defers to several sub-functions.
pub fn has_music_file_changed(
    song: &Song,
    target: &Path,
    previous_sync_db: Option<&PreviousSyncDb>,
    want_embedded_album_art: bool,
    // Any file that is above this bitrate will just be considered to be copied.
    desired_bitrate: u32,
    pb: Option<&ProgressBar>,
    verbose: bool,
) -> UpdateType {
    use UpdateType as U;

    // We need to perform costly checks here:
    // Ideally, we'd only parse the metadata for the target file if it is truly necessary.

    // Checking the hash of a file takes like 1-2 ms
    let Some(source_hash) = hash_file(&song.absolute_path) else {
        // If you can't determine a hash, there is no way of knowing whether or not the file has
        // changed.
        if verbose {
            log_failure(
                format!(
                    "Could not determine hash of {}. Falling back to comparing metadata.",
                    song
                ),
                pb,
            );
        }
        return compare_files_on_metadata(
            song,
            target,
            want_embedded_album_art,
            desired_bitrate,
            pb,
            verbose,
        );
    };
    // If a previous_sync_db is given, then we can use that to check if the hash is the same.
    if let Some(db) = previous_sync_db {
        return has_music_file_changed_based_on_hash_and_records(
            song,
            source_hash,
            target,
            want_embedded_album_art,
            desired_bitrate,
            db,
            pb,
            verbose,
        );
    };

    // If the file is not there yet, then it is a new file.
    // This is only done after checking the hash existence, because otherwise missing songs
    // (exists in recods, not as file) cannot be detected.
    if !target.exists() {
        return if song.metadata.bitrate_kbps < desired_bitrate {
            U::Copied
        } else {
            U::NewTranscode
        };
    }

    // If you are here, no previous_sync_db is available, or checking for a previous sync didn't work.
    // See if the source file is newer than the destination file.

    let target_is_outdated =
        match has_source_changed_after_target_has_been_created(&song.absolute_path, target) {
            Ok(x) => x,
            Err(e) => {
                if verbose {
                    log_failure(
                        format!(
                            "Could not compare last changed time and \
                            created time of shadow copy of {song}: {e:?}. \
                            Falling back to comparing metadata.",
                        ),
                        pb,
                    );
                }
                return compare_files_on_metadata(
                    song,
                    target,
                    want_embedded_album_art,
                    desired_bitrate,
                    pb,
                    verbose,
                );
            }
        };
    if target_is_outdated {
        return if song.metadata.bitrate_kbps < desired_bitrate {
            U::Copied
        } else {
            U::NewTranscode
        };
    }

    // We cannot just hash the target file, since it will be encoded differently.
    // So, instead we can check if the metadata is the same, and if the album art has
    // not changed.
    compare_files_on_metadata(
        song,
        target,
        want_embedded_album_art,
        desired_bitrate,
        pb,
        verbose,
    )
}

/// Fallback, costly method: Comparing the metadata of the two files.
/// Parsing music file metadata takes like 250 ms.
fn compare_files_on_metadata(
    source: &Song,
    target: &Path,
    want_embedded_album_art: bool,
    desired_bitrate: u32,
    pb: Option<&ProgressBar>,
    verbose: bool,
) -> UpdateType {
    match SongMetaData::parse_file(target) {
        Ok(shadow_metadata) => {
            // The tags should be identical, but the art might be different depending on the
            // desired format.

            if source.metadata.title == shadow_metadata.title
                && want_embedded_album_art == shadow_metadata.has_embedded_album_art
            {
                U::NoChange
            } else {
                // Just copy a file if you'd just incur more encoding loss
                if source.metadata.bitrate_kbps < desired_bitrate {
                    U::Copied
                } else {
                    U::Overwrite
                }
            }
        }
        Err(e) => {
            // If we also can't read the metadata of the existing song, then its pretty clear that we need to overwrite it.
            if verbose {
                log_failure(
                    format!("Could not read metadata from shadow file, so overwriting it: {e}"),
                    pb,
                );
            }
            debug_assert!(target.exists(), "Checking metadata should not fail because the file exists, because file existence is already checked earlier.");
            U::Overwrite
        }
    }
}

fn has_music_file_changed_based_on_hash_and_records(
    song: &Song,
    source_hash: u64,
    target: &Path,
    want_embedded_album_art: bool,
    desired_bitrate: u32,
    db: &PreviousSyncDb,
    pb: Option<&ProgressBar>,
    verbose: bool,
) -> UpdateType {
    if let Some(previous_record) = db.get(&song.library_relative_path) {
        // If the file is in the previous_sync_db, but is not actually present,
        // consider it a missing file.
        if !target.exists() {
            return U::TranscodeMissingTarget;
        }
        // Check if there is a saved hash, and if so, if they are the same.
        if let Some(hash_at_previous_sync) = previous_record.hash {
            if hash_at_previous_sync == source_hash {
                return U::NoChange;
            } else {
                // The hashes are not the same. Hence, the file must have changed.
                return U::Overwrite;
            }
        }
        // Didn't save a hash at previous sync.
        log_failure(
            format!("{song} does not have a hash for previous sync cached, but a record exists."),
            pb,
        );
    };
    // The file is not yet present, and it also does not yet appear in the records.
    // It has to be a new file, so transcode it or copy it.
    if !target.exists() {
        return if song.metadata.bitrate_kbps < desired_bitrate {
            U::Copied
        } else {
            U::NewTranscode
        };
    } else {
        // The file is present, but somehow does not appear in the previous sync db.
        // It could be manually moved into the target library, but then there is no way of
        // knowing if it is still up to date. Hence, it should be checked.
        // It could also be that it could just not be inserted into the records; then too,
        // checking based on metadata is a good idea.
        return compare_files_on_metadata(
            song,
            target,
            want_embedded_album_art,
            desired_bitrate,
            pb,
            verbose,
        );
    }
}

fn has_source_changed_after_target_has_been_created(
    source: &Path,
    target: &Path,
) -> Result<bool, MusicLibraryError> {
    let source_filesystem_md =
        std::fs::metadata(source).map_err(MusicLibraryError::SourceModifiedTime)?;
    let source_last_modified = source_filesystem_md
        .modified()
        .map_err(MusicLibraryError::SourceModifiedTime)?;
    let target_filesystem_md =
        std::fs::metadata(target).map_err(MusicLibraryError::TargetCreatedTime)?;
    let target_created = target_filesystem_md
        .created()
        .map_err(MusicLibraryError::TargetCreatedTime)?;
    Ok(source_last_modified > target_created)
}

#[cfg(test)]
mod tests {
    use crate::{
        ffmpeg_interface::SongMetaData,
        hashing::PreviousSyncDb,
        music_library::{
            get_shadow_filename, library_relative_path, ArtStrategy, ArtworkType, FileType,
            MusicFileType, UpdateType,
        },
        song::Song,
        test_data::TestFile,
    };
    use std::path::PathBuf;

    // TODO: Unit tests for changed artist, album artist, lyrics, album art, etc.

    /// Creates a random folder where a test file can be written to. If it fails, tries again.
    fn create_test_target_library() -> PathBuf {
        const MAX_ATTEMPTS: usize = 3;
        let mut target_library = None;
        for _ in 0..MAX_ATTEMPTS {
            let x: PathBuf = format!(
                "/tmp/syncbops/test_target_lib_{}",
                random_string::generate(24, "abcdefghijklmnopqrstuvwxyz")
            )
            .into();
            match std::fs::create_dir(&x) {
                Ok(_) => {
                    target_library = Some(x);
                    break;
                }
                Err(_) => continue,
            }
        }
        match target_library {
            Some(x) => x,
            None => panic!(
                "Could not create test target library even after {} attempts ",
                MAX_ATTEMPTS
            ),
        }
    }

    /// convenience function to simulate adding a new song.
    /// Used for checking if the resulting som actually has the data that is requested of it.
    fn sync_new_song_test(
        test_file: TestFile,
        target_filetype: MusicFileType,
        external_art: Option<TestFile>,
        art_strategy: ArtStrategy,
    ) -> miette::Result<()> {
        use super::sync_song;

        let target_library = create_test_target_library();
        // let target_filetype = MusicFileType::Mp3CBR { bitrate: 60 };
        let song = Song::new_debug(test_file.path(), external_art.map(|tf| tf.path()))?;
        let target = get_shadow_filename(
            &song.library_relative_path,
            &target_library,
            &target_filetype,
        );
        let updated_record = sync_song(
            &song,
            &target_library,
            target_filetype.clone(),
            art_strategy,
            None,
            false,
            false,
            None,
            true,
        )?;
        let output_metadata = SongMetaData::parse_file(&target)?;

        // The whole point of this program is to save space. The transcoded file should be
        // smaller, while retaining detail. Should not do any transcoding where filesize
        // increases.
        // Bit rate of the target file needs to be smaller than or equal to the original.
        assert!(song.metadata.bitrate_kbps >= output_metadata.bitrate_kbps,
            "source bitrate ({}) should be higher or equivalent to bitrate in generated file ({})- no upscaling!",
            song.metadata.bitrate_kbps, output_metadata.bitrate_kbps);
        if target_filetype.equivalent_bitrate() > song.metadata.bitrate_kbps {
            assert_eq!(updated_record.update_type.unwrap(), UpdateType::Copied)
        } else {
            assert_eq!(
                updated_record.update_type.unwrap(),
                UpdateType::NewTranscode
            );
        }

        // Now, let's see if the art is what we expected it to be.
        match art_strategy {
            ArtStrategy::None => assert!(
                !output_metadata.has_embedded_album_art,
                "Art strategy is to have no artwork yet there is embedded artwork."
            ),
            ArtStrategy::EmbedAll => {
                // Can't have any artwork if there never was any.
                if song.has_artwork() != ArtworkType::None {
                    assert!(
                        output_metadata.has_embedded_album_art,
                        "ArtStrategy::EmbedAll, yet no embedded artwork.."
                    )
                }
            }
            ArtStrategy::PreferFile => {
                if song.external_album_art.is_some() {
                    assert!(
                !output_metadata.has_embedded_album_art,
                        "If song has dedicated artwork, it should copy it over with this ArtStrategy, and not embed it."
                    )
                } else if song.metadata.has_embedded_album_art {
                    assert!(output_metadata.has_embedded_album_art , "Even though not preferred option, should still retain artwork that was already embedded")
                }
            }
            ArtStrategy::FileOnly => {
                assert!(
                    !output_metadata.has_embedded_album_art,
                    "If File Only, should not have any embedded artwork."
                )
            }
        }

        Ok(())
    }

    #[test]
    /// Trying to convert to a higher bitrate means the thing should just be copied over.
    fn sync_mp3_to_mp3_with_higher_bitrate() -> miette::Result<()> {
        sync_new_song_test(
            TestFile::Mp3CBRWithoutArt,
            MusicFileType::Mp3CBR { bitrate: 320 },
            None,
            ArtStrategy::None,
        )
    }

    // ART STRATEGY = NONE

    #[test]
    /// Song with embedded album art, no external, art strategy = none.
    fn sync_song_artstrat_none_embedded_art() -> miette::Result<()> {
        sync_new_song_test(
            TestFile::Mp3CBRWithArt,
            MusicFileType::Mp3CBR { bitrate: 60 },
            None,
            ArtStrategy::None,
        )
    }

    #[test]
    /// Song with external art only, art strategy = none
    fn sync_song_artstrat_none_external_art() -> miette::Result<()> {
        sync_new_song_test(
            TestFile::Mp3CBRWithoutArt,
            MusicFileType::Mp3CBR { bitrate: 60 },
            Some(TestFile::Jpg600),
            ArtStrategy::None,
        )
    }

    #[test]
    /// Song with no art at all, art strategy = none
    fn sync_song_artstrat_none_no_art() -> miette::Result<()> {
        sync_new_song_test(
            TestFile::Mp3CBRWithoutArt,
            MusicFileType::Mp3CBR { bitrate: 60 },
            None,
            ArtStrategy::None,
        )
    }

    #[test]
    /// embedded and external art, art strategy = no.
    fn sync_song_artstrat_none_both() -> miette::Result<()> {
        sync_new_song_test(
            TestFile::Mp3CBRWithArt,
            MusicFileType::Mp3CBR { bitrate: 60 },
            Some(TestFile::Jpg600),
            ArtStrategy::None,
        )
    }

    // END ART STRATEGY = NONE
    // ART STRATEGY = EMBED ALL

    #[test]
    /// album art, no external, art strategy = no.
    fn sync_song_artstrat_embed_embedded_art() -> miette::Result<()> {
        sync_new_song_test(
            TestFile::Mp3CBRWithArt,
            MusicFileType::Mp3CBR { bitrate: 60 },
            None,
            ArtStrategy::EmbedAll,
        )
    }

    #[test]
    /// Song with external art only, art strategy = none
    fn sync_song_artstrat_embed_external_art() -> miette::Result<()> {
        sync_new_song_test(
            TestFile::Mp3CBRWithoutArt,
            MusicFileType::Mp3CBR { bitrate: 60 },
            Some(TestFile::Jpg600),
            ArtStrategy::EmbedAll,
        )
    }

    #[test]
    /// Song with no art at all, art strategy = none
    fn sync_song_artstrat_embed_no_art() -> miette::Result<()> {
        sync_new_song_test(
            TestFile::Mp3CBRWithoutArt,
            MusicFileType::Mp3CBR { bitrate: 60 },
            None,
            ArtStrategy::EmbedAll,
        )
    }

    #[test]
    /// Song with both embedded and external art, art strategy = none.
    fn sync_song_artstrat_embed_both() -> miette::Result<()> {
        sync_new_song_test(
            TestFile::Mp3CBRWithArt,
            MusicFileType::Mp3CBR { bitrate: 60 },
            Some(TestFile::Jpg600),
            ArtStrategy::EmbedAll,
        )
    }

    // END ART STRATEGY = EMBED ALL
    // ART STRATEGY = PREFER_FILE

    #[test]
    /// Song with embedded album art, no external, art strategy = prefer file.
    fn sync_song_artstrat_prefer_file_embedded_art() -> miette::Result<()> {
        sync_new_song_test(
            TestFile::Mp3CBRWithArt,
            MusicFileType::Mp3CBR { bitrate: 60 },
            None,
            ArtStrategy::PreferFile,
        )
    }

    #[test]
    /// Song with external art only, art strategy = prefer_file
    fn sync_song_artstrat_prefer_file_external_art() -> miette::Result<()> {
        sync_new_song_test(
            TestFile::Mp3CBRWithoutArt,
            MusicFileType::Mp3CBR { bitrate: 60 },
            Some(TestFile::Jpg600),
            ArtStrategy::PreferFile,
        )
    }

    #[test]
    /// Song with no art at all, art strategy = prefer_file
    fn sync_song_artstrat_prefer_file_no_art() -> miette::Result<()> {
        sync_new_song_test(
            TestFile::Mp3CBRWithoutArt,
            MusicFileType::Mp3CBR { bitrate: 60 },
            None,
            ArtStrategy::PreferFile,
        )
    }

    #[test]
    /// Song with both embedded and external art, art strategy = prefer_file.
    fn sync_song_artstrat_prefer_file_both() -> miette::Result<()> {
        sync_new_song_test(
            TestFile::Mp3CBRWithArt,
            MusicFileType::Mp3CBR { bitrate: 60 },
            Some(TestFile::Jpg600),
            ArtStrategy::PreferFile,
        )
    }

    // END ART STRATEGY = PREFER_FILE
    // ART STRATEGY = FILE_ONLY

    #[test]
    /// Song with embedded album art, no external, art strategy = file_only.
    fn sync_song_artstrat_file_only_embedded_art() -> miette::Result<()> {
        sync_new_song_test(
            TestFile::Mp3CBRWithArt,
            MusicFileType::Mp3CBR { bitrate: 60 },
            None,
            ArtStrategy::FileOnly,
        )
    }

    #[test]
    /// Song with external art only, art strategy = file_only
    fn sync_song_artstrat_file_only_external_art() -> miette::Result<()> {
        sync_new_song_test(
            TestFile::Mp3CBRWithoutArt,
            MusicFileType::Mp3CBR { bitrate: 60 },
            Some(TestFile::Jpg600),
            ArtStrategy::FileOnly,
        )
    }

    #[test]
    /// Song with no art at all, art strategy = file_only
    fn sync_song_artstrat_file_only_no_art() -> miette::Result<()> {
        sync_new_song_test(
            TestFile::Mp3CBRWithoutArt,
            MusicFileType::Mp3CBR { bitrate: 60 },
            None,
            ArtStrategy::FileOnly,
        )
    }

    #[test]
    /// Song with both embedded and external art, art strategy = file_only.
    fn sync_song_artstrat_file_only_both() -> miette::Result<()> {
        sync_new_song_test(
            TestFile::Mp3CBRWithArt,
            MusicFileType::Mp3CBR { bitrate: 60 },
            Some(TestFile::Jpg600),
            ArtStrategy::FileOnly,
        )
    }

    // END ART STRATEGY = FILE_ONLY

    #[test]
    /// Write a song that is present in the database, but is not actually physically in the
    /// directory: it should report it as a missing file and add it again.
    fn sync_missing_song() -> miette::Result<()> {
        let target_library = create_test_target_library();
        let song = Song::new_debug(TestFile::Rotterdam128kbpsMp3.path(), None)?;
        let u = super::sync_song(
            &song,
            &target_library,
            MusicFileType::Mp3VBR { quality: 6 },
            ArtStrategy::PreferFile,
            None,
            false,
            false,
            None,
            true,
        )?;
        assert_eq!(u.update_type.unwrap(), UpdateType::NewTranscode);

        let db = {
            let mut a = PreviousSyncDb::default();
            a.insert(song.library_relative_path.clone(), u);
            a
        };

        // Delete it. The record remains in db.
        std::fs::remove_file(target_library.join(song.library_relative_path.clone())).unwrap();

        let u2 = super::sync_song(
            &song,
            &target_library,
            MusicFileType::Mp3VBR { quality: 6 },
            ArtStrategy::PreferFile,
            Some(&db),
            false,
            false,
            None,
            true,
        )?;
        assert_eq!(u2.update_type.unwrap(), UpdateType::TranscodeMissingTarget);

        Ok(())
    }

    #[test]
    /// Running sync-song on a file that is not changed, with records. Should not update.
    fn sync_existing_song() -> miette::Result<()> {
        let target_library = create_test_target_library();
        let song = Song::new_debug(TestFile::Rotterdam128kbpsMp3.path(), None)?;
        let u = super::sync_song(
            &song,
            &target_library,
            MusicFileType::Mp3VBR { quality: 6 },
            ArtStrategy::PreferFile,
            None,
            false,
            false,
            None,
            true,
        )?;
        assert_eq!(u.update_type.unwrap(), UpdateType::NewTranscode);

        let db = {
            let mut a = PreviousSyncDb::default();
            a.insert(song.library_relative_path.clone(), u);
            a
        };

        let u2 = super::sync_song(
            &song,
            &target_library,
            MusicFileType::Mp3VBR { quality: 6 },
            ArtStrategy::PreferFile,
            Some(&db),
            false,
            false,
            None,
            true,
        )?;
        assert_eq!(u2.update_type.unwrap(), UpdateType::NoChange);

        Ok(())
    }

    #[test]
    /// Running sync-rong on a file that is not changed, without records. Should not update.
    fn sync_existing_song_no_record() -> miette::Result<()> {
        let target_library = create_test_target_library();
        let song = Song::new_debug(TestFile::Rotterdam128kbpsMp3.path(), None)?;
        let u = super::sync_song(
            &song,
            &target_library,
            MusicFileType::Mp3VBR { quality: 6 },
            ArtStrategy::PreferFile,
            None,
            false,
            false,
            None,
            true,
        )?;
        assert_eq!(u.update_type.unwrap(), UpdateType::NewTranscode);

        let u2 = super::sync_song(
            &song,
            &target_library,
            MusicFileType::Mp3VBR { quality: 6 },
            ArtStrategy::PreferFile,
            None,
            false,
            false,
            None,
            true,
        )?;
        assert_eq!(u2.update_type.unwrap(), UpdateType::NoChange);

        Ok(())
    }
}
