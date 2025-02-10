use crate::ffmpeg_interface::does_file_have_embedded_artwork;
use crate::ffmpeg_interface::transcode_song;
use crate::ffmpeg_interface::FfmpegError;
use crate::song::Song;
use rayon::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug)]
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
    Lyrics,
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
                FileType::Lyrics => todo!(),
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

/// Synchronises the file. Returns true if the file is updated, false it was not.
pub fn sync_song(
    song: &Song,
    source_library: &Path,
    target_library: &Path,
    v_level: u32,
    include_album_art: bool,
) -> Result<UpdateType, MusicLibraryError> {
    // Early exit if it doesn't need to be updated.
    let shadow = song.get_shadow_filename(source_library, target_library);
    if !has_music_file_changed(&song.path, &shadow) {
        return Ok(UpdateType::Unchanged);
    }

    // Can't change files in place with ffmpeg, so if we need to update then we need to
    // overwrite the file anyway.
    let how_updated = if shadow.exists() {
        UpdateType::Overwritten
    } else {
        UpdateType::New
    };

    // If the source directory does not yet exist, create it. ffmpeg will otherwise throw an error.
    // TODO: Only ignore error if the folder already exists, otherwise bubble up error.
    let a = fs::create_dir_all(
        shadow
            .parent()
            .expect("Cannot create picture in target library"),
    );

    // TODO: If the source file is already a lower bitrate, then don't do any transcoding.
    transcode_song(
        &song.path,
        &shadow,
        v_level,
        include_album_art,
        song.external_album_art.as_deref(),
    )?;

    Ok(how_updated)
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
    use crate::song::Song;
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

    #[test]
    fn sync_song() -> miette::Result<()> {
        use super::sync_song as f;

        // New song, that doesn't have a shadow copy yet
        let source_library: PathBuf = "/home/aida/portable_music/".into();
        let target_library: PathBuf = "/tmp/target_library".into();
        let new_song = Song {
            path: "/home/aida/portable_music/Ado/狂言/04. FREEDOM.mp3".into(),
            external_album_art: None,
        };
        let _ =
            std::fs::remove_file(new_song.get_shadow_filename(&source_library, &target_library));
        f(&new_song, &source_library, &target_library, 3, false)?;
        Ok(())
    }

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
