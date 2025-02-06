use std::{
    path::Path,
    process::{Command, Stdio},
};

/// Queries `ffprobe "04. FREEDOM.mp3" 2>&1 | grep "Cover"`.
pub fn does_file_have_embedded_artwork(path: &Path) -> bool {
    let ffprobe = Command::new("ffprobe").arg(path).output().unwrap();
    let txt = String::from_utf8(ffprobe.stderr).unwrap();
    txt.contains("Cover")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::does_file_have_embedded_artwork;

    #[test]
    fn embedded_artwork() {
        let file_with_embedded_artwork: PathBuf =
            "/home/aida/portable_music/Ado/狂言/04. FREEDOM.mp3".into();
        assert!(does_file_have_embedded_artwork(&file_with_embedded_artwork));

        let file_without_embedded_artwork: PathBuf =
            "/home/aida/portable_music/Area 11/All The Lights In The Sky/1-02. Vectors.mp3".into();
        assert!(!does_file_have_embedded_artwork(
            &file_without_embedded_artwork
        ))
    }
}
