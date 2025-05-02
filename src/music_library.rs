use crate::ffmpeg_interface::FfmpegError;
use crate::log_failure;
use crate::song::Song;
use indicatif::ParallelProgressIterator;
use indicatif::ProgressBar;
use indicatif::ProgressStyle;
use itertools::Itertools;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Display;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// How should the file be updated? (or how was it updated last time)
#[derive(Debug, PartialEq, Serialize, Deserialize, Clone, Copy)]
pub enum UpdateType {
    /// The file did not need to be changed, as it is up-to-date
    NoChange,
    /// The file is completely new, so everything had to be done from scratch
    NewTranscode,
    /// Updated because it was modified more recently than the shadow copy
    Overwrite,
    /// Actually unchanged, but forced into being overwritten.
    ForceOverwrite,
    /// The song is present in the SyncDB (It has been synced before),
    /// but the target file is no longer there
    TranscodeMissingTarget,

    /// The target file does not yet exist, and the source file already has a low bitrate.
    /// It should just be copied, and not transcoded.
    Copied,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ArtworkType {
    Embedded,
    External,
    None,
}

#[derive(Clone, Debug, clap::Subcommand)]
pub enum MusicFileType {
    /// Constant bitrate MP3. Very widely supported, not very good.
    Mp3CBR {
        /// The constant bitrate in kbps
        #[arg(short, long, value_name = "BITRATE", default_value_t = 180)]
        bitrate: u32,
    },
    /// Variable bitrate MP3. A decent bit smaller than MP3 CBR, usually at negligible qualtiy
    /// degredation.
    Mp3VBR {
        /// quality factor. From 0 to 9. Lower is higher quality, but larger filesize. See https://trac.ffmpeg.org/wiki/Encode/MP3
        #[arg(short, long, default_value_t = 3)]
        quality: usize,
    },
    /// Transcode to Opus. Nichely supported, but highest quality audio codec. This might not be supported by your ffmpeg build.
    /// You need to explicitly configure the ffmpeg build with --enable-libopus.
    Opus {
        /// Target bitrate in
        #[arg(short, long, value_name = "BITRATE", default_value_t = 180)]
        bitrate: u32,
        /// Compression algorithm complexity. 0-10. Trades quality for encoding time. higher is best quality. Does not affect filesize
        #[arg(short, long, default_value_t = 3)]
        compression_level: usize,
    },
    /// Transcode to Vorbis. Good support, high quality. Not always supported by ffmpeg
    /// You need to explicitly configure the build with --enable-libvorbis.
    Vorbis {
        /// Trades quality for filesize. -1.0 - 10.0 (float!). Higher is better quality.
        #[arg(short, long, default_value_t = 10.0)]
        quality: f64,
    },
    /// Lossless. If a source file is already compressed, it will not be re-encoded.
    Flac {
        /// Compression factor. Trades compilation time for filesize. Higher is smaller file. From 0 to 12.
        #[arg(short, long, default_value_t = 10)]
        quality: u64,
    },
}

impl MusicFileType {
    /// To be able to compare quality and file sizes of different file types.
    pub fn equivalent_bitrate(&self) -> u32 {
        match self {
            MusicFileType::Mp3CBR { bitrate } => *bitrate,
            MusicFileType::Mp3VBR { quality } => match quality {
                // Values obtained from https://trac.ffmpeg.org/wiki/Encode/MP3
                0 => 245,
                1 => 225,
                2 => 190,
                3 => 175,
                4 => 165,
                5 => 130,
                6 => 115,
                7 => 100,
                8 => 85,
                9 => 65,
                _ => panic!("Invalid MP3 VBR quality number."),
            },
            MusicFileType::Opus {
                bitrate,
                compression_level: _,
            } => *bitrate,
            MusicFileType::Vorbis { quality } => {
                let q = *quality;
                // Equation obtained from https://trac.ffmpeg.org/wiki/TheoraVorbisEncodingGuide#VariableBitrateVBR
                (if q < 4. {
                    16. * (q + 4.)
                } else if q < 8. {
                    32. * q
                } else {
                    64. * (q - 4.)
                })
                .round() as u32
            }
            // Sorry man but if you want to transcode into flac you are using the wrong software.
            MusicFileType::Flac { .. } => 800,
        }
    }

