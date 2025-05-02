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
use std::{fs, path::Path};

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
) -> Result<SyncRecord, MusicLibraryError> {
    use UpdateType as U;
    // TODO:If it exists with a different filetype, give a warning
    let shadow = get_shadow_filename(
        &song.library_relative_path,
        target_library,
        &target_filetype,
    );
    let desired_bitrate = target_filetype.equivalent_bitrate();
    let status = has_music_file_changed(song, &shadow, previous_sync_db, desired_bitrate, pb);
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
pub fn has_music_file_changed(
    song: &Song,
    target: &Path,
    previous_sync_db: Option<&PreviousSyncDb>,
    // Any file that is above this bitrate will just be considered to be copied.
    desired_bitrate: u32,
    pb: Option<&ProgressBar>,
) -> UpdateType {
    use UpdateType as U;
    fn compare_metadata(
        source: &Song,
        target: &Path,
        desired_bitrate: u32,
        pb: Option<&ProgressBar>,
    ) -> UpdateType {
        match SongMetaData::parse_file(target) {
            Ok(shadow_metadata) => {
                if source.metadata == shadow_metadata {
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
                if matches!(e, FfmpegError::FileDoesNotExist { .. }) {
                    // False alarm. Just consider it as new.
                    // Just copy a file if you'd just incur more encoding loss
                    if source.metadata.bitrate_kbps < desired_bitrate {
                        U::Copied
                    } else {
                        U::NewTranscode
                    }
                } else {
                    // If we also can't read the metadata of the existing song, then its pretty clear that we need to overwrite it.
                    log_failure(
                        format!("Could not read metadata from shadow file, so overwriting it: {e}"),
                        pb,
                    );
                    U::Overwrite
                }
            }
        }
    }

    let Some(source_hash) = hash_file(&song.absolute_path) else {
        // If you can't determine a hash,there is no way of knowing whether or not the file has
        // changed.
        return compare_metadata(song, target, desired_bitrate, pb);
    };
    // If a previous_sync_db is given, then we can use that to check if the hash is the same.
    if let Some(db) = previous_sync_db {
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
                format!(
                    "{song} does not have a hash for previous sync cached, but a record exists."
                ),
                pb,
            );
        };
        // This file does not exist in the previous_sync db.
        // If it does exist, but somehow does not appear in the previous sync db, do not early
        // exit- apparently it is overwritten, but weirdly.
        if !target.exists() {
            return if song.metadata.bitrate_kbps < desired_bitrate {
                U::Copied
            } else {
                U::NewTranscode
            };
        }
    };
    // No previous_sync_db is available, or checking for a previous sync didn't work.
    // TODO: Re-instate the small check here to see if the source file is newer than the
    // destination file.

    // We cannot just hash the target file, since it will be encoded differently.
    // So, instead we can check if the metadata is the same, and if the album art has
    // not changed.
    compare_metadata(song, target, desired_bitrate, pb)
}

#[cfg(test)]
mod tests {
    use crate::{
        ffmpeg_interface::SongMetaData,
        music_library::{
            get_shadow_filename, library_relative_path, ArtStrategy, ArtworkType, MusicFileType,
            UpdateType,
        },
        song::Song,
        test_data::TestFile,
    };
    use std::path::PathBuf;

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

        f(&original_song, &shadow, None, 60, None)
    }

    #[test]
    /// Calling it on the same file.
    fn has_music_file_changed_identical_file() {
        assert_eq!(
            construct_has_music_file_changed("no_art", "no_art"),
            UpdateType::NoChange,
            "identical file, should say it has not changed"
        )
    }

    /// For tests that should have changed
    fn construct_should_have_changed(mod_suffix: &str) {
        let is_changed =
            construct_has_music_file_changed("no_art", &format!("no_art_changed_{}", mod_suffix));
        assert_eq!(
            is_changed,
            UpdateType::Overwrite,
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
        test_file: TestFile,
        target_filetype: MusicFileType,
        external_art: Option<TestFile>,
        art_strategy: ArtStrategy,
    ) -> miette::Result<()> {
        use super::sync_song;

        let mut d = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        d.push("test_data/");
        let source_library: PathBuf = d;
        let target_library: PathBuf = format!(
            "/tmp/syncbops/sync_test_lib_{:?}_to{:?}_{:?}_{:?}",
            test_file, target_filetype, external_art, art_strategy
        )
        .into();
        let _ = std::fs::create_dir(&target_library);
        // Delete anything that's already there, because we wanna test it if it's a new file.
        let library_relative_path = library_relative_path(&test_file.path(), &source_library);
        let target = get_shadow_filename(&library_relative_path, &target_library, &target_filetype);
        let _ = std::fs::remove_file(&target);
        assert!(!target.exists());

        // let target_filetype = MusicFileType::Mp3CBR { bitrate: 60 };
        let song = Song::new_debug(test_file.path(), external_art.map(|tf| tf.path()))?;
        let updated_record = sync_song(
            &song,
            &target_library,
            target_filetype.clone(),
            art_strategy,
            None,
            false,
            false,
            None,
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
}
