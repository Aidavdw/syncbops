use std::path::PathBuf;

#[derive(Debug, Clone)]
pub enum TestFile {
    Mp3CBRWithArt,
    Mp3CBRWithoutArt,
    FlacWithArt,
    FlacWithoutArt,
    M4aWithArt,
    M4aWithoutArt,
    OggWithArt,
    OggWithoutArt,
    Jpg600,
}

impl TestFile {
    pub fn path(&self) -> PathBuf {
        let mut d = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        d.push("test_data");
        let a = match self {
            TestFile::Mp3CBRWithArt => "with_art.mp3",
            TestFile::Mp3CBRWithoutArt => "no_art.mp3",
            TestFile::FlacWithArt => "with_art.flac",
            TestFile::FlacWithoutArt => "no_art.flac",
            TestFile::M4aWithArt => "with_art.m4a",
            TestFile::M4aWithoutArt => "no_art.m4a",
            TestFile::OggWithArt => "with_art.ogg",
            TestFile::OggWithoutArt => "no_art.ogg",
            TestFile::Jpg600 => "cover_art.jpg",
        };
        d.push(a);
        d
    }
}
