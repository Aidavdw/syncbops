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
    Rotterdam128kbpsMp3,
    Rotterdam128kbpsM4a,
    RotterdamFlac,
    Rotterdam96kbpsMp3,
    Rotterdam110kbpsM4a,
    Rotterdam96kbpsOpus,
    Rotterdam96kbpsOpusWithArt,
    Rotterdam128kbpsOpus,
    Rotterdam128kbpsOpusWithArt,
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
            TestFile::Rotterdam128kbpsMp3 => "ns_rotterdam_128kbps.mp3",
            TestFile::Rotterdam128kbpsM4a => "ns_rotterdam_128kbps.m4a",
            TestFile::RotterdamFlac => "ns_rotterdam.flac",
            TestFile::Rotterdam96kbpsMp3 => "ns_rotterdam_96kbps.mp3",
            TestFile::Rotterdam110kbpsM4a => "ns_rotterdam_110kbps.m4a",
            TestFile::Rotterdam96kbpsOpus => "ns_rotterdam_96kbps.opus",
            TestFile::Rotterdam96kbpsOpusWithArt => "ns_rotterdam_96kbps_art.opus",
            TestFile::Rotterdam128kbpsOpus => "ns_rotterdam_128kbps.opus",
            TestFile::Rotterdam128kbpsOpusWithArt => "ns_rotterdam_128kbps_art.opus",
        };
        d.push(a);
        debug_assert!(d.exists(), "Test data does not exist!");
        d
    }
}

// pub const COMPARISON_BENCHMARK_TEST_FILES: [TestFile; 3] = [
//     TestFile::Mp3CBRWithArt,
//     TestFile::FlacWithArt,
//     TestFile::Rotterdam128kbpsM4a,
// ];
