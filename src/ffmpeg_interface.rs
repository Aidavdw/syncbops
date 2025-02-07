use std::ffi::OsString;
use std::io::{Seek, SeekFrom, Write};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// Queries `ffprobe "04. FREEDOM.mp3" 2>&1 | grep "Cover"`.
pub fn does_file_have_embedded_artwork(path: &Path) -> bool {
    let ffprobe = Command::new("ffprobe").arg(path).output().unwrap();
    let txt = String::from_utf8(ffprobe.stderr).unwrap();
    txt.contains("Cover")
}

/// Takes a path of a song file, transcodes it using ffmpeg, and saves it to the target path. Returns the path of the output file. Like `ffmpeg -i [input file] -codec:a libmp3lame -q:a [V-level] [output file].mp3`
pub fn transcode_song(
    source: &Path,
    target: &Path,
    v_level: u32,
    include_album_art: bool,
    external_album_art: Option<&Path>,
) -> Result<(), FfmpegError> {
    debug_assert!(
        v_level < 9,
        "Presets for v-level compression only go up to 9. Be sure to sanitise input."
    );
    let mut binding = Command::new("ffmpeg");
    binding
        .arg("-y") // Replace if it already exists
        .arg("-i")
        .arg(source)
        .arg("-codec:a")
        .arg("libmp3lame")
        .arg("-q:a")
        .arg(v_level.to_string());

    // TODO: embed artwork if missing
    if include_album_art {
        ()
        //if does_file_have_embedded_artwork(source) {}
    } else {
        binding.arg("-vn");
    }

    let cmd = binding.arg(target);
    let output = cmd.output()?;
    if !output.status.success() {
        let msg = String::from_utf8_lossy(&output.stderr).to_string();
        return Err(FfmpegError::Transcode {
            file: source.into(),
            msg,
        });
    }
    //if does_file_have_embedded_artwork(source) {
    //
    //}
    Ok(())
}

#[derive(thiserror::Error, Debug)]
pub enum FfmpegError {
    #[error("Tried to discover albums in directory '{path}', but that is not a directory.")]
    NotADirectory { path: PathBuf },

    #[error("Could not transcode file {file}: {msg} ")]
    Transcode { file: PathBuf, msg: String },

    #[error("IO error")]
    Io(#[from] std::io::Error),

    #[error("No albums found in directory {dir}")]
    NoAlbumsFound { dir: PathBuf },
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{does_file_have_embedded_artwork, transcode_song, FfmpegError};

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

    #[test]
    fn transcode_embedded_album_art() -> Result<(), FfmpegError> {
        let file_with_embedded_artwork: PathBuf =
            "/home/aida/portable_music/Ado/狂言/04. FREEDOM.mp3".into();
        let target: PathBuf = "/tmp/test_transcode_keep_embedded_album_art.mp3".into();
        transcode_song(&file_with_embedded_artwork, &target, 3, true, None)?;
        assert!(does_file_have_embedded_artwork(&target));

        Ok(())
    }

    #[test]
    fn transcode_drop_album_art() -> Result<(), FfmpegError> {
        let file_with_embedded_artwork: PathBuf =
            "/home/aida/portable_music/Ado/狂言/04. FREEDOM.mp3".into();
        let target: PathBuf = "/tmp/test_transcode_drop_embedded_album_art.mp3".into();
        transcode_song(&file_with_embedded_artwork, &target, 3, false, None)?;
        assert!(!does_file_have_embedded_artwork(&target));

        Ok(())
    }
}
