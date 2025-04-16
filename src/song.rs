use crate::{
    ffmpeg_interface::{does_file_have_embedded_artwork, FfmpegError},
    hashing::PreviousSyncDb,
    music_library::{
        has_music_file_changed, ArtworkType, MusicFileType, MusicLibraryError, UpdateType,
    },
};
use std::{
    fmt::Display,
    path::{Path, PathBuf},
};

#[derive(Debug, PartialEq)]
pub struct Song {
    /// Where the original song file can be found
    pub path: PathBuf,

    /// Where the external album art is, if it exists.
    pub external_album_art: Option<PathBuf>,
}

impl Song {
    pub fn has_artwork(&self) -> ArtworkType {
        if self.external_album_art.is_some() {
            return ArtworkType::External;
        }
        let Ok(has_embedded_artwork) = does_file_have_embedded_artwork(&self.path) else {
            eprintln!("Could not read artwork for {}: ", self);
            return ArtworkType::None;
        };
        if has_embedded_artwork {
            ArtworkType::Embedded
        } else {
            ArtworkType::None
        }
    }
}

impl Display for Song {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let p = self.path.to_str().unwrap();
        write!(f, "Song @{}, artwork={:?}", p, self.has_artwork())
    }
}
