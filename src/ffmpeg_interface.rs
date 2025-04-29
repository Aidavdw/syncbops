use crate::music_library::MusicFileType;
use itertools::Itertools;
use serde_json::Value as JsonValue;
use std::{
    path::{Path, PathBuf},
    process::Command,
};

/// Gets stuff like title, artist name, etc.
/// Also, whether the song has album art.
#[derive(Debug, PartialEq, Eq)]
pub struct SongMetaData {
    pub title: Option<String>,
    pub bitrate_kbps: u32,
    pub has_embedded_album_art: bool,
}

impl SongMetaData {
    pub fn parse_file(path: &Path) -> Result<SongMetaData, FfmpegError> {
        parse_music_file_metadata(path)
    }
}

fn parse_music_file_metadata(path: &Path) -> Result<SongMetaData, FfmpegError> {
    // Try to run `ffprobe -loglevel 0 -print_format json -show_format -show_streams <path>`
    let mut binding = Command::new("ffprobe");
    binding
        .arg("-loglevel")
        .arg("0")
        .arg("-print_format")
        .arg("json")
        .arg("-show_format")
        .arg("-show_streams")
        .arg(path);
    let ffprobe = binding
        .output()
        .map_err(|e| FfmpegError::CheckForAlbumArtCommand {
            source: e,
            arguments: binding
                .get_args()
                .map(|osstr| osstr.to_string_lossy())
                .join(" "),
        })?;
    let ffprobe_json_output = String::from_utf8(ffprobe.stdout).unwrap();
    let parsed: JsonValue =
        serde_json::from_str(&ffprobe_json_output).map_err(|_| FfmpegError::JsonMetadata)?;
    dbg!(&parsed);

    // The first stream must be the audio.
    let audio_stream: &JsonValue = &parsed["streams"][0];

    let JsonValue::String(first_stream) = &audio_stream["codec_type"] else {
        return Err(FfmpegError::JsonMetadata);
    };
    assert!(first_stream == "audio");

    // If it is given as a string, turn it into a number.
    let Some(bitrate_kbps) = match &audio_stream["bit_rate"] {
        JsonValue::Number(x) => x.as_u64().map(|a| a as u32),
        JsonValue::String(s) => s.parse::<u32>().ok(),
        _ => None,
    }
    // If bitrate of audio track is not given, then we can approximate it with the length
    .or_else(||
        // The file bit rate also includes the image stream, so it will be higher.
        match &parsed["format"]["bit_rate"] {
            JsonValue::Number(x) => x.as_u64().map(|a| a as u32),
            JsonValue::String(s) => s.parse::<u32>().ok(),
            _ => None,
        })
    .map(|bits_per_second| bits_per_second / 1000) else {
        return Err(FfmpegError::Bitrate {
            path: path.to_str().unwrap().to_owned(),
        });
    };

    // Extract the title from the global metadata block
    let title = parsed["format"]["tags"]["title"]
        .as_str()
        // in FLAC, often fully capitalised
        .or_else(|| parsed["format"]["tags"]["TITLE"].as_str())
        // in .ogg, sometimes the global metadata block is missing. Then try the audio
        // stream-specific block.
        .or_else(|| audio_stream["tags"]["TITLE"].as_str())
        .or_else(|| audio_stream["tags"]["title"].as_str())
        .or_else(|| todo!("Can't extract title. Implement other fallbacks!"))
        .map(|s| s.to_owned());

    // To check if the thing has album art, just check if there is a video stream.
    let video_stream: &JsonValue = &parsed["streams"][1];
    let has_embedded_album_art = !video_stream.is_null();
    // debug_assert!(video_stream["codec_type"].as_str().unwrap() == "video")

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
        // Replace file if it already exists
        .arg("-y")
        // input url: the source file
        .arg("-i")
        .arg(source);

    if embed_art {
        if let Some(path) = external_art_to_embed {
            // Second input url: the external album art.
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
            binding.arg("libmp3lame");
            if vbr {
                // Specific for vbr: quality scale of the audio track, instead of the bitrate.
                // should be between 0 and 9. See https://trac.ffmpeg.org/wiki/Encode/MP3#VBREncoding
                binding.arg("-q:a").arg(quality.to_string());
            } else {
                // Constant bitrate in kbps.
                // See https://trac.ffmpeg.org/wiki/Encode/MP3#VBREncoding
                binding.arg("-b:a").arg(format!("{}k", constant_bitrate));
            }
        }
        _ => panic!("MusicFileType not yet implemented as a target."),
    }

    // Take all the metadata from file 0 (source library music file).
    // For both the global metadata (0) and the metadata of the first stream (0:s:0)
    // This also handles conversion of metadata (e.g. from VORBIS comments) to ID3v2
    binding
        .arg("-map_metadata")
        .arg("0")
        .arg("-map_metadata")
        .arg("0:s:0");

    // More metadata mapping operations:
    match target_type {
        MusicFileType::Mp3 { .. } => {
            // Write tags as ID3v2.3. This is more broadly supported than ID3v2.4.
            binding.arg("-id3v2_version").arg("3")
        }
        MusicFileType::Opus { .. } => todo!(),
        MusicFileType::Vorbis { .. } => todo!(),
        MusicFileType::Flac { .. } => todo!(),
    };

    // TODO: Downscale art if it is higher resolution than required. If the desired resolution is
    // higher, then don't do any scaling.

    if external_art_to_embed.is_some() && embed_art {
        // We have an external art to embed.
        // TODO: Check if the external art is higher quality than the already embedded art. If it is,
        // prefer using that, unless the resolution is already exactly the target resolution.

        // It becomes `ffmpeg -i input.wav -i cover.jpg -codec:a libmp3lame -qscale:a 2 -metadata:s:v title="Cover" -metadata:s:v comment="Cover" -map 0:a -map 1:v output.mp3`
        binding
            // give the title "cover" to the inserted album art
            .arg("-metadata:s:v")
            .arg("title=\"Cover\"")
            // give the comment "cover" to the inserted album art.
            // Some music players look for comment instead of title.
            .arg("-metadata:s:v")
            .arg("comments=\"Cover\"")
            // Use the first provided file (source library audio file) as the audio track
            .arg("-map")
            .arg("0:a")
            // Use the second provided source (external album art) as the video track.
            .arg("-map")
            .arg("1:v");
    } else if !embed_art {
        // -vn drops the video track
        binding.arg("-vn");
    }

    binding.arg(target);

    // Check if there is any problem with the generated command. If this error occurs, it is
    // most likely an implementation error
    let output = binding
        .output()
        .map_err(|e| FfmpegError::TranscodeCommand {
            source: e,
            arguments: binding
                .get_args()
                .map(|osstr| osstr.to_string_lossy())
                .join(" "),
        })?;
    // Check if there was a problem with running ffmpeg.
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

    #[error("Could not parse json metadata output from ffprobe.")]
    JsonMetadata,
}

