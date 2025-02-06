use std::{
    path::Path,
    process::{Command, Stdio},
};

/// Queries `ffprobe "04. FREEDOM.mp3" 2>&1 | grep "Cover"`.
pub fn does_file_have_embedded_artwork(path: &Path) -> bool {
    let ffprobe = Command::new("ffprobe")
        .arg(path)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let grep_cover = Command::new("grep")
        .arg("Cover")
        .stdin(Stdio::from(ffprobe.stdout.unwrap()))
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    match grep_cover.wait_with_output() {
        // Grep returns a status code of OK if there is a match.
        Ok(x) => {
            return x.stdout.is_empty();
        }
        Err(e) => eprintln!(
            "Could not check for embedded artwork in file '{}': {}",
            path.display(),
            e
        ),
    }
    false
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
    }
}
