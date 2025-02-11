use std::path::{Path, PathBuf};

use crate::{
    ffmpeg_interface::{does_file_have_embedded_artwork, transcode_song, FfmpegError},
    music_library::{has_music_file_changed, MusicLibraryError, UpdateType},
};

pub struct Song {
    /// Where the original song file can be found
    pub path: PathBuf,

    /// Where the external album art is, if it exists.
    pub external_album_art: Option<PathBuf>,
}

impl Song {
    /// Where to put the synchronised copy
    pub fn get_shadow_filename(&self, source_library: &Path, target_library: &Path) -> PathBuf {
        target_library.join(self.library_relative_path(source_library))
    }

    /// gets the path relative to the library.
    pub fn library_relative_path(&self, source_library: &Path) -> PathBuf {
        self.path
            .strip_prefix(source_library)
            .unwrap()
            .to_path_buf()
    }

    pub fn has_artwork(&self) -> Result<bool, FfmpegError> {
        if self.external_album_art.is_some() {
            return Ok(true);
        }
        does_file_have_embedded_artwork(&self.path)
    }
}