#[cfg(test)]
mod tests {
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
        assert!(md.bitrate_kbps == 169);
        Ok(())
    }

    #[test]
    fn metadata_mp3_without_art() -> miette::Result<()> {
        let md = SongMetaData::parse_file(&mp3_without_art())?;
        dbg!(&md);
        assert!(!md.has_embedded_album_art);
        assert!(md.title == Some("mp3 without art".to_string()));
        assert!(md.bitrate_kbps == 180);
        Ok(())
    }

    #[test]
    fn metadata_flac_with_art() -> miette::Result<()> {
        let md = SongMetaData::parse_file(&flac_with_art())?;
        dbg!(&md);
        assert!(md.has_embedded_album_art);
        assert!(md.title == Some("flac with art".to_string()));
        // Not actual bitrate, because uses the fallback approximation here
        assert!(md.bitrate_kbps == 1070);
        Ok(())
    }

    #[test]
    fn metadata_flac_without_art() -> miette::Result<()> {
        let md = SongMetaData::parse_file(&flac_without_art())?;
        dbg!(&md);
        assert!(!md.has_embedded_album_art);
        assert!(md.title == Some("Flac without art".to_string()));
        assert!(md.bitrate_kbps == 869);
        Ok(())
    }

    #[test]
    fn metadata_ogg_with_art() -> miette::Result<()> {
        let md = SongMetaData::parse_file(&ogg_with_art())?;
        dbg!(&md);
        assert!(md.has_embedded_album_art);
        assert!(md.title == Some("ogg with art".to_string()));
        assert!(md.bitrate_kbps == 499);
        Ok(())
    }

    #[test]
    fn metadata_ogg_without_art() -> miette::Result<()> {
        let md = SongMetaData::parse_file(&ogg_without_art())?;
        dbg!(&md);
        assert!(!md.has_embedded_album_art);
        assert!(md.title == Some("vorbis without art".to_string()));
        assert!(md.bitrate_kbps == 499);
        Ok(())
    }

    #[test]
    fn metadata_m4a_with_art() -> miette::Result<()> {
        let md = SongMetaData::parse_file(&m4a_with_art())?;
        dbg!(&md);
        assert!(md.has_embedded_album_art);
        assert!(md.title == Some("m4a with art".to_string()));
        assert!(md.bitrate_kbps == 197);
        Ok(())
    }

    #[test]
    fn metadata_m4a_without_art() -> miette::Result<()> {
        let md = SongMetaData::parse_file(&m4a_without_art())?;
        dbg!(&md);
        assert!(!md.has_embedded_album_art);
        assert!(md.title == Some("m4a without art".to_string()));
        assert!(md.bitrate_kbps == 198);
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
        let source_md = SongMetaData::parse_file(&source)?;
        let target_md = SongMetaData::parse_file(&target)?;
        if source_md.has_embedded_album_art || external_art_to_embed.is_some() {
            assert_eq!(target_md.has_embedded_album_art, embed_art)
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
