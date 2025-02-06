use std::fs::{self, DirEntry};
use std::io;
use std::path::{Path, PathBuf};

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

pub enum ImageType {}

pub enum FileType {
    Music(MusicFileType),
    Art(ImageType),
    Lyrics,
}

fn identify_file_type(path: &Path) -> Option<FileType> {
    let ext = path.extension()?.to_ascii_lowercase();
    if ext == "mp3" {
        Some(FileType::Music(MusicFileType::Mp3))
    } else if ext == "flac" {
        Some(FileType::Music(MusicFileType::Flac))
    } else {
        None
    }
}

pub fn find_albums_in_directory(path: &PathBuf) -> Result<Vec<Album>, MusicLibraryError> {
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
                    if album_art.is_none() {
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

#[derive(thiserror::Error, Debug)]
pub enum MusicLibraryError {
    #[error("Tried to discover albums in directory '{path}', but that is not a directory.")]
    NotADirectory { path: PathBuf },

    #[error("IO error")]
    Io(#[from] io::Error),

    #[error("No albums found in directory {dir}")]
    NoAlbumsFound { dir: PathBuf },
}
