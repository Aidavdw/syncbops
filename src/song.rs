use crate::{ffmpeg_interface::SongMetaData, music_library::ArtworkType};
use std::{fmt::Display, path::PathBuf};

#[derive(Debug, PartialEq)]
pub struct Song {
    /// Where the original song file can be found
    pub path: PathBuf,

    /// Where the external album art is, if it exists.
    pub external_album_art: Option<PathBuf>,
}

impl Song {
    // Does the song have artwork information? Can use a
    pub fn has_artwork(&self, cached_metadata: Option<SongMetaData>) -> ArtworkType {
        if self.external_album_art.is_some() {
            return ArtworkType::External;
        }
        let metadata = cached_metadata.unwrap_or_else(|| {
            SongMetaData::parse_file(&self.path).unwrap_or_else(|e| {
                panic!(
                    "song {self} should have correct path, but still cannot parse into metadata: {e}"
                );
            })
        });
        if metadata.has_embedded_album_art {
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
        }
        Ok(())
    }
}
