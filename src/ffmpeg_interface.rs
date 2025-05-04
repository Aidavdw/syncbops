use crate::music_library::MusicFileType;
use itertools::Itertools;
use serde_json::Value as JsonValue;
use std::{
    path::{Path, PathBuf},
    process::Command,
};

/// Gets stuff like title, artist name, etc.
/// Also, whether the song has album art.
#[derive(Debug)]
pub struct SongMetaData {
    pub title: Option<String>,
    pub bitrate_kbps: u32,
    pub has_embedded_album_art: bool,
    // TODO: Extend with Duration, Artist, Album Artist, Album, etc. Considering how many tags
    // there are, maybe even save all actual 'tags' as a hashmap.
}

impl SongMetaData {
    pub fn parse_file(path: &Path) -> Result<SongMetaData, FfmpegError> {
        parse_music_file_metadata(path)
    }
}

fn parse_music_file_metadata(path: &Path) -> Result<SongMetaData, FfmpegError> {
    if !path.exists() {
        return Err(FfmpegError::FileDoesNotExist {
            path: path.to_str().unwrap().to_owned(),
        });
    }

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
    // dbg!(&parsed);

    // There must be only one audio stream here, but there might be more video streams (different
    // art).
    // Usually, the first stream is the audio stream, but it might not be.
    let audio_stream = &parsed["streams"]
        .as_array()
        .expect("streams is not an array?")
        .iter()
        .find(|stream| {
            let JsonValue::String(first_stream) = &stream["codec_type"] else {
                return false;
            };
            first_stream == "audio"
        })
        .expect("File does not have an audio stream.");

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

    use MusicFileType as M;
    match target_type {
        M::Mp3VBR { quality } => {
            binding.arg("libmp3lame");
            // Specific for vbr: quality scale of the audio track, instead of the bitrate.
            // should be between 0 and 9. See https://trac.ffmpeg.org/wiki/Encode/MP3#VBREncoding
            binding.arg("-q:a").arg(quality.to_string());
        }
        M::Mp3CBR { bitrate } => {
            binding.arg("libmp3lame");
            // Constant bitrate in kbps.
            // See https://trac.ffmpeg.org/wiki/Encode/MP3#VBREncoding
            binding.arg("-b:a").arg(format!("{}k", bitrate));
        }
        M::Vorbis { quality } => {
            binding
                .arg("libvorbis")
                .arg("-qscale:a")
                .arg(format!("{quality:.3}"));
        }
        M::Opus {
            bitrate,
            compression_level,
        } => {
            binding
                .arg("libopus")
                .arg("-b:a")
                .arg(format!("{}k", bitrate));
        }
        M::Flac { quality: _ } => {
            panic!("Encoding to flac not yet implemented as a target. Feel free to send a PR <3")
        }
    }

    // Take all the metadata from file 0 (source library music file).
    // For both the global metadata (0) and the metadata of the first stream (0:s:0)
    // This also handles conversion of metadata (e.g. from VORBIS comments) to ID3v2
    binding
        .arg("-map_metadata")
        .arg("0")
        .arg("-map_metadata")
        .arg("0:s:0");

    // TODO: For some reason, when transcoding MP3 to Ogg, it really wants to put the video track
    // first. At least, that is what ffprobe reports. I don't think this is a problem, but maybe
    // this should be fixed.

    // More metadata mapping operations:
    match target_type {
        MusicFileType::Mp3VBR { .. } => {
            // Write tags as ID3v2.3. This is more broadly supported than ID3v2.4.
            binding.arg("-id3v2_version").arg("3");
        }
        MusicFileType::Mp3CBR { .. } => {
            // Write tags as ID3v2.3. This is more broadly supported than ID3v2.4.
            binding.arg("-id3v2_version").arg("3");
        }
        MusicFileType::Opus { .. } => (),
        MusicFileType::Vorbis { .. } => (),
        MusicFileType::Flac { .. } => (),
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

#[derive(thiserror::Error, Debug)]
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

    #[error("Could not run FFmpeg on {path}, because it does not exist.")]
    FileDoesNotExist { path: String },
}

#[cfg(test)]
mod tests {
    use super::FfmpegError;
    use crate::{
        ffmpeg_interface::SongMetaData, music_library::MusicFileType, test_data::TestFile,
    };
    use std::path::PathBuf;

    // miette::Diagnostic/ miette::Result is only used in tests, so can't use the derive macro.
    impl miette::Diagnostic for FfmpegError {}

    #[test]
    fn metadata_mp3_with_art() -> miette::Result<()> {
        let md = SongMetaData::parse_file(&TestFile::Mp3CBRWithArt.path())?;
        dbg!(&md);
        assert!(md.has_embedded_album_art);
        assert!(md.title == Some("mp3 with art".to_string()));
        assert!(md.bitrate_kbps == 169);
        Ok(())
    }

    #[test]
    fn metadata_mp3_without_art() -> miette::Result<()> {
        let md = SongMetaData::parse_file(&TestFile::Mp3CBRWithoutArt.path())?;
        dbg!(&md);
        assert!(!md.has_embedded_album_art);
        assert!(md.title == Some("mp3 without art".to_string()));
        assert!(md.bitrate_kbps == 180);
        Ok(())
    }

    #[test]
    fn metadata_flac_with_art() -> miette::Result<()> {
        let md = SongMetaData::parse_file(&TestFile::FlacWithArt.path())?;
        dbg!(&md);
        assert!(md.has_embedded_album_art);
        assert!(md.title == Some("flac with art".to_string()));
        // Not actual bitrate, because uses the fallback approximation here
        assert!(md.bitrate_kbps == 1070);
        Ok(())
    }

    #[test]
    fn metadata_flac_without_art() -> miette::Result<()> {
        let md = SongMetaData::parse_file(&TestFile::FlacWithoutArt.path())?;
        dbg!(&md);
        assert!(!md.has_embedded_album_art);
        assert!(md.title == Some("Flac without art".to_string()));
        assert!(md.bitrate_kbps == 869);
        Ok(())
    }

    #[test]
    fn metadata_ogg_with_art() -> miette::Result<()> {
        let md = SongMetaData::parse_file(&TestFile::OggWithArt.path())?;
        dbg!(&md);
        assert!(md.has_embedded_album_art);
        assert!(md.title == Some("ogg with art".to_string()));
        assert!(md.bitrate_kbps == 499);
        Ok(())
    }

    #[test]
    fn metadata_ogg_without_art() -> miette::Result<()> {
        let md = SongMetaData::parse_file(&TestFile::OggWithoutArt.path())?;
        dbg!(&md);
        assert!(!md.has_embedded_album_art);
        assert!(md.title == Some("vorbis without art".to_string()));
        assert!(md.bitrate_kbps == 499);
        Ok(())
    }

    #[test]
    fn metadata_m4a_with_art() -> miette::Result<()> {
        let md = SongMetaData::parse_file(&TestFile::M4aWithArt.path())?;
        dbg!(&md);
        assert!(md.has_embedded_album_art);
        assert!(md.title == Some("m4a with art".to_string()));
        assert!(md.bitrate_kbps == 197);
        Ok(())
    }

    #[test]
    fn metadata_m4a_without_art() -> miette::Result<()> {
        let md = SongMetaData::parse_file(&TestFile::M4aWithoutArt.path())?;
        dbg!(&md);
        assert!(!md.has_embedded_album_art);
        assert!(md.title == Some("m4a without art".to_string()));
        assert!(md.bitrate_kbps == 198);
        Ok(())
    }

    // Convenience function to see if file transcoding actually works as intended.
    fn transcode_file_test(
        test_file: TestFile,
        embed_art: bool,
        external_art_to_embed: Option<TestFile>,
        target_type: MusicFileType,
    ) -> miette::Result<()> {
        use super::transcode_song;
        let source = test_file.path();

        let random_string = random_string::generate(16, "abcdefghijklmnopqrstuvwxyz");
        let target: PathBuf = format!(
            "/tmp/syncbops/transcode_test_{:?}_{}.{}",
            test_file, random_string, target_type
        )
        .into();
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
            target_type,
            embed_art,
            external_art_to_embed.clone().map(|tf| tf.path()).as_deref(),
        )?;
        assert!(std::fs::exists(&target).unwrap());
        let source_md = SongMetaData::parse_file(&source)?;
        let target_md = SongMetaData::parse_file(&target)?;

        // Album art needs to be the set or removed
        if source_md.has_embedded_album_art || external_art_to_embed.is_some() {
            assert_eq!(target_md.has_embedded_album_art, embed_art)
        }

        // Tags need to be identical. Album art might not be if set to embed.
        assert_eq!(source_md.title, target_md.title);

        // Don't do checks for if the target file is smaller here! That's the responsibility
        // of sync_song, not of transcode.

        Ok(())
    }

    mod to_mp3_vbr {
        use crate::{music_library::MusicFileType, test_data::TestFile};

        /// Setting up a test to transcode into mp3 vbr
        fn build(
            test_file: TestFile,
            embed_art: bool,
            external_art_to_embed: Option<TestFile>,
        ) -> miette::Result<()> {
            super::transcode_file_test(
                test_file,
                embed_art,
                external_art_to_embed,
                MusicFileType::Mp3VBR { quality: 6 },
            )
        }

        // START TRANSCODING TO MP3 VBR

        #[test]
        /// Attempt to get embedded art, even though no art is supplied
        fn mp3_no_art_embed() -> miette::Result<()> {
            build(TestFile::Mp3CBRWithoutArt, true, None)
        }

        #[test]
        /// Keep embedded art
        fn mp3_keep_embedded_art() -> miette::Result<()> {
            build(TestFile::Mp3CBRWithArt, true, None)
        }

        #[test]
        /// drop embedded album art
        fn mp3_embedded_art_drop() -> miette::Result<()> {
            build(TestFile::Mp3CBRWithArt, false, None)
        }

        #[test]
        /// drop external art
        fn mp3_external_art_drop() -> miette::Result<()> {
            build(TestFile::Mp3CBRWithoutArt, false, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed external art
        fn mp3_external_art_embed() -> miette::Result<()> {
            build(TestFile::Mp3CBRWithoutArt, true, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed, supplied are both external art and already embedded.
        fn mp3_both_embed() -> miette::Result<()> {
            build(TestFile::Mp3CBRWithArt, true, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed, supplied are both external art and already embedded.
        fn mp3_both_drop() -> miette::Result<()> {
            build(TestFile::Mp3CBRWithArt, false, Some(TestFile::Jpg600))
        }

        #[test]
        /// Attempt to get embedded art, even though no art is supplied
        fn ogg_no_art_embed() -> miette::Result<()> {
            build(TestFile::OggWithoutArt, true, None)
        }

        #[test]
        /// Keep embedded art
        fn ogg_keep_embedded_art() -> miette::Result<()> {
            build(TestFile::OggWithArt, true, None)
        }

        #[test]
        /// drop embedded album art
        fn ogg_embedded_art_drop() -> miette::Result<()> {
            build(TestFile::OggWithArt, false, None)
        }

        #[test]
        /// drop external art
        fn ogg_external_art_drop() -> miette::Result<()> {
            build(TestFile::OggWithoutArt, false, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed external art
        fn ogg_external_art_embed() -> miette::Result<()> {
            build(TestFile::OggWithoutArt, true, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed, supplied are both external art and already embedded.
        fn ogg_both_embed() -> miette::Result<()> {
            build(TestFile::OggWithArt, true, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed, supplied are both external art and already embedded.
        fn ogg_both_drop() -> miette::Result<()> {
            build(TestFile::OggWithArt, false, Some(TestFile::Jpg600))
        }

        #[test]
        /// Attempt to get embedded art, even though no art is supplied
        fn flac_no_art_embed() -> miette::Result<()> {
            build(TestFile::FlacWithoutArt, true, None)
        }

        #[test]
        /// Keep embedded art
        fn flac_keep_embedded_art() -> miette::Result<()> {
            build(TestFile::FlacWithArt, true, None)
        }

        #[test]
        /// drop embedded album art
        fn flac_embedded_art_drop() -> miette::Result<()> {
            build(TestFile::FlacWithArt, false, None)
        }

        #[test]
        /// drop external art
        fn flac_external_art_drop() -> miette::Result<()> {
            build(TestFile::FlacWithoutArt, false, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed external art
        fn flac_external_art_embed() -> miette::Result<()> {
            build(TestFile::FlacWithoutArt, true, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed, supplied are both external art and already embedded.
        fn flac_both_embed() -> miette::Result<()> {
            build(TestFile::FlacWithArt, true, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed, supplied are both external art and already embedded.
        fn flac_both_drop() -> miette::Result<()> {
            build(TestFile::FlacWithArt, false, Some(TestFile::Jpg600))
        }

        #[test]
        /// Attempt to get embedded art, even though no art is supplied
        fn m4a_no_art_embed() -> miette::Result<()> {
            build(TestFile::Rotterdam96kbpsOpus, true, None)
        }

        #[test]
        /// Keep embedded art
        fn m4a_keep_embedded_art() -> miette::Result<()> {
            build(TestFile::M4aWithArt, true, None)
        }

        #[test]
        /// drop embedded album art
        fn m4a_embedded_art_drop() -> miette::Result<()> {
            build(TestFile::M4aWithArt, false, None)
        }

        #[test]
        /// drop external art
        fn m4a_external_art_drop() -> miette::Result<()> {
            build(TestFile::M4aWithoutArt, false, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed external art
        fn m4a_external_art_embed() -> miette::Result<()> {
            build(TestFile::M4aWithoutArt, true, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed, supplied are both external art and already embedded.
        fn m4a_both_embed() -> miette::Result<()> {
            build(TestFile::M4aWithArt, true, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed, supplied are both external art and already embedded.
        fn m4a_both_drop() -> miette::Result<()> {
            build(TestFile::M4aWithArt, false, Some(TestFile::Jpg600))
        }

        #[test]
        /// Attempt to get embedded art, even though no art is supplied
        fn opus_no_art_embed() -> miette::Result<()> {
            build(TestFile::Rotterdam96kbpsOpus, true, None)
        }

        #[test]
        /// Keep embedded art
        fn opus_keep_embedded_art() -> miette::Result<()> {
            build(TestFile::Rotterdam96kbpsOpusWithArt, true, None)
        }

        #[test]
        /// drop embedded album art
        fn opus_embedded_art_drop() -> miette::Result<()> {
            build(TestFile::Rotterdam96kbpsOpusWithArt, false, None)
        }

        #[test]
        /// drop external art
        fn opus_external_art_drop() -> miette::Result<()> {
            build(TestFile::Rotterdam96kbpsOpus, false, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed external art
        fn opus_external_art_embed() -> miette::Result<()> {
            build(TestFile::Rotterdam96kbpsOpus, true, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed, supplied are both external art and already embedded.
        fn opus_both_embed() -> miette::Result<()> {
            build(
                TestFile::Rotterdam96kbpsOpusWithArt,
                true,
                Some(TestFile::Jpg600),
            )
        }

        #[test]
        /// embed, supplied are both external art and already embedded.
        fn opus_both_drop() -> miette::Result<()> {
            build(
                TestFile::Rotterdam96kbpsOpusWithArt,
                false,
                Some(TestFile::Jpg600),
            )
        }
    }

    mod to_mp3_cbr {
        use crate::{music_library::MusicFileType, test_data::TestFile};

        /// Setting up a test to transcode into mp3 vbr
        fn build(
            test_file: TestFile,
            embed_art: bool,
            external_art_to_embed: Option<TestFile>,
        ) -> miette::Result<()> {
            super::transcode_file_test(
                test_file,
                embed_art,
                external_art_to_embed,
                MusicFileType::Mp3CBR { bitrate: 80 },
            )
        }

        // START TRANSCODING TO MP3 VBR

        #[test]
        /// Attempt to get embedded art, even though no art is supplied
        fn mp3_no_art_embed() -> miette::Result<()> {
            build(TestFile::Mp3CBRWithoutArt, true, None)
        }

        #[test]
        /// Keep embedded art
        fn mp3_keep_embedded_art() -> miette::Result<()> {
            build(TestFile::Mp3CBRWithArt, true, None)
        }

        #[test]
        /// drop embedded album art
        fn mp3_embedded_art_drop() -> miette::Result<()> {
            build(TestFile::Mp3CBRWithArt, false, None)
        }

        #[test]
        /// drop external art
        fn mp3_external_art_drop() -> miette::Result<()> {
            build(TestFile::Mp3CBRWithoutArt, false, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed external art
        fn mp3_external_art_embed() -> miette::Result<()> {
            build(TestFile::Mp3CBRWithoutArt, true, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed, supplied are both external art and already embedded.
        fn mp3_both_embed() -> miette::Result<()> {
            build(TestFile::Mp3CBRWithArt, true, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed, supplied are both external art and already embedded.
        fn mp3_both_drop() -> miette::Result<()> {
            build(TestFile::Mp3CBRWithArt, false, Some(TestFile::Jpg600))
        }

        #[test]
        /// Attempt to get embedded art, even though no art is supplied
        fn ogg_no_art_embed() -> miette::Result<()> {
            build(TestFile::OggWithoutArt, true, None)
        }

        #[test]
        /// Keep embedded art
        fn ogg_keep_embedded_art() -> miette::Result<()> {
            build(TestFile::OggWithArt, true, None)
        }

        #[test]
        /// drop embedded album art
        fn ogg_embedded_art_drop() -> miette::Result<()> {
            build(TestFile::OggWithArt, false, None)
        }

        #[test]
        /// drop external art
        fn ogg_external_art_drop() -> miette::Result<()> {
            build(TestFile::OggWithoutArt, false, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed external art
        fn ogg_external_art_embed() -> miette::Result<()> {
            build(TestFile::OggWithoutArt, true, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed, supplied are both external art and already embedded.
        fn ogg_both_embed() -> miette::Result<()> {
            build(TestFile::OggWithArt, true, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed, supplied are both external art and already embedded.
        fn ogg_both_drop() -> miette::Result<()> {
            build(TestFile::OggWithArt, false, Some(TestFile::Jpg600))
        }

        #[test]
        /// Attempt to get embedded art, even though no art is supplied
        fn flac_no_art_embed() -> miette::Result<()> {
            build(TestFile::FlacWithoutArt, true, None)
        }

        #[test]
        /// Keep embedded art
        fn flac_keep_embedded_art() -> miette::Result<()> {
            build(TestFile::FlacWithArt, true, None)
        }

        #[test]
        /// drop embedded album art
        fn flac_embedded_art_drop() -> miette::Result<()> {
            build(TestFile::FlacWithArt, false, None)
        }

        #[test]
        /// drop external art
        fn flac_external_art_drop() -> miette::Result<()> {
            build(TestFile::FlacWithoutArt, false, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed external art
        fn flac_external_art_embed() -> miette::Result<()> {
            build(TestFile::FlacWithoutArt, true, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed, supplied are both external art and already embedded.
        fn flac_both_embed() -> miette::Result<()> {
            build(TestFile::FlacWithArt, true, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed, supplied are both external art and already embedded.
        fn flac_both_drop() -> miette::Result<()> {
            build(TestFile::FlacWithArt, false, Some(TestFile::Jpg600))
        }

        #[test]
        /// Attempt to get embedded art, even though no art is supplied
        fn m4a_no_art_embed() -> miette::Result<()> {
            build(TestFile::Rotterdam96kbpsOpus, true, None)
        }

        #[test]
        /// Keep embedded art
        fn m4a_keep_embedded_art() -> miette::Result<()> {
            build(TestFile::M4aWithArt, true, None)
        }

        #[test]
        /// drop embedded album art
        fn m4a_embedded_art_drop() -> miette::Result<()> {
            build(TestFile::M4aWithArt, false, None)
        }

        #[test]
        /// drop external art
        fn m4a_external_art_drop() -> miette::Result<()> {
            build(TestFile::M4aWithoutArt, false, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed external art
        fn m4a_external_art_embed() -> miette::Result<()> {
            build(TestFile::M4aWithoutArt, true, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed, supplied are both external art and already embedded.
        fn m4a_both_embed() -> miette::Result<()> {
            build(TestFile::M4aWithArt, true, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed, supplied are both external art and already embedded.
        fn m4a_both_drop() -> miette::Result<()> {
            build(TestFile::M4aWithArt, false, Some(TestFile::Jpg600))
        }

        #[test]
        /// Attempt to get embedded art, even though no art is supplied
        fn opus_no_art_embed() -> miette::Result<()> {
            build(TestFile::Rotterdam96kbpsOpus, true, None)
        }

        #[test]
        /// Keep embedded art
        fn opus_keep_embedded_art() -> miette::Result<()> {
            build(TestFile::Rotterdam96kbpsOpusWithArt, true, None)
        }

        #[test]
        /// drop embedded album art
        fn opus_embedded_art_drop() -> miette::Result<()> {
            build(TestFile::Rotterdam96kbpsOpusWithArt, false, None)
        }

        #[test]
        /// drop external art
        fn opus_external_art_drop() -> miette::Result<()> {
            build(TestFile::Rotterdam96kbpsOpus, false, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed external art
        fn opus_external_art_embed() -> miette::Result<()> {
            build(TestFile::Rotterdam96kbpsOpus, true, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed, supplied are both external art and already embedded.
        fn opus_both_embed() -> miette::Result<()> {
            build(
                TestFile::Rotterdam96kbpsOpusWithArt,
                true,
                Some(TestFile::Jpg600),
            )
        }

        #[test]
        /// embed, supplied are both external art and already embedded.
        fn opus_both_drop() -> miette::Result<()> {
            build(
                TestFile::Rotterdam96kbpsOpusWithArt,
                false,
                Some(TestFile::Jpg600),
            )
        }
    }

    mod to_ogg {
        use crate::{music_library::MusicFileType, test_data::TestFile};

        /// Setting up a test to transcode into mp3 vbr
        fn build(
            test_file: TestFile,
            embed_art: bool,
            external_art_to_embed: Option<TestFile>,
        ) -> miette::Result<()> {
            super::transcode_file_test(
                test_file,
                embed_art,
                external_art_to_embed,
                MusicFileType::Vorbis { quality: 2. },
            )
        }

        // START TRANSCODING TO MP3 VBR

        #[test]
        /// Attempt to get embedded art, even though no art is supplied
        fn mp3_no_art_embed() -> miette::Result<()> {
            build(TestFile::Mp3CBRWithoutArt, true, None)
        }

        #[test]
        /// Keep embedded art
        fn mp3_keep_embedded_art() -> miette::Result<()> {
            build(TestFile::Mp3CBRWithArt, true, None)
        }

        #[test]
        /// drop embedded album art
        fn mp3_embedded_art_drop() -> miette::Result<()> {
            build(TestFile::Mp3CBRWithArt, false, None)
        }

        #[test]
        /// drop external art
        fn mp3_external_art_drop() -> miette::Result<()> {
            build(TestFile::Mp3CBRWithoutArt, false, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed external art
        fn mp3_external_art_embed() -> miette::Result<()> {
            build(TestFile::Mp3CBRWithoutArt, true, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed, supplied are both external art and already embedded.
        fn mp3_both_embed() -> miette::Result<()> {
            build(TestFile::Mp3CBRWithArt, true, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed, supplied are both external art and already embedded.
        fn mp3_both_drop() -> miette::Result<()> {
            build(TestFile::Mp3CBRWithArt, false, Some(TestFile::Jpg600))
        }

        #[test]
        /// Attempt to get embedded art, even though no art is supplied
        fn ogg_no_art_embed() -> miette::Result<()> {
            build(TestFile::OggWithoutArt, true, None)
        }

        #[test]
        /// Keep embedded art
        fn ogg_keep_embedded_art() -> miette::Result<()> {
            build(TestFile::OggWithArt, true, None)
        }

        #[test]
        /// drop embedded album art
        fn ogg_embedded_art_drop() -> miette::Result<()> {
            build(TestFile::OggWithArt, false, None)
        }

        #[test]
        /// drop external art
        fn ogg_external_art_drop() -> miette::Result<()> {
            build(TestFile::OggWithoutArt, false, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed external art
        fn ogg_external_art_embed() -> miette::Result<()> {
            build(TestFile::OggWithoutArt, true, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed, supplied are both external art and already embedded.
        fn ogg_both_embed() -> miette::Result<()> {
            build(TestFile::OggWithArt, true, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed, supplied are both external art and already embedded.
        fn ogg_both_drop() -> miette::Result<()> {
            build(TestFile::OggWithArt, false, Some(TestFile::Jpg600))
        }

        #[test]
        /// Attempt to get embedded art, even though no art is supplied
        fn flac_no_art_embed() -> miette::Result<()> {
            build(TestFile::FlacWithoutArt, true, None)
        }

        #[test]
        /// Keep embedded art
        fn flac_keep_embedded_art() -> miette::Result<()> {
            build(TestFile::FlacWithArt, true, None)
        }

        #[test]
        /// drop embedded album art
        fn flac_embedded_art_drop() -> miette::Result<()> {
            build(TestFile::FlacWithArt, false, None)
        }

        #[test]
        /// drop external art
        fn flac_external_art_drop() -> miette::Result<()> {
            build(TestFile::FlacWithoutArt, false, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed external art
        fn flac_external_art_embed() -> miette::Result<()> {
            build(TestFile::FlacWithoutArt, true, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed, supplied are both external art and already embedded.
        fn flac_both_embed() -> miette::Result<()> {
            build(TestFile::FlacWithArt, true, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed, supplied are both external art and already embedded.
        fn flac_both_drop() -> miette::Result<()> {
            build(TestFile::FlacWithArt, false, Some(TestFile::Jpg600))
        }

        #[test]
        /// Attempt to get embedded art, even though no art is supplied
        fn m4a_no_art_embed() -> miette::Result<()> {
            build(TestFile::Rotterdam96kbpsOpus, true, None)
        }

        #[test]
        /// Keep embedded art
        fn m4a_keep_embedded_art() -> miette::Result<()> {
            build(TestFile::M4aWithArt, true, None)
        }

        #[test]
        /// drop embedded album art
        fn m4a_embedded_art_drop() -> miette::Result<()> {
            build(TestFile::M4aWithArt, false, None)
        }

        #[test]
        /// drop external art
        fn m4a_external_art_drop() -> miette::Result<()> {
            build(TestFile::M4aWithoutArt, false, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed external art
        fn m4a_external_art_embed() -> miette::Result<()> {
            build(TestFile::M4aWithoutArt, true, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed, supplied are both external art and already embedded.
        fn m4a_both_embed() -> miette::Result<()> {
            build(TestFile::M4aWithArt, true, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed, supplied are both external art and already embedded.
        fn m4a_both_drop() -> miette::Result<()> {
            build(TestFile::M4aWithArt, false, Some(TestFile::Jpg600))
        }

        #[test]
        /// Attempt to get embedded art, even though no art is supplied
        fn opus_no_art_embed() -> miette::Result<()> {
            build(TestFile::Rotterdam96kbpsOpus, true, None)
        }

        #[test]
        /// Keep embedded art
        fn opus_keep_embedded_art() -> miette::Result<()> {
            build(TestFile::Rotterdam96kbpsOpusWithArt, true, None)
        }

        #[test]
        /// drop embedded album art
        fn opus_embedded_art_drop() -> miette::Result<()> {
            build(TestFile::Rotterdam96kbpsOpusWithArt, false, None)
        }

        #[test]
        /// drop external art
        fn opus_external_art_drop() -> miette::Result<()> {
            build(TestFile::Rotterdam96kbpsOpus, false, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed external art
        fn opus_external_art_embed() -> miette::Result<()> {
            build(TestFile::Rotterdam96kbpsOpus, true, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed, supplied are both external art and already embedded.
        fn opus_both_embed() -> miette::Result<()> {
            build(
                TestFile::Rotterdam96kbpsOpusWithArt,
                true,
                Some(TestFile::Jpg600),
            )
        }

        #[test]
        /// embed, supplied are both external art and already embedded.
        fn opus_both_drop() -> miette::Result<()> {
            build(
                TestFile::Rotterdam96kbpsOpusWithArt,
                false,
                Some(TestFile::Jpg600),
            )
        }
    }

    mod to_opus {
        use crate::{music_library::MusicFileType, test_data::TestFile};

        /// Setting up a test to transcode into mp3 vbr
        fn build(
            test_file: TestFile,
            embed_art: bool,
            external_art_to_embed: Option<TestFile>,
        ) -> miette::Result<()> {
            super::transcode_file_test(
                test_file,
                embed_art,
                external_art_to_embed,
                MusicFileType::Opus {
                    bitrate: 96,
                    compression_level: 3,
                },
            )
        }

        // START TRANSCODING TO MP3 VBR

        #[test]
        /// Attempt to get embedded art, even though no art is supplied
        fn mp3_no_art_embed() -> miette::Result<()> {
            build(TestFile::Mp3CBRWithoutArt, true, None)
        }

        #[test]
        /// Keep embedded art
        fn mp3_keep_embedded_art() -> miette::Result<()> {
            build(TestFile::Mp3CBRWithArt, true, None)
        }

        #[test]
        /// drop embedded album art
        fn mp3_embedded_art_drop() -> miette::Result<()> {
            build(TestFile::Mp3CBRWithArt, false, None)
        }

        #[test]
        /// drop external art
        fn mp3_external_art_drop() -> miette::Result<()> {
            build(TestFile::Mp3CBRWithoutArt, false, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed external art
        fn mp3_external_art_embed() -> miette::Result<()> {
            build(TestFile::Mp3CBRWithoutArt, true, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed, supplied are both external art and already embedded.
        fn mp3_both_embed() -> miette::Result<()> {
            build(TestFile::Mp3CBRWithArt, true, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed, supplied are both external art and already embedded.
        fn mp3_both_drop() -> miette::Result<()> {
            build(TestFile::Mp3CBRWithArt, false, Some(TestFile::Jpg600))
        }

        #[test]
        /// Attempt to get embedded art, even though no art is supplied
        fn ogg_no_art_embed() -> miette::Result<()> {
            build(TestFile::OggWithoutArt, true, None)
        }

        #[test]
        /// Keep embedded art
        fn ogg_keep_embedded_art() -> miette::Result<()> {
            build(TestFile::OggWithArt, true, None)
        }

        #[test]
        /// drop embedded album art
        fn ogg_embedded_art_drop() -> miette::Result<()> {
            build(TestFile::OggWithArt, false, None)
        }

        #[test]
        /// drop external art
        fn ogg_external_art_drop() -> miette::Result<()> {
            build(TestFile::OggWithoutArt, false, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed external art
        fn ogg_external_art_embed() -> miette::Result<()> {
            build(TestFile::OggWithoutArt, true, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed, supplied are both external art and already embedded.
        fn ogg_both_embed() -> miette::Result<()> {
            build(TestFile::OggWithArt, true, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed, supplied are both external art and already embedded.
        fn ogg_both_drop() -> miette::Result<()> {
            build(TestFile::OggWithArt, false, Some(TestFile::Jpg600))
        }

        #[test]
        /// Attempt to get embedded art, even though no art is supplied
        fn flac_no_art_embed() -> miette::Result<()> {
            build(TestFile::FlacWithoutArt, true, None)
        }

        #[test]
        /// Keep embedded art
        fn flac_keep_embedded_art() -> miette::Result<()> {
            build(TestFile::FlacWithArt, true, None)
        }

        #[test]
        /// drop embedded album art
        fn flac_embedded_art_drop() -> miette::Result<()> {
            build(TestFile::FlacWithArt, false, None)
        }

        #[test]
        /// drop external art
        fn flac_external_art_drop() -> miette::Result<()> {
            build(TestFile::FlacWithoutArt, false, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed external art
        fn flac_external_art_embed() -> miette::Result<()> {
            build(TestFile::FlacWithoutArt, true, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed, supplied are both external art and already embedded.
        fn flac_both_embed() -> miette::Result<()> {
            build(TestFile::FlacWithArt, true, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed, supplied are both external art and already embedded.
        fn flac_both_drop() -> miette::Result<()> {
            build(TestFile::FlacWithArt, false, Some(TestFile::Jpg600))
        }

        #[test]
        /// Attempt to get embedded art, even though no art is supplied
        fn m4a_no_art_embed() -> miette::Result<()> {
            build(TestFile::Rotterdam96kbpsOpus, true, None)
        }

        #[test]
        /// Keep embedded art
        fn m4a_keep_embedded_art() -> miette::Result<()> {
            build(TestFile::M4aWithArt, true, None)
        }

        #[test]
        /// drop embedded album art
        fn m4a_embedded_art_drop() -> miette::Result<()> {
            build(TestFile::M4aWithArt, false, None)
        }

        #[test]
        /// drop external art
        fn m4a_external_art_drop() -> miette::Result<()> {
            build(TestFile::M4aWithoutArt, false, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed external art
        fn m4a_external_art_embed() -> miette::Result<()> {
            build(TestFile::M4aWithoutArt, true, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed, supplied are both external art and already embedded.
        fn m4a_both_embed() -> miette::Result<()> {
            build(TestFile::M4aWithArt, true, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed, supplied are both external art and already embedded.
        fn m4a_both_drop() -> miette::Result<()> {
            build(TestFile::M4aWithArt, false, Some(TestFile::Jpg600))
        }

        #[test]
        /// Attempt to get embedded art, even though no art is supplied
        fn opus_no_art_embed() -> miette::Result<()> {
            build(TestFile::Rotterdam96kbpsOpus, true, None)
        }

        #[test]
        /// Keep embedded art
        fn opus_keep_embedded_art() -> miette::Result<()> {
            build(TestFile::Rotterdam96kbpsOpusWithArt, true, None)
        }

        #[test]
        /// drop embedded album art
        fn opus_embedded_art_drop() -> miette::Result<()> {
            build(TestFile::Rotterdam96kbpsOpusWithArt, false, None)
        }

        #[test]
        /// drop external art
        fn opus_external_art_drop() -> miette::Result<()> {
            build(TestFile::Rotterdam96kbpsOpus, false, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed external art
        fn opus_external_art_embed() -> miette::Result<()> {
            build(TestFile::Rotterdam96kbpsOpus, true, Some(TestFile::Jpg600))
        }

        #[test]
        /// embed, supplied are both external art and already embedded.
        fn opus_both_embed() -> miette::Result<()> {
            build(
                TestFile::Rotterdam96kbpsOpusWithArt,
                true,
                Some(TestFile::Jpg600),
            )
        }

        #[test]
        /// embed, supplied are both external art and already embedded.
        fn opus_both_drop() -> miette::Result<()> {
            build(
                TestFile::Rotterdam96kbpsOpusWithArt,
                false,
                Some(TestFile::Jpg600),
            )
        }
    }
    // #[test]
    // /// Comparing how long it takes to hash a file versuse how long it takes to get metadata.
    // /// The shorter of the two should be preferred to be done first when comparing files.
    // fn time_hash() {
    //     use std::time::Instant;
    //
    //     let now = Instant::now();
    //     {
    //         for testfile in COMPARISON_BENCHMARK_TEST_FILES {
    //             let _ = parse_music_file_metadata(&testfile.path()).unwrap();
    //         }
    //     }
    //     let elapsed = now.elapsed();
    //     let avg_time_per_item = elapsed / COMPARISON_BENCHMARK_TEST_FILES.len() as u32;
    //     panic!(
    //         "parsing music file metadata takes avg {:.6?} per file",
    //         avg_time_per_item
    //     );
    // }
}
