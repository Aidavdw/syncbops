use rayon::prelude::*;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::ffmpeg_interface::does_file_have_embedded_artwork;

// Represents an album: A directory with songs in it.
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

pub fn songs_without_album_art(albums: &[Album]) -> Vec<PathBuf> {
    // If there is an associated album art file, there definitely is album art. If there is
    // not, check if there is embedde art for each file (costlier)
    albums
        .par_iter()
        .filter(|album| album.album_art.is_none())
        .flat_map(|album| album.music_files.clone())
        .filter(|music_file| !does_file_have_embedded_artwork(music_file))
        .collect()
}

#[derive(thiserror::Error, Debug)]
pub enum MusicLibraryError {
    #[error("Tried to discover albums in directory '{path}', but that is not a directory.")]
    NotADirectory { path: PathBuf },

    #[error("IO error")]
    Io(#[from] io::Error),

    #[error("No albums found in directory {dir}")]
    NoAlbumsFound { dir: PathBuf },
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use itertools::Itertools;

    use super::{songs_without_album_art, Album};

    #[test]
    fn songs_without_album_art_test() {
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
            }])
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
            ])
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
            ])
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
        }])
        .is_empty());

        assert!(songs_without_album_art(&[Album {
            music_files: vec![
                file_without_embedded_artwork.clone(),
                file_without_embedded_artwork.clone()
            ],
            album_art: Some(PathBuf::default())
        }])
        .is_empty());
    }
}
