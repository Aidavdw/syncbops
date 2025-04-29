use crate::{
    ffmpeg_interface::SongMetaData,
    music_library::{library_relative_path, ArtworkType, MusicLibraryError},
};
use std::{fmt::Display, path::PathBuf};

#[derive(Debug, PartialEq)]
pub struct Song {
    /// Where the original song file can be found
    pub absolute_path: PathBuf,
    /// The location of the song file relative to the source library.
    pub library_relative_path: PathBuf,

    /// Where the external album art is, if it exists.
    pub external_album_art: Option<PathBuf>,

    pub metadata: SongMetaData,
}

impl Song {
    /// Creates a new song. Also reads its metadata.
    pub fn new(
        path: PathBuf,
        source_library: PathBuf,
        external_album_art: Option<PathBuf>,
    ) -> Result<Song, MusicLibraryError> {
        let metadata = SongMetaData::parse_file(&path)?;
        let library_relative_path = library_relative_path(&path, &source_library);
        Ok(Song {
            absolute_path: path,
            external_album_art,
            metadata,
            library_relative_path,
        })
    }

    // Does the song have artwork information? Can use a
    pub fn has_artwork(&self) -> ArtworkType {
        if self.external_album_art.is_some() {
            return ArtworkType::External;
        }
        if self.metadata.has_embedded_album_art {
            ArtworkType::Embedded
        } else {
            ArtworkType::None
        }
    }

    /// Shorthand for creating a new Song.
    /// Sets the library as the dir the file is in directly (no nesting)
    #[cfg(test)]
    pub fn new_debug(
        path: PathBuf,
        external_album_art: Option<PathBuf>,
    ) -> Result<Song, MusicLibraryError> {
        let parent_directory = path
            .parent()
            .expect("Cannot get parent directory for making a debug Song")
            .to_path_buf();
        Song::new(path, parent_directory, external_album_art)
    }
}

impl Display for Song {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let p = self.absolute_path.to_str().unwrap();
        write!(f, "Song @{}", p)?;
        if let Some(external_art_path) = &self.external_album_art {
            write!(f, "w/ external art ({})", external_art_path.display())?;
        } else if self.metadata.has_embedded_album_art {
            write!(f, "w/ embedded art")?;
        } else {
            write!(f, "w/o art")?;
        }
        Ok(())
    }
}
