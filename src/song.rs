use crate::{
    ffmpeg_interface::{does_file_have_embedded_artwork, FfmpegError},
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
    /// Where to put the synchronised copy
    pub fn get_shadow_filename(
        &self,
        source_library: &Path,
        target_library: &Path,
        filetype: &MusicFileType,
    ) -> PathBuf {
        target_library.join(
            self.library_relative_path(source_library)
                .with_extension(filetype.to_string()),
        )
    }

    /// gets the path relative to the library.
    pub fn library_relative_path(&self, source_library: &Path) -> PathBuf {
        self.path
            .strip_prefix(source_library)
            .unwrap()
            .to_path_buf()
    }

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
    /// How has the file changed since the last sync?
    pub fn status(
        &self,
        source_library: &Path,
        target_library: &Path,
        filetype: &MusicFileType,
    ) -> UpdateType {
        // TODO:If it exists with a different filetype, give a warning
        let shadow = self.get_shadow_filename(source_library, target_library, filetype);
        if !has_music_file_changed(&self.path, &shadow) {
            UpdateType::Unchanged
        } else if shadow.exists() {
            UpdateType::Overwritten
        } else {
            UpdateType::New
        }
    }
}

impl Display for Song {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let p = self.path.to_str().unwrap();
        write!(f, "Song @{}, artwork={:?}", p, self.has_artwork())
    }
}
