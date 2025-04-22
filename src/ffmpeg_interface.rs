use itertools::Itertools;
use regex::Regex;
use std::{
    path::{Path, PathBuf},
    process::Command,
};

use crate::music_library::MusicFileType;

/// Queries `ffprobe "<filename>" 2>&1 | grep "Cover"`.
pub fn does_file_have_embedded_artwork(path: &Path) -> Result<bool, FfmpegError> {
    let mut binding = Command::new("ffprobe");
    binding.arg(path);
    let ffprobe = binding
        .output()
        .map_err(|e| FfmpegError::CheckForAlbumArtCommand {
            source: e,
            arguments: binding
                .get_args()
                .map(|osstr| osstr.to_string_lossy())
                .join(" "),
        })?;
    let txt = String::from_utf8(ffprobe.stderr).unwrap();
    // In ffmpeg, embedded artworks are considered as extra "streams". They are, confusingly enough, of type video. Generally they are also tagged with a meta tag, such as "cover"
    Ok(txt.contains("Video"))
}

/// Gets stuff like title, artist name, etc.
/// Also, whether the song has album art.
#[derive(Debug)]
pub struct SongMetaData {
    title: Option<String>,
    bitrate_kbps: Option<u32>,
    has_embedded_album_art: bool,
}

impl SongMetaData {
    pub fn parse_file(path: &Path) -> Result<SongMetaData, FfmpegError> {
        parse_music_file_metadata(path)
    }
}

fn parse_music_file_metadata(path: &Path) -> Result<SongMetaData, FfmpegError> {
    let mut binding = Command::new("ffprobe");
    binding.arg(path);
    let ffprobe = binding
        .output()
        .map_err(|e| FfmpegError::CheckForAlbumArtCommand {
            source: e,
            arguments: binding
                .get_args()
                .map(|osstr| osstr.to_string_lossy())
                .join(" "),
        })?;
    let txt = String::from_utf8(ffprobe.stderr).unwrap();

    // Regex patterns to match the title, bitrate, and check for streams
    let metadata_re = Regex::new(r"(?s)Metadata:\n(?P<meta>(?:\s{4,}.+\n)+)").unwrap();
    let title_re = Regex::new(r"^\s{4,}title\s*:\s*(?P<title>.+)$").unwrap();
    let bitrate_re = Regex::new(r"Stream #\d+:\d+: Audio:.*?, (?P<bitrate>\d+)\s*kb/s").unwrap();
    let has_video_re = Regex::new(r"Stream #\d+:\d+: Video:.*\(attached pic\)").unwrap();

    // 1. Extract the title from the global metadata block
    let title = metadata_re.captures(&txt).and_then(|cap| {
        cap["meta"].lines().find_map(|line| {
            title_re
                .captures(line)
                .map(|m| m["title"].trim().to_string())
        })
    });

    // 2. Extract audio bitrate
    let bitrate_kbps = bitrate_re
        .captures(&txt)
        .and_then(|cap| cap.name("bitrate"))
        .and_then(|m| m.as_str().parse().ok());

    // 3. Check for embedded album art (presence of video stream with "attached pic")
    let has_embedded_album_art = has_video_re.is_match(&txt);

    Ok(SongMetaData {
        title,
        bitrate_kbps,
        has_embedded_album_art,
    })
}