    //     pub fn get_extension(path: &Path) -> Option<MusicFileType> {
    //         use MusicFileType as M;
    //         if !path.exists() {
    //             return None;
    //         }
    //         if path.is_dir() {
    //             return None;
    //         };
    //         let ext = path.extension()?.to_ascii_lowercase();
    //
    //         Some(match ext.as_os_str().to_str()? {
    //             "mp3" => M::Mp3 {
    //                 constant_bitrate: 0,
    //                 vbr: false,
    //                 quality: 0,
    //             },
    //             _ => return None,
    //         })
    //     }
}

impl Display for MusicFileType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                MusicFileType::Mp3VBR { .. } => "mp3",
                MusicFileType::Mp3CBR { .. } => "mp3",
                MusicFileType::Opus { .. } => "opus",
                MusicFileType::Vorbis { .. } => "ogg",
                MusicFileType::Flac { .. } => "flac",
            }
        )
    }
}

pub enum ImageType {
    Png,
    Jpg,
}

#[derive(PartialEq, Eq)]
pub enum FileType {
    Folder,
    Music,
    Art,
    // Things like cue files, etc
    Meta,
    Playlist,
}

/// Returns None if the file does not exist or is not identifiable.
fn identify_file_type(path: &Path) -> Option<FileType> {
    if !path.exists() {
        return None;
    }
    if path.is_dir() {
        return Some(FileType::Folder);
    };
    let ext = path.extension()?.to_ascii_lowercase();

    use FileType as F;
    Some(match ext.as_os_str().to_str()? {
        "mp3" => F::Music,
        "m4a" => F::Music,
        "ogg" => F::Music,
        "flac" => F::Music,
        "png" => F::Art,
        "jpg" => F::Art,
        "jpeg" => F::Art,
        "cue" => F::Meta,
        "nfo" => F::Meta,
        "log" => F::Meta,
        "accurip" => F::Meta,
        "lrc" => F::Meta,
        "lyrics" => F::Meta,
        "sfv" => F::Meta,
        "m3u" => F::Playlist,
        _ => return None,
    })
}

/// Checks if the file meets the criteria to be considered dedicated album art: is it named
/// cover.jpg or something?
fn is_image_file_album_art(path: &Path) -> bool {
    // if it's something like "cover" or "folder"
    const ALLOWED_STEMS: [&str; 6] = [
        "cover",
        "folder",
        "album",
        "cover_image",
        "cover_art",
        "front",
    ];
    let stem_is_allowed: bool = ALLOWED_STEMS.iter().any(|x| {
        path.file_stem()
            .is_some_and(|s| s.to_ascii_lowercase() == *x)
    });

    let has_right_extension =
        identify_file_type(path).is_some_and(|file_type| matches!(file_type, FileType::Art));

    stem_is_allowed && has_right_extension
}

pub fn find_songs_in_library(library_root: &Path) -> Result<Vec<Song>, MusicLibraryError> {
    let filenames = WalkDir::new(library_root)
        .into_iter()
        .filter_map(|direntry_res| {
            let item = match direntry_res {
                Ok(x) => x,
                Err(e) => {
                    eprintln!("Could not read subdir in library: {e}",);
                    return None;
                }
            }
            .path()
            .to_path_buf();
            if item.is_dir() {
                return None;
            }
            Some(item)
        })
        .collect_vec();

    // Create an easy-to-access way to find external album art
    let external_album_arts: HashMap<PathBuf, PathBuf> = {
        let mut m = HashMap::with_capacity(20);
        for image_file in filenames
            .iter()
            .filter(|path| is_image_file_album_art(path))
        {
            // TODO: Instead of picking the first one, sort by quality and prefer the highest
            // quality one.
            let containing_directory = image_file
                .parent()
                .expect("should be able to get containing directory of image file.");
            m.entry(containing_directory.to_path_buf())
                .or_insert(image_file.to_path_buf());
        }
        m
    };

    // Since we are also checking the files for metadata, it is worth doing this in parallel.
    let pb = ProgressBar::new(filenames.len() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("[{elapsed}] [{bar:60.cyan/blue}] {pos}/{len} [ETA: {eta}] {msg}")
            .unwrap()
            .progress_chars("#>-"),
    );
    let songs = filenames
        .par_iter()
        // If it is a song file, the processing might take a while because metadata needs to be
        // parsed. If it is not a music file, it will be done very quickly though. Maybe set up
        // some sort of chunking here? Realistically that shouldn't be necessary, because the
        // majority of files in a directory should be music files.
        .progress_with(pb.clone())
        .filter_map(|path| {
            let Some(filetype) = identify_file_type(path) else {
                log_failure(
                    format!("Could not identify file {}", path.display()),
                    Some(&pb),
                );
                return None;
            };
            // Don't do anything if this is not a music file.
            match filetype {
                FileType::Folder => return None,
                FileType::Music => (),
                FileType::Art => return None,
                FileType::Meta => return None,
                FileType::Playlist => return None,
            };
            match process_song_file(path, library_root, &external_album_arts) {
                Ok(song) => Some(song),
                Err(e) => {
                    log_failure(
                        format!("Could not process song at {}: {}", path.display(), e),
                        Some(&pb),
                    );
                    None
                }
            }
        })
        .collect::<Vec<_>>();
    Ok(songs)
}

