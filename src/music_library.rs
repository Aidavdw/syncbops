use crate::ffmpeg_interface::does_file_have_embedded_artwork;
use crate::ffmpeg_interface::transcode_song;
use crate::ffmpeg_interface::FfmpegError;
use crate::song::Song;
use rayon::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq)]
pub enum UpdateType {
    /// The file did not need to be changed, as it is up-to-date
    Unchanged,
    /// The file is completely new, so everything had to be done from scratch
    New,
    /// Updated because it was modified more recently than the shadow copy
    Overwritten,
}

// TODO: Phase out albums, use Song instead.
/// Represents an album: A directory with songs in it.
#[derive(Debug)]
pub struct Album {
    /// All the files in it that are music
    pub music_files: Vec<PathBuf>,
    /// If the album has an art file, like a cover.jpg.
    pub album_art: Option<PathBuf>,
}

pub enum MusicFileType {
    Mp3,
    Flac,
}

pub enum ImageType {
    Png,
    Jpg,
}

pub enum FileType {
    Music(MusicFileType),
    Art(ImageType),
}

fn identify_file_type(path: &Path) -> Option<FileType> {
    let ext = path.extension()?.to_ascii_lowercase();
    Some(match ext.as_os_str().to_str()? {
        "mp3" => FileType::Music(MusicFileType::Mp3),
        "flac" => FileType::Music(MusicFileType::Flac),
        "png" => FileType::Art(ImageType::Png),
        "jpg" => FileType::Art(ImageType::Jpg),
        "jpeg" => FileType::Art(ImageType::Jpg),
        _ => return None,
    })
}

/// Checks if the file meets the criteria to be considered dedicated album art: is it named
/// cover.jpg or something?
fn is_image_file_album_art(path: &Path) -> bool {
    // if it's something like "cover" or "folder"
    const ALLOWED_STEMS: [&str; 5] = ["cover", "folder", "album", "cover_image", "cover_art"];
    let stem_is_allowed: bool = ALLOWED_STEMS.iter().any(|x| {
        path.file_stem()
            .is_some_and(|s| s.to_ascii_lowercase() == *x)
    });

    let has_right_extension =
        identify_file_type(path).is_some_and(|file_type| matches!(file_type, FileType::Art(_)));

    stem_is_allowed && has_right_extension
}

pub fn find_albums_in_directory(path: &PathBuf) -> Result<Vec<Album>, MusicLibraryError> {
    // Iterate through the folders. If there is a music file here, then this should be an
    // album.
    // if there are no music files here, then go some level deeper, because there might be
    // music in a sub-folder.
    // If there are no music files, and there are also no sub-folders, then ignore this foledr
    // and continue with the next one.
    if !path.is_dir() {
        return Err(MusicLibraryError::NotADirectory {
            path: path.to_path_buf(),
        });
    }
    let dir = match fs::read_dir(path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!(
                "Unable to process '{}' as a directory to find albums in: {}",
                path.display(),
                e
            );
            return Ok(Vec::new());
        }
    };
    let mut albums = Vec::new();
    let mut music_files = Vec::new();
    let mut album_art = None;
    for entry in dir {
        let sub_path = match entry {
            Ok(x) => x,
            Err(e) => {
                // Can't convert entry itself into a string, so logging needs to be a little
                // more indirect
                eprintln!(
                    "Unable to process an entry in folder {}: {}",
                    path.display(),
                    e
                );
                continue;
            }
        }
        .path();
        if sub_path.is_dir() {
            // Recurse
            match find_albums_in_directory(&sub_path) {
                Ok(albums_in_sub_dir) => albums.extend(albums_in_sub_dir),
                Err(e) => eprintln!(
                    "Error in processing sub-directory of {}: {}",
                    path.display(),
                    e
                ),
            }
        } else {
            let Some(filetype) = identify_file_type(&sub_path) else {
                println!("Ignoring file {}", sub_path.display());
                continue;
            };
            match filetype {
                FileType::Music(_) => {
                    music_files.push(sub_path);
                }
                FileType::Art(_) => {
                    if album_art.is_none() && is_image_file_album_art(&sub_path) {
                        album_art = Some(sub_path)
                    }
                }
            }
        }
    }

    // If music files are found, create an album and add it to the collection
    if !music_files.is_empty() {
        albums.push(Album {
            music_files,
            album_art,
        });
    } else if albums.is_empty() {
        println!(
            "No music files found in {} (and its subfolders)",
            path.display()
        )
    };
    Ok(albums)
}

