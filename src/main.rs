mod ffmpeg_interface;
mod music_library;
mod song;

use clap::{arg, value_parser};
use music_library::{
    find_albums_in_directory, songs_without_album_art, sync_song, Album, MusicLibraryError,
    UpdateType,
};
use song::Song;
use std::fs;
use std::path::{Path, PathBuf};
fn main() -> miette::Result<()> {
    // Long arguments with dashes need to be in "", per https://github.com/clap-rs/clap/issues/3586
    let cmd = clap::Command::new("musicsync")
        .bin_name("music_portable_sync")
        .about("Make a smaller version of your music library, and keep it in sync with your main library. Useful for keeping on a phone!")
        .version(clap::crate_version!())
        .arg_required_else_help(true)
        .args([
            arg!(<INPUT> "Input directory for walking").required(true).value_parser(value_parser!(PathBuf)),
            arg!(<OUTPUT> "Output directory").required(true).value_parser(value_parser!(PathBuf)),
            // LAME uses V1 etc, ffmpeg actually uses -q:a 1. https://trac.ffmpeg.org/wiki/Encode/MP3 
            arg!(-c --compressionlevel <MP3_COMPRESSION_LEVEL> "Target average bitrate preset for MP3 VBR compression. Only supply the number, e.g '0'. V0 = 245, V1 = 225, V2 = 190, up to V9 = 65 kbit/s. Defaults to V3 = 175kbit/s"),
            arg!(-f --force "Force overwrite existing files"),
            arg!(--preserve "Preserve target folder files, even if they don't exist in source dir"),
            arg!(--"dont-copy-cover-image-files" "Don't copy cover images found in the music library."),
            arg!(--"embed-art" <EMBED_ART> "How to handle embedded art. 'none' removes all embedded art. 'retain' keeps embedded art, but does not embed new art. 'retain_if_no_album_art' keeps only embedded art for albums where no cover art file is found. 'embed' forces embedding for every track (this might take up extra space). Defaults to 'retain_if_no_album_art.'"),
            arg!(--"embed-art-resolution" "Maximum resolution for embedded art. Files lower in resolution will not be touched. Default (not set) will embed everything at their actual resolution."),
            arg!(--"cover-image-file-names" <COVER> "Cover image suffix (case-insensitive). Default to Cover.jpg,Cover.png,AlbumArtSmall.jpg,AlbumArtwork.png")
        ]);
    let matches = cmd.get_matches();

    let source_library: PathBuf = matches
        .get_one::<PathBuf>("INPUT")
        .expect("no library dir given")
        .to_path_buf();

    println!("Discovering files in {}", source_library.display());
    let albums = find_albums_in_directory(&source_library).unwrap();
    println!(
        "Discovered {} songs in {} folders.",
        albums
            .iter()
            .map(|album| album.music_files.len())
            .sum::<usize>(),
        albums.len()
    );
    // Report if there are songs without album art.
    let songs_without_album_art = songs_without_album_art(&albums);
    if !songs_without_album_art.is_empty() {
        println!("Warning! There are songs without any album art (embedded or found in Cover.jpg, folder.png, etc:");
        for x in songs_without_album_art {
            println!("\t- {}", x.display())
        }
    }

    // Convert Albums to Songs
    let mut songs = Vec::new();
    for album in albums {
        songs.extend(album.music_files.iter().map(|music_file| Song {
            path: music_file.to_path_buf(),
            external_album_art: album.album_art.clone(),
        }));
    }
    let songs = songs; // unmut

    let target_library: PathBuf = matches
        .get_one::<PathBuf>("OUTPUT")
        .expect("no output library dir given")
        .to_path_buf();
    // If the target dir coes not exist, warn the user that it does not exist. Don't just
    // willy-nilly create it, because they could've made a typo.
    if !target_library.is_dir() {
        return Err(ClientError::TargetLibraryDoesNotExist {
            target_library: target_library.clone(),
        }
        .into());
    }

    let v_level = 3;
    let include_album_art = false;

    let res = songs
        .iter()
        .map(|song| {
            sync_song(
                song,
                &source_library,
                &target_library,
                v_level,
                include_album_art,
            )
        })
        .collect::<Vec<_>>();

    // Do the synchronising on a per-file basis, so that it can be parallelised. Each one starting
    // with its own ffmpeg thread.
    for file_res in res {
        match file_res {
            Ok(x) => println!("song OK: {:?}", x),
            Err(e) => eprintln!("Error processing a song: {}", e),
        }
    }

    Ok(())
    // TODO: Separately search for "albumname.jpg" everywhere. Match this to the albums by
    // reading their tags, and link it if the album does not yet have art set.
}

#[derive(thiserror::Error, Debug, miette::Diagnostic)]
pub enum ClientError {
    #[error("The given target directory '{target_library}' does not (yet) exist. Please make sure the folder exists, even if it is just an empty folder!")]
    TargetLibraryDoesNotExist { target_library: PathBuf },
}

//fn write_log(sync_results: &[Result<UpdateType, MusicLibraryError>]) {}
