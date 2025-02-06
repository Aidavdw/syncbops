mod music_library;

use std::path::{Path, PathBuf};

use clap::{arg, value_parser};
use music_library::find_albums_in_directory;
use walkdir::{DirEntry, WalkDir};
fn main() {
    // Long arguments with dashes need to be in "", per https://github.com/clap-rs/clap/issues/3586
    let cmd = clap::Command::new("musicsync")
        .bin_name("music_portable_sync")
        .about("Make a smaller version of your music library, and keep it in sync with your main library. Useful for keeping on a phone!")
        .version(clap::crate_version!())
        .arg_required_else_help(true)
        .args([
            arg!(<INPUT> "Input directory for walking").required(true).value_parser(value_parser!(PathBuf)),
            arg!(<OUTPUT> "Output directory").required(true),
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

    let library_dir: PathBuf = matches
        .get_one::<PathBuf>("INPUT")
        .expect("no library dir given")
        .to_path_buf();

    println!("Discovering files in {}", library_dir.display());
    let albums = find_albums_in_directory(&library_dir).unwrap();
    println!(
        "Discovered {} songs in {} folders.",
        albums
            .iter()
            .map(|album| album.music_files.len())
            .sum::<usize>(),
        albums.len()
    );

    // Iterate through the folders. If there is a music file here, then this should be an
    // album.
    // if there are no music files here, then go some level deeper, because there might be
    // music in a sub-folder.
    // If there are no music files, and there are also no sub-folders, then ignore this foledr
    // and continue with the next one.
}