/// Checks if the source music file has been changed since it has been transcoded. First checks
/// if the source file is newer (more recently changed), and if not, checks if the metadata is
/// identical.
pub fn has_music_file_changed(source: &Path, target: &Path) -> bool {
    if !target.exists() {
        // If the target doesn't exist, it must be newer.
        return true;
    }
    // Get the metadata for both files
    let source_last_modified = fs::metadata(source)
        .expect("Unable to read source file metadata.")
        .modified()
        .expect("could not get modification time for source");
    let target_last_modified = fs::metadata(target)
        .expect("Unable to read target file metadata.")
        .modified()
        .expect("could not get modification time for source");
    if source_last_modified > target_last_modified {
        return true;
    }
    false
}

pub fn songs_without_album_art(albums: &[Album]) -> Result<Vec<PathBuf>, FfmpegError> {
    // If there is an associated album art file, there definitely is album art. If there is
    // not, check if there is embedde art for each file (costlier)
    let songs = albums
        .iter()
        .filter(|album| album.album_art.is_none())
        .flat_map(|album| album.music_files.clone())
        .collect::<Vec<_>>();
    // Separately run the querying function, because it can error. if it errors, exit the entire
    // function.
    // TODO: Return the paths where it resulted in an error too
    let results = songs
        .par_iter()
        .map(|music_file| does_file_have_embedded_artwork(music_file))
        .collect::<Result<Vec<_>, _>>()?;

    let a = songs
        .iter()
        .zip(results.iter())
        .filter(|(_, b)| !**b)
        .map(|(filename, _)| filename.to_owned())
        .collect::<Vec<_>>();

    Ok(a)
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
    source_library: &Path,
    target_library: &Path,
    v_level: u64,
    art_strategy: ArtStrategy,
    force: bool,
    dry_run: bool,
) -> Result<UpdateType, MusicLibraryError> {
    // Early exit if it doesn't need to be updated.
    // Can't change files in place with ffmpeg, so if we need to update then we need to
    // overwrite the file anyway.
    let mut how_updated = song.status(source_library, target_library);
    if how_updated == UpdateType::Unchanged && !force {
        return Ok(UpdateType::Unchanged);
    }
    if how_updated == UpdateType::Unchanged && force {
        how_updated = UpdateType::Overwritten
    }

    // If the source directory does not yet exist, create it. ffmpeg will otherwise throw an error.
    // TODO: Only ignore error if the folder already exists, otherwise bubble up error.
    let shadow = song.get_shadow_filename(source_library, target_library);
    let _ = fs::create_dir_all(shadow.parent().expect("Cannot get parent dir of shadow"));

    // TODO: If the source file is already a lower bitrate, then don't do any transcoding.
    let embed_art = match art_strategy {
        ArtStrategy::None => false,
        ArtStrategy::EmbedAll => true,
        ArtStrategy::PreferFile => song.external_album_art.is_none(),
        ArtStrategy::FileOnly => false,
    };
    if !dry_run {
        transcode_song(
            &song.path,
            &shadow,
            v_level,
            embed_art,
            song.external_album_art.as_deref(),
        )?
    };

    Ok(how_updated)
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

    #[error("Error in calling ffmpeg")]
    Ffmpeg(#[from] FfmpegError),

    #[error("The given target directory '{target_library}' does not (yet) exist. Please make sure the folder exists, even if it is just an empty folder!")]
    TargetLibraryDoesNotExist { target_library: PathBuf },
}

#[cfg(test)]
mod tests {
    use super::{songs_without_album_art, Album};
    use crate::{
        ffmpeg_interface::does_file_have_embedded_artwork,
        music_library::{ArtStrategy, UpdateType},
        song::Song,
    };
    use core::time;
    use itertools::Itertools;
    use std::{fs::File, path::PathBuf, thread::sleep};

    #[test]
    fn has_music_file_changed_based_on_last_modified_time() {
        use super::has_music_file_changed as f;
        let older_file: PathBuf = "/tmp/older_file.mp3".into();
        let newer_file: PathBuf = "/tmp/newer_file.mp3".into();
        File::create(older_file.clone()).unwrap();
        sleep(time::Duration::new(2, 0));
        File::create(newer_file.clone()).unwrap();
        assert!(!f(&older_file, &newer_file));
        assert!(f(&newer_file, &older_file));
    }

    fn sync_song_test(
        identifier: &str,
        song: Song,
        art_strategy: ArtStrategy,
    ) -> miette::Result<()> {
        use super::sync_song;

        let source_library: PathBuf = "/home/aida/portable_music/".into();
        let target_library: PathBuf = format!("/tmp/target_library_{}", identifier).into();
        let _ = std::fs::create_dir(&target_library);
        // Delete anything that's already there, because we wanna test it if it's a new file.
        let target = song.get_shadow_filename(&source_library, &target_library);
        let _ = std::fs::remove_file(&target);
        let updated = sync_song(
            &song,
            &source_library,
            &target_library,
            3,
            art_strategy,
            false,
            false,
        )?;

        assert!(updated == UpdateType::New);
        match art_strategy {
            ArtStrategy::None => assert!(
                !does_file_have_embedded_artwork(&target)?,
                "Art strategy is to have no artwork yet there is embedded artwork."
            ),
            ArtStrategy::EmbedAll => {
                // Can't have any artwork if there never was any.
                if song.has_artwork()? {
                    assert!(
                        does_file_have_embedded_artwork(&target)?,
                        "ArtStrategy::EmbedAll, yet no embedded artwork.."
                    )
                }
            }
            ArtStrategy::PreferFile => {
                if song.external_album_art.is_some() {
                    assert!(
                        !does_file_have_embedded_artwork(&target)?,
                        "If song has dedicated artwork, it should copy it over with this ArtStrategy, and not embed it."
                    )
                } else if does_file_have_embedded_artwork(&song.path)? {
                    assert!(does_file_have_embedded_artwork(&target)?, "Even though not preferred option, should still retain artwork that was already embedded")
                }
            }
            ArtStrategy::FileOnly => {
                assert!(
                    !does_file_have_embedded_artwork(&target)?,
                    "If File Only, should not have any embedded artwork."
                )
            }
        }

        // The it should not be overwritten if
        Ok(())
    }

    fn with_embedded_album_art() -> PathBuf {
        "/home/aida/portable_music/Ado/狂言/04. FREEDOM.mp3".into()
    }

    fn without_art() -> PathBuf {
        "/home/aida/portable_music/Area 11/All The Lights In The Sky/1-03. Euphemia.mp3".into()
    }

    fn external_art() -> Option<PathBuf> {
        Some("/home/aida/portable_music/Area 11/All The Lights In The Sky/folder.jpg".into())
    }

    // ART STRATEGY = NONE

    #[test]
    /// Song with embedded album art, no external, art strategy = none.
    fn sync_song_artstrat_none_embedded_art() -> miette::Result<()> {
        sync_song_test(
            "artstrat_none/embedded",
            Song {
                path: with_embedded_album_art(),
                external_album_art: None,
            },
            ArtStrategy::None,
        )
    }

    #[test]
    /// Song with external art only, art strategy = none
    fn sync_song_artstrat_none_external_art() -> miette::Result<()> {
        sync_song_test(
            "artstrat_none/external",
            Song {
                path: without_art(),
                external_album_art: external_art(),
            },
            ArtStrategy::None,
        )
    }

    #[test]
    /// Song with no art at all, art strategy = none
    fn sync_song_artstrat_none_no_art() -> miette::Result<()> {
        sync_song_test(
            "artstrat_none/no-art",
            Song {
                path: without_art(),
                external_album_art: None,
            },
            ArtStrategy::None,
        )
    }

    #[test]
    /// Song with both embedded and external art, art strategy = none.
    fn sync_song_artstrat_none_both() -> miette::Result<()> {
        sync_song_test(
            "artstrat_none/both",
            Song {
                path: with_embedded_album_art(),
                external_album_art: external_art(),
            },
            ArtStrategy::None,
        )
    }

    // END ART STRATEGY = NONE
    // ART STRATEGY = EMBED ALL

    #[test]
    /// Song with embedded album art, no external, art strategy = none.
    fn sync_song_artstrat_embed_embedded_art() -> miette::Result<()> {
        sync_song_test(
            "artstrat_embed/embedded",
            Song {
                path: with_embedded_album_art(),
                external_album_art: None,
            },
            ArtStrategy::EmbedAll,
        )
    }

    #[test]
    /// Song with external art only, art strategy = none
    fn sync_song_artstrat_embed_external_art() -> miette::Result<()> {
        sync_song_test(
            "artstrat_embed/external",
            Song {
                path: without_art(),
                external_album_art: external_art(),
            },
            ArtStrategy::EmbedAll,
        )
    }

    #[test]
    /// Song with no art at all, art strategy = none
    fn sync_song_artstrat_embed_no_art() -> miette::Result<()> {
        sync_song_test(
            "artstrat_embed/no-art",
            Song {
                path: without_art(),
                external_album_art: None,
            },
            ArtStrategy::EmbedAll,
        )
    }

    #[test]
    /// Song with both embedded and external art, art strategy = none.
    fn sync_song_artstrat_embed_both() -> miette::Result<()> {
        sync_song_test(
            "artstrat_embed/both",
            Song {
                path: with_embedded_album_art(),
                external_album_art: external_art(),
            },
            ArtStrategy::EmbedAll,
        )
    }

    // END ART STRATEGY = EMBED ALL
    // ART STRATEGY = PREFER_FILE

    #[test]
    /// Song with embedded album art, no external, art strategy = prefer file.
    fn sync_song_artstrat_prefer_file_embedded_art() -> miette::Result<()> {
        sync_song_test(
            "artstrat_prefer_file/embedded",
            Song {
                path: with_embedded_album_art(),
                external_album_art: None,
            },
            ArtStrategy::PreferFile,
        )
    }

    #[test]
    /// Song with external art only, art strategy = prefer_file
    fn sync_song_artstrat_prefer_file_external_art() -> miette::Result<()> {
        sync_song_test(
            "artstrat_prefer_file/external",
            Song {
                path: without_art(),
                external_album_art: external_art(),
            },
            ArtStrategy::PreferFile,
        )
    }

    #[test]
    /// Song with no art at all, art strategy = prefer_file
    fn sync_song_artstrat_prefer_file_no_art() -> miette::Result<()> {
        sync_song_test(
            "artstrat_prefer_file/no-art",
            Song {
                path: without_art(),
                external_album_art: None,
            },
            ArtStrategy::PreferFile,
        )
    }

    #[test]
    /// Song with both embedded and external art, art strategy = prefer_file.
    fn sync_song_artstrat_prefer_file_both() -> miette::Result<()> {
        sync_song_test(
            "artstrat_prefer_file/both",
            Song {
                path: with_embedded_album_art(),
                external_album_art: external_art(),
            },
            ArtStrategy::PreferFile,
        )
    }

    // END ART STRATEGY = PREFER_FILE
    // ART STRATEGY = FILE_ONLY

    #[test]
    /// Song with embedded album art, no external, art strategy = file_only.
    fn sync_song_artstrat_file_only_embedded_art() -> miette::Result<()> {
        sync_song_test(
            "artstrat_file_only/embedded",
            Song {
                path: with_embedded_album_art(),
                external_album_art: None,
            },
            ArtStrategy::FileOnly,
        )
    }

    #[test]
    /// Song with external art only, art strategy = file_only
    fn sync_song_artstrat_file_only_external_art() -> miette::Result<()> {
        sync_song_test(
            "artstrat_file_only/external",
            Song {
                path: without_art(),
                external_album_art: external_art(),
            },
            ArtStrategy::FileOnly,
        )
    }

    #[test]
    /// Song with no art at all, art strategy = file_only
    fn sync_song_artstrat_file_only_no_art() -> miette::Result<()> {
        sync_song_test(
            "artstrat_file_only/no-art",
            Song {
                path: without_art(),
                external_album_art: None,
            },
            ArtStrategy::FileOnly,
        )
    }

    #[test]
    /// Song with both embedded and external art, art strategy = file_only.
    fn sync_song_artstrat_file_only_both() -> miette::Result<()> {
        sync_song_test(
            "artstrat_file_only/both",
            Song {
                path: with_embedded_album_art(),
                external_album_art: external_art(),
            },
            ArtStrategy::FileOnly,
        )
    }

    // END ART STRATEGY = FILE_ONLY

    #[test]
    fn songs_without_album_art_test() -> miette::Result<()> {
        let file_with_embedded_artwork: PathBuf =
            "/home/aida/portable_music/Ado/狂言/04. FREEDOM.mp3".into();

        let file_without_embedded_artwork: PathBuf =
            "/home/aida/portable_music/Area 11/All The Lights In The Sky/1-02. Vectors.mp3".into();

        // One album only, only embedded
        assert_eq!(
            songs_without_album_art(&[Album {
                music_files: vec![
                    file_with_embedded_artwork.clone(),
                    file_without_embedded_artwork.clone()
                ],
                album_art: None
            }])?
            .iter()
            .exactly_one()
            .unwrap(),
            &file_without_embedded_artwork
        );

        // Two albums, only embbeded
        assert_eq!(
            songs_without_album_art(&[
                Album {
                    music_files: vec![
                        file_with_embedded_artwork.clone(),
                        file_without_embedded_artwork.clone()
                    ],
                    album_art: None
                },
                Album {
                    music_files: vec![
                        file_with_embedded_artwork.clone(),
                        file_without_embedded_artwork.clone()
                    ],
                    album_art: None
                }
            ])?
            .len(),
            2
        );

        // Two albums, one embedded, the other dedicated.
        assert_eq!(
            songs_without_album_art(&[
                Album {
                    music_files: vec![
                        file_with_embedded_artwork.clone(),
                        file_without_embedded_artwork.clone()
                    ],
                    album_art: None
                },
                Album {
                    music_files: vec![
                        file_with_embedded_artwork.clone(),
                        file_without_embedded_artwork.clone()
                    ],
                    album_art: Some(PathBuf::default())
                }
            ])?
            .iter()
            .exactly_one()
            .unwrap(),
            &file_without_embedded_artwork
        );

        assert!(songs_without_album_art(&[Album {
            music_files: vec![
                file_with_embedded_artwork.clone(),
                file_without_embedded_artwork.clone()
            ],
            album_art: Some(PathBuf::default())
        }])?
        .is_empty());

        assert!(songs_without_album_art(&[Album {
            music_files: vec![
                file_without_embedded_artwork.clone(),
                file_without_embedded_artwork.clone()
            ],
            album_art: Some(PathBuf::default())
        }])?
        .is_empty());
        Ok(())
    }
}