/// Takes a path of a song file, transcodes it using ffmpeg, and saves it to the target path. Returns the path of the output file. Like `ffmpeg -i [input file] -codec:a libmp3lame -q:a [V-level] [output file].mp3`
pub fn transcode_song(
    source: &Path,
    target: &Path,
    target_type: MusicFileType,
    embed_art: bool,
    external_art_to_embed: Option<&Path>,
) -> Result<(), FfmpegError> {
    let mut binding = Command::new("ffmpeg");
    binding
        .arg("-y") // Replace if it already exists
        .arg("-i")
        .arg(source);

    if embed_art {
        if let Some(path) = external_art_to_embed {
            binding.arg("-i").arg(path);
        }
    }

    // Mp3:
    // `ffmpeg -i input.wav -i cover.jpg -codec:a libmp3lame -qscale:a 2 -metadata:s:v title="Cover" -metadata:s:v comment="Cover" -map 0:a -map 1:v output.mp3`

    binding.arg("-codec:a");

    match target_type {
        MusicFileType::Mp3 {
            constant_bitrate,
            vbr,
            quality,
        } => {
            // TODO: Write tags as ID3v2.3. Right now it is ID3v4, which is less broadly supported.
            binding.arg("libmp3lame");
            if vbr {
                binding.arg("-q:a").arg(quality.to_string());
            } else {
                binding.arg("-b:a").arg(format!("{}k", constant_bitrate));
            }
        }
        _ => panic!("MusicFileType not yet implemented as a target."),
    }

    if external_art_to_embed.is_some() && embed_art {
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
    } else if !embed_art {
        binding.arg("-vn");
    }

    binding.arg(target);
    let output = binding
        .output()
        .map_err(|e| FfmpegError::TranscodeCommand {
            source: e,
            arguments: binding
                .get_args()
                .map(|osstr| osstr.to_string_lossy())
                .join(" "),
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

    #[error("Could not determine the bitrate for file `{path}`")]
    Bitrate { path: String },
}

#[cfg(test)]
mod tests {
    use super::does_file_have_embedded_artwork;
    use crate::{ffmpeg_interface::SongMetaData, music_library::MusicFileType};
    use std::path::PathBuf;

    fn mp3_with_art() -> PathBuf {
        let mut d = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        d.push("test_data/with_art.mp3");
        d
    }

    fn mp3_without_art() -> PathBuf {
        let mut d = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        d.push("test_data/no_art.mp3");
        d
    }

    fn flac_with_art() -> PathBuf {
        let mut d = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        d.push("test_data/with_art.flac");
        d
    }

    fn flac_without_art() -> PathBuf {
        let mut d = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        d.push("test_data/no_art.flac");
        d
    }

    fn m4a_with_art() -> PathBuf {
        let mut d = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        d.push("test_data/with_art.m4a");
        d
    }

    fn m4a_without_art() -> PathBuf {
        let mut d = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        d.push("test_data/no_art.m4a");
        d
    }

    fn ogg_with_art() -> PathBuf {
        let mut d = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        d.push("test_data/with_art.ogg");
        d
    }

    fn ogg_without_art() -> PathBuf {
        let mut d = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        d.push("test_data/no_art.ogg");
        d
    }

    fn external_art() -> Option<PathBuf> {
        let mut d = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        d.push("test_data/cover_art.jpg");
        Some(d)
    }

    #[test]
    fn metadata_mp3_with_art() -> miette::Result<()> {
        let md = SongMetaData::parse_file(&mp3_with_art())?;
        dbg!(&md);
        assert!(md.has_embedded_album_art);
        assert!(md.title == Some("mp3 with art".to_string()));
        assert!(md.bitrate_kbps == Some(169));
        Ok(())
    }

    #[test]
    fn metadata_mp3_without_art() -> miette::Result<()> {
        let md = SongMetaData::parse_file(&mp3_without_art())?;
        dbg!(&md);
        assert!(!md.has_embedded_album_art);
        assert!(md.title == Some("mp3 without art".to_string()));
        assert!(md.bitrate_kbps == Some(180));
        Ok(())
    }

    #[test]
    fn metadata_flac_with_art() -> miette::Result<()> {
        let md = SongMetaData::parse_file(&flac_with_art())?;
        dbg!(&md);
        assert!(md.has_embedded_album_art);
        assert!(md.title == Some("flac with art".to_string()));
        assert!(md.bitrate_kbps.is_none());
        Ok(())
    }

    #[test]
    fn does_file_have_cover_art_flac_no() -> miette::Result<()> {
        let md = SongMetaData::parse_file(&flac_without_art())?;
        dbg!(&md);
        assert!(!md.has_embedded_album_art);
        assert!(md.title == Some("flac without art".to_string()));
        assert!(md.bitrate_kbps.is_none());
        Ok(())
    }

    #[test]
    fn does_file_have_cover_art_ogg_yes() -> miette::Result<()> {
        assert!(does_file_have_embedded_artwork(&ogg_with_art())?);
        Ok(())
    }

    #[test]
    fn does_file_have_cover_art_ogg_no() -> miette::Result<()> {
        assert!(!does_file_have_embedded_artwork(&ogg_without_art())?);
        Ok(())
    }

    #[test]
    fn does_file_have_cover_art_m4a_yes() -> miette::Result<()> {
        assert!(does_file_have_embedded_artwork(&m4a_with_art())?);
        Ok(())
    }

    #[test]
    fn does_file_have_cover_art_m4a_no() -> miette::Result<()> {
        assert!(!does_file_have_embedded_artwork(&m4a_without_art())?);
        Ok(())
    }

    fn transcode_file_test(
        source: PathBuf,
        embed_art: bool,
        external_art_to_embed: Option<PathBuf>,
    ) -> miette::Result<()> {
        use super::transcode_song;
        let random_string = random_string::generate(16, "abcdefghijklmnopqrstuvwxyz");
        let target: PathBuf = format!("/tmp/transcode_test_{}.mp3", random_string).into();
        println!("Using {}", target.display());
        assert!(
            !std::fs::exists(&target).unwrap(),
            "Astronomically small chance but randomly generated file already exists lol"
        );
        let _ = std::fs::create_dir_all(target.parent().unwrap());
        let _ = std::fs::remove_file(&target);

        transcode_song(
            &source,
            &target,
            MusicFileType::Mp3 {
                constant_bitrate: 0,
                vbr: true,
                quality: 3,
            },
            embed_art,
            external_art_to_embed.as_deref(),
        )?;
        assert!(std::fs::exists(&target).unwrap());
        if does_file_have_embedded_artwork(&source)? || external_art_to_embed.is_some() {
            assert_eq!(does_file_have_embedded_artwork(&target)?, embed_art);
        }
        Ok(())
    }

    #[test]
    /// Attempt to get embedded art, even though no art is supplied
    fn mp3_no_art_embed() -> miette::Result<()> {
        transcode_file_test(mp3_without_art(), true, None)
    }

    #[test]
    /// Keep embedded art
    fn mp3_keep_embedded_art() -> miette::Result<()> {
        transcode_file_test(mp3_with_art(), true, None)
    }

    #[test]
    /// drop embedded album art
    fn mp3_embedded_art_drop() -> miette::Result<()> {
        transcode_file_test(mp3_with_art(), false, None)
    }

    #[test]
    /// drop external art
    fn mp3_external_art_drop() -> miette::Result<()> {
        transcode_file_test(mp3_without_art(), false, external_art())
    }

    #[test]
    /// embed external art
    fn mp3_external_art_embed() -> miette::Result<()> {
        transcode_file_test(mp3_without_art(), true, external_art())
    }

    #[test]
    /// embed, supplied are both external art and already embedded.
    fn mp3_both_embed() -> miette::Result<()> {
        transcode_file_test(mp3_with_art(), true, external_art())
    }

    #[test]
    /// embed, supplied are both external art and already embedded.
    fn mp3_both_drop() -> miette::Result<()> {
        transcode_file_test(mp3_with_art(), false, external_art())
    }

    #[test]
    /// Attempt to get embedded art, even though no art is supplied
    fn ogg_no_art_embed() -> miette::Result<()> {
        transcode_file_test(ogg_without_art(), true, None)
    }

    #[test]
    /// Keep embedded art
    fn ogg_keep_embedded_art() -> miette::Result<()> {
        transcode_file_test(ogg_with_art(), true, None)
    }

    #[test]
    /// drop embedded album art
    fn ogg_embedded_art_drop() -> miette::Result<()> {
        transcode_file_test(ogg_with_art(), false, None)
    }

    #[test]
    /// drop external art
    fn ogg_external_art_drop() -> miette::Result<()> {
        transcode_file_test(ogg_without_art(), false, external_art())
    }

    #[test]
    /// embed external art
    fn ogg_external_art_embed() -> miette::Result<()> {
        transcode_file_test(ogg_without_art(), true, external_art())
    }

    #[test]
    /// embed, supplied are both external art and already embedded.
    fn ogg_both_embed() -> miette::Result<()> {
        transcode_file_test(ogg_with_art(), true, external_art())
    }

    #[test]
    /// embed, supplied are both external art and already embedded.
    fn ogg_both_drop() -> miette::Result<()> {
        transcode_file_test(ogg_with_art(), false, external_art())
    }

    #[test]
    /// Attempt to get embedded art, even though no art is supplied
    fn flac_no_art_embed() -> miette::Result<()> {
        transcode_file_test(flac_without_art(), true, None)
    }

    #[test]
    /// Keep embedded art
    fn flac_keep_embedded_art() -> miette::Result<()> {
        transcode_file_test(flac_with_art(), true, None)
    }

    #[test]
    /// drop embedded album art
    fn flac_embedded_art_drop() -> miette::Result<()> {
        transcode_file_test(flac_with_art(), false, None)
    }

    #[test]
    /// drop external art
    fn flac_external_art_drop() -> miette::Result<()> {
        transcode_file_test(flac_without_art(), false, external_art())
    }

    #[test]
    /// embed external art
    fn flac_external_art_embed() -> miette::Result<()> {
        transcode_file_test(flac_without_art(), true, external_art())
    }

    #[test]
    /// embed, supplied are both external art and already embedded.
    fn flac_both_embed() -> miette::Result<()> {
        transcode_file_test(flac_with_art(), true, external_art())
    }

    #[test]
    /// embed, supplied are both external art and already embedded.
    fn flac_both_drop() -> miette::Result<()> {
        transcode_file_test(flac_with_art(), false, external_art())
    }

    #[test]
    /// Attempt to get embedded art, even though no art is supplied
    fn m4a_no_art_embed() -> miette::Result<()> {
        transcode_file_test(m4a_without_art(), true, None)
    }

    #[test]
    /// Keep embedded art
    fn m4a_keep_embedded_art() -> miette::Result<()> {
        transcode_file_test(m4a_with_art(), true, None)
    }

    #[test]
    /// drop embedded album art
    fn m4a_embedded_art_drop() -> miette::Result<()> {
        transcode_file_test(m4a_with_art(), false, None)
    }

    #[test]
    /// drop external art
    fn m4a_external_art_drop() -> miette::Result<()> {
        transcode_file_test(m4a_without_art(), false, external_art())
    }

    #[test]
    /// embed external art
    fn m4a_external_art_embed() -> miette::Result<()> {
        transcode_file_test(m4a_without_art(), true, external_art())
    }

    #[test]
    /// embed, supplied are both external art and already embedded.
    fn m4a_both_embed() -> miette::Result<()> {
        transcode_file_test(m4a_with_art(), true, external_art())
    }

    #[test]
    /// embed, supplied are both external art and already embedded.
    fn m4a_both_drop() -> miette::Result<()> {
        transcode_file_test(m4a_with_art(), false, external_art())
    }
}
