use crate::{
    ffmpeg_interface::SongMetaData,
    music_library::{ArtworkType, MusicLibraryError},
};
use std::{fmt::Display, path::PathBuf};

#[derive(Debug, PartialEq)]
pub struct Song {
    /// Where the original song file can be found
    pub path: PathBuf,

    /// Where the external album art is, if it exists.
    pub external_album_art: Option<PathBuf>,

    pub metadata: SongMetaData,
}

impl Song {
    // Creates a new song. Also reads its metadata.
    pub fn new(
        path: PathBuf,
        external_album_art: Option<PathBuf>,
    ) -> Result<Song, MusicLibraryError> {
        let metadata = SongMetaData::parse_file(&path)?;
        Ok(Song {
            path,
            external_album_art,
            metadata,
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
}

impl Display for Song {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let p = self.path.to_str().unwrap();
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
