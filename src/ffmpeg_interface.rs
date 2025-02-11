use itertools::Itertools;
use std::{
    path::{Path, PathBuf},
    process::Command,
};

/// Queries `ffprobe "04. FREEDOM.mp3" 2>&1 | grep "Cover"`.
pub fn does_file_have_embedded_artwork(path: &Path) -> Result<bool, FfmpegError> {
    let mut binding = Command::new("ffprobe");
    binding.arg(path);
    let arguments = binding
        .get_args()
        .map(|osstr| osstr.to_string_lossy())
        .join(" ");
    let ffprobe = binding
        .output()
        .map_err(|e| FfmpegError::CheckForAlbumArtCommand {
            source: e,
            arguments,
        })?;
    let txt = String::from_utf8(ffprobe.stderr).unwrap();
    // TODO: Instead, check if it has a video stream: If the title is different (the default for
    // ffmpeg is "other") then it won't recognise it.
    //TODO: Increase test coverage, to see if this works with .m4a, FLAC, etc.
    Ok(txt.contains("Cover"))
}

/// Takes a path of a song file, transcodes it using ffmpeg, and saves it to the target path. Returns the path of the output file. Like `ffmpeg -i [input file] -codec:a libmp3lame -q:a [V-level] [output file].mp3`
pub fn transcode_song(
    source: &Path,
    target: &Path,
    v_level: u32,
    include_album_art: bool,
    external_art_to_embed: Option<&Path>,
) -> Result<(), FfmpegError> {
    debug_assert!(
        v_level < 9,
        "Presets for v-level compression only go up to 9. Be sure to sanitise input."
    );

    let mut binding = Command::new("ffmpeg");
    binding
        .arg("-y") // Replace if it already exists
        .arg("-i")
        .arg(source);

    if include_album_art {
        if let Some(path) = external_art_to_embed {
            binding.arg("-i").arg(path);
        }
    }

    binding
        .arg("-codec:a")
        .arg("libmp3lame")
        .arg("-q:a")
        .arg(v_level.to_string());

    // TODO: embed artwork if missing
    if external_art_to_embed.is_some() && include_album_art {
        // It becomes `ffmpeg -i input.wav -i cover.jpg -codec:a libmp3lame -qscale:a 2 -metadata:s:v title="Cover" -metadata:s:v comment="Cover" -map 0:a -map 1:v output.mp3`
        binding
            .arg("-metadata:s:v")
            .arg("title=\"Cover\"")
            .arg("-metadata:s:v")
            .arg("comments=\"Cover\"")
            .arg("-map")
            .arg("0:a")
            .arg("-map")
            .arg("1:v");
    } else if !include_album_art {
        binding.arg("-vn");
    }

    binding.arg(target);
    let arguments = binding
        .get_args()
        .map(|osstr| osstr.to_string_lossy())
        .join(" ");
    let output = binding
        .output()
        .map_err(|e| FfmpegError::TranscodeCommand {
            source: e,
            arguments,
        })?;
    if !output.status.success() {
        let cmd_txt = binding
            .get_args()
            .map(|osstr| osstr.to_string_lossy())
            .join(" ");
        let msg = String::from_utf8_lossy(&output.stderr).to_string();
        return Err(FfmpegError::FfmpegNotSuccesful {
            file: source.into(),
            arguments: cmd_txt,
            msg,
        });
    }
    Ok(())
}

#[derive(thiserror::Error, Debug, miette::Diagnostic)]
pub enum FfmpegError {
    #[error(
        "ffmpeg exited with a failure code for file {file}. Tried calling `ffmpeg {arguments}`. Output of ffmpeg: {msg} "
    )]
    FfmpegNotSuccesful {
        file: PathBuf,
        arguments: String,
        msg: String,
    },

    #[error("could not run the command to transcode a music file. Ran ffmpeg with arguments `{arguments}`: {source}")]
    TranscodeCommand {
        source: std::io::Error,
        arguments: String,
    },

    #[error("could not use ffmpeg to check for album art. Ran ffmpeg with arguments `{arguments}`: {source}")]
    CheckForAlbumArtCommand {
        source: std::io::Error,
        arguments: String,
    },
}

#[cfg(test)]
mod tests {
    use super::{does_file_have_embedded_artwork, transcode_song};
    use std::path::PathBuf;

    #[test]
    fn embedded_artwork() -> miette::Result<()> {
        let file_with_embedded_artwork: PathBuf =
            "/home/aida/portable_music/Ado/狂言/04. FREEDOM.mp3".into();
        assert!(does_file_have_embedded_artwork(
            &file_with_embedded_artwork
        )?);

        let file_without_embedded_artwork: PathBuf =
            "/home/aida/portable_music/Area 11/All The Lights In The Sky/1-02. Vectors.mp3".into();
        assert!(!does_file_have_embedded_artwork(
            &file_without_embedded_artwork
        )?);
        Ok(())
    }

    #[test]
    fn transcode_embedded_album_art() -> miette::Result<()> {
        let file_with_embedded_artwork: PathBuf =
            "/home/aida/portable_music/Ado/狂言/04. FREEDOM.mp3".into();
        let target: PathBuf = "/tmp/test_transcode_keep_embedded_album_art.mp3".into();
        transcode_song(&file_with_embedded_artwork, &target, 3, true, None)?;
        assert!(does_file_have_embedded_artwork(&target)?);

        Ok(())
    }

    #[test]
    fn transcode_no_embedded_album_art() -> miette::Result<()> {
        let file_without_embedded_artwork: PathBuf =
            "/home/aida/portable_music/Area 11/All The Lights In The Sky/1-02. Vectors.mp3".into();
        let target: PathBuf = "/tmp/test_transcode_never_had_embedded_album_art.mp3".into();
        transcode_song(&file_without_embedded_artwork, &target, 3, true, None)?;
        // TODO: Throw an error if you include_album_art, but the final file is not able to embed
        // album art.
        assert!(!does_file_have_embedded_artwork(&target)?);

        Ok(())
    }

    #[test]
    fn transcode_drop_album_art() -> miette::Result<()> {
        let file_with_embedded_artwork: PathBuf =
            "/home/aida/portable_music/Ado/狂言/04. FREEDOM.mp3".into();
        let target: PathBuf = "/tmp/test_transcode_drop_embedded_album_art.mp3".into();
        transcode_song(&file_with_embedded_artwork, &target, 3, false, None)?;
        assert!(!does_file_have_embedded_artwork(&target)?);

        Ok(())
    }

    #[test]
    fn transcode_embed_external_album_art() -> miette::Result<()> {
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
        assert!(does_file_have_embedded_artwork(&target)?);

        Ok(())
    }
}