fn process_song_file(
    song_path: &Path,
    source_library: &Path,
    external_album_arts: &HashMap<PathBuf, PathBuf>,
) -> Result<Song, MusicLibraryError> {
    debug_assert!(matches!(
        identify_file_type(song_path).unwrap(),
        FileType::Music
    ));

    // If there is album art in this folder, use it.
    // If there is not, see if the parent directory maybe has it.
    let containing_folder = song_path.parent().expect("Can't get song parent");
    let external_album_art = external_album_arts
        .get(containing_folder)
        .or_else(|| {
            let one_folder_up = containing_folder
                .parent()
                .expect("Can't access parent's parent.");
            external_album_arts.get(one_folder_up)
        })
        .cloned();
    Song::new(
        song_path.to_path_buf(),
        source_library.to_path_buf(),
        external_album_art,
    )
}

/// Where to put the synchronised copy
pub fn get_shadow_filename(
    library_relative_path: &Path,
    target_library: &Path,
    // TODO: Change to FileType, so I can re-use the same code for images.
    filetype: &MusicFileType,
) -> PathBuf {
    target_library.join(library_relative_path.with_extension(filetype.to_string()))
}

/// How to handle album art
#[derive(Clone, Copy, PartialEq, clap::ValueEnum, Debug)]
pub enum ArtStrategy {
    /// Remove all embedded album art, and don't copy album art files.
    None,
    /// Embeds album art in all files. Carries over album art that was already in source files, and embeds external album art. Might take up more space!
    EmbedAll,
    /// If there is both embedded and external, prefer external. E.g. If there is a cover.jpg (or similar), use that. If there is no dedicated file, use embedded art.
    PreferFile,
    /// Do not embed any cover art: Discard all existing embedded art, only keep cover.jpg if it exists.
    FileOnly,
}

/// gets the path relative to the library.
pub fn library_relative_path(full_path: &Path, source_library: &Path) -> PathBuf {
    full_path
        .strip_prefix(source_library)
        .unwrap()
        .to_path_buf()
}

/// Returns the path to the new cover art if the file is copied over.
pub fn copy_dedicated_cover_art_for_song(
    song: &Song,
    source_library: &Path,
    target_library: &Path,
    dry_run: bool,
) -> Result<Option<PathBuf>, MusicLibraryError> {
    let Some(path) = &song.external_album_art else {
        return Ok(None);
    };

    let relative_path = path.strip_prefix(source_library).unwrap();
    let shadow = target_library.join(relative_path);
    // TODO: Return error on something that is not a "file already exists"
    if !fs::exists(&shadow).unwrap() {
        if !dry_run {
            let _ = std::fs::copy(path, &shadow);
        }
        Ok(Some(shadow))
    } else {
        Ok(None)
    }
}

#[derive(thiserror::Error, Debug, miette::Diagnostic)]
pub enum MusicLibraryError {
    #[error("Could not generate a list of filenames in the source library.")]
    ListFilenames(#[from] std::io::Error),

    #[error("Tried to discover albums in directory '{path}', but that is not a directory.")]
    NotADirectory { path: PathBuf },

    #[error("Could not process reading directory.")]
    CouldNotProcessDir { path: PathBuf },

    #[error("Error in calling ffmpeg")]
    Ffmpeg(#[from] FfmpegError),

    #[error("The given target directory '{target_library}' does not (yet) exist. Please make sure the folder exists, even if it is just an empty folder!")]
    TargetLibraryDoesNotExist { target_library: PathBuf },

    #[error("This output filetype/encoding is not yet supported :(. Feel free to implement it and send a PR <3")]
    OutputCodecNotYetImplemented,

    #[error("Could not hash the file {path}")]
    CantHash { path: PathBuf },
}
