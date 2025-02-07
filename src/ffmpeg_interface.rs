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
    // TODO: Instead, check if it has a video stream: If the title is different (the default for
    // ffmpeg is "other") then it won't recognise it.
    //TODO: Increase test coverage, to see if this works with .m4a, FLAC, etc.
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

    let embed_external_artwork = include_album_art
        && !does_file_have_embedded_artwork(source)
        && external_album_art.is_some();

    let mut binding = Command::new("ffmpeg");
    binding
        .arg("-y") // Replace if it already exists
        .arg("-i")
        .arg(source);

    if embed_external_artwork {
        // safe to unwrap, already checked before.
        binding.arg("-i").arg(external_album_art.unwrap());
    };
    binding
        .arg("-codec:a")
        .arg("libmp3lame")
        .arg("-q:a")
        .arg(v_level.to_string());

    // TODO: embed artwork if missing
    if embed_external_artwork {
        // It becomes `ffmpeg -i input.wav -i cover.jpg -codec:a libmp3lame -qscale:a 2 -metadata:s:v title="Cover" -metadata:s:v comment="Cover" -map 0:a -map 1:v output.mp3`
        binding
            .arg("-metadata:s:v")
            .arg("title=\"Cover\"")
            .arg("-metadata:s:v")
            .arg("comments=\"Cover\"")
            .arg("-map")
            .arg("0:a")
            .arg("-map")
            .arg("1");
    } else if !include_album_art {
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
    fn transcode_no_embedded_album_art() -> Result<(), FfmpegError> {
        let file_without_embedded_artwork: PathBuf =
            "/home/aida/portable_music/Area 11/All The Lights In The Sky/1-02. Vectors.mp3".into();
        let target: PathBuf = "/tmp/test_transcode_never_had_embedded_album_art.mp3".into();
        transcode_song(&file_without_embedded_artwork, &target, 3, true, None)?;
        // TODO: Throw an error if you include_album_art, but the final file is not able to embed
        // album art.
        assert!(!does_file_have_embedded_artwork(&target));

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

    #[test]
    fn transcode_embed_external_album_art() -> Result<(), FfmpegError> {
        let file_without_embedded_artwork: PathBuf =
            "/home/aida/portable_music/Area 11/All The Lights In The Sky/1-02. Vectors.mp3".into();
        let external_artwork_file: PathBuf =
            "/home/aida/portable_music/Area 11/All The Lights In The Sky/folder.jpg".into();
        let target: PathBuf = "/tmp/test_transcode_newly_embedded_album_art.mp3".into();
        transcode_song(
            &file_without_embedded_artwork,
            &target,
            3,
            true,
            Some(&external_artwork_file),
        )?;
        assert!(does_file_have_embedded_artwork(&target));

        Ok(())
    }
}
